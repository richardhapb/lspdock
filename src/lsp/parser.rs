use crate::proxy::{Pair, config::ProxyConfig};
use std::error::Error;
use std::str;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, trace};

pub struct LspFramedReader<R> {
    reader: BufReader<R>,
}

impl<R: AsyncRead + Unpin> LspFramedReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            reader: BufReader::new(inner),
        }
    }

    /// Read a message from the sender and capture the content
    pub async fn read_message(&mut self) -> Result<Option<String>, Box<dyn Error + Send + Sync>> {
        let content_length = match self.read_headers().await {
            Ok(len) => len,
            Err(e) => {
                error!("Error reading headers: {}", e);
                return Err(e);
            }
        };

        if content_length == 0 {
            return Ok(None);
        }

        trace!(content_length, "Reading body");

        let mut buf = vec![0u8; content_length];
        match self.reader.read_exact(&mut buf).await {
            Ok(_) => {}
            Err(e) => {
                error!("Error reading message body: {}", e);
                return Err(e.into());
            }
        }

        let body = String::from_utf8(buf)?;
        trace!(%body);
        Ok(Some(body))
    }

    /// Read the headers and content-length to read the body accordingly later.
    async fn read_headers(&mut self) -> Result<usize, Box<dyn Error + Send + Sync>> {
        let mut headers_buf = Vec::new();
        let mut temp_buf = [0u8; 1];

        // Read until we find \r\n\r\n
        loop {
            match self.reader.read_exact(&mut temp_buf).await {
                Ok(_) => {
                    headers_buf.push(temp_buf[0]);

                    // Check if we've reached the end of headers (\r\n\r\n)
                    if headers_buf.len() >= 4 {
                        let len = headers_buf.len();
                        if headers_buf[len - 4..] == [b'\r', b'\n', b'\r', b'\n'] {
                            break;
                        }
                    }

                    // Safety check
                    if headers_buf.len() > 8192 {
                        return Err("Headers too large".into());
                    }
                }
                Err(e) => {
                    error!(
                        "Error reading header byte at position {}: {}",
                        headers_buf.len(),
                        e
                    );
                    return Err(e.into());
                }
            }
        }

        let headers_str = match String::from_utf8(headers_buf.clone()) {
            Ok(s) => s,
            Err(e) => {
                error!("Invalid UTF-8 in headers. Raw bytes: {:?}", headers_buf);
                return Err(format!("Invalid UTF-8 in headers: {}", e).into());
            }
        };

        trace!(%headers_str, "Raw headers");

        let mut content_length = None;

        for line in headers_str.split("\r\n") {
            if line.is_empty() {
                continue;
            }

            trace!("Processing header line: '{}'", line);

            // Try to find Content-Length header, being case-insensitive
            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim();
                let value = line[colon_pos + 1..].trim();

                trace!("Header key: '{}', value: '{}'", key, value);

                // Check for Content-Length with case-insensitive matching and handle truncated headers
                if key.eq_ignore_ascii_case("content-length") {
                    match value.parse::<usize>() {
                        Ok(len) => {
                            content_length = Some(len);
                            break;
                        }
                        Err(e) => {
                            error!("Failed to parse Content-Length '{}': {}", value, e);
                            return Err(format!("Invalid Content-Length: {}", value).into());
                        }
                    }
                }
                // Handle the case where the first character is missing (common bug).
                // This bug occurs in Pyright because it provides an incorrect content-length
                // in the first message, this behavior needs research because there may be a reason for it.
                else if key.eq_ignore_ascii_case("ontent-length") {
                    trace!(
                        "Found truncated Content-Length header (missing 'C'), treating as Content-Length"
                    );
                    match value.parse::<usize>() {
                        Ok(len) => {
                            content_length = Some(len);
                            break;
                        }
                        Err(e) => {
                            error!(
                                "Failed to parse truncated Content-Length '{}': {}",
                                value, e
                            );
                            return Err(format!("Invalid Content-Length: {}", value).into());
                        }
                    }
                }
            } else {
                debug!("Header line without colon: '{}'", line);

                // Check if this might be a truncated Content-Length header
                if line.to_lowercase().contains("ontent-length") {
                    error!("Found truncated Content-Length header: '{}'", line);
                    error!("This suggests a bug in the reading logic - missing first character");
                }
            }
        }

        content_length.ok_or_else(|| "Missing Content-Length header".into())
    }
}

/// Send a message from the proxy to the destination
pub async fn send_message(
    writer: &mut tokio::io::BufWriter<impl tokio::io::AsyncWriteExt + Unpin>,
    message: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let len = message.as_bytes().len();
    debug!(%len, "Sending message");
    trace!(%message);
    let msg = format!("Content-Length: {len}\r\n\r\n{message}");

    writer.write_all(msg.as_bytes()).await?;
    writer.flush().await?;

    Ok(())
}

/// Redirect the paths from the sender pair to the receiver pair; this is used
/// for matching the paths between the container and the host path.
pub fn redirect_uri(
    raw_str: &mut String,
    from: &Pair,
    config: &ProxyConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let from_path_str: &str;
    let to_path_str: &str;

    match from {
        Pair::Client => {
            from_path_str = &config.local_path;
            to_path_str = &config.docker_internal_path;
        }
        Pair::Server => {
            from_path_str = &config.docker_internal_path;
            to_path_str = &config.local_path;
        }
    }

    trace!(%from_path_str, %to_path_str);

    *raw_str = raw_str.replace(from_path_str, to_path_str);

    Ok(())
}

/// Patch the processId parameter from the client.
///
/// Patching the PID is necessary because if it is passed to an LSP located inside a Docker
/// container, the LSP will try to detect the PID, and if it is missing inside the container,
/// the LSP will close and break the pipe.
pub fn patch_initialize_process_id(raw_str: &mut String) -> bool {
    if raw_str.contains(r#""method":"initialize""#) {
        debug!("Initialize method found, patching");
        trace!(%raw_str, "before patch");

        let re = regex::Regex::new(r#""processId":\s*\d+"#).unwrap();
        *raw_str = re.replace_all(raw_str, r#""processId":null"#).to_string();

        trace!(%raw_str, "patched");
        return true;
    }
    return false;
}
