use std::error::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_util::bytes::{Buf, BytesMut};
use tracing::{debug, error, trace, warn};

pub struct LspFramedReader<R> {
    reader: BufReader<R>,
    buffer: BytesMut,
}

impl<R: AsyncRead + Unpin> LspFramedReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            reader: BufReader::new(inner),
            buffer: BytesMut::with_capacity(8192),
        }
    }

    /// Read messages from the sender and capture their content
    pub async fn read_messages(
        &mut self,
    ) -> Result<Option<Vec<String>>, Box<dyn Error + Send + Sync>> {
        let mut messages = Vec::new();
        let n = self.reader.read_buf(&mut self.buffer).await?;
        if n == 0 && self.buffer.is_empty() {
            return Ok(None);
        }

        while let Some((message, advance)) = self.try_parse_message() {
            self.buffer.advance(advance);
            messages.push(message);
        }

        Ok(Some(messages))
    }

    fn try_parse_message(&self) -> Option<(String, usize)> {
        let header_end = self.find_header_end()?;
        trace!(header_end);
        let headers = &self.buffer[..header_end];

        let content_length = self.extract_content_length(headers)?;
        let (body, advance) = self.extract_body(header_end, content_length)?;

        Some((body, advance))
    }

    fn find_header_end(&self) -> Option<usize> {
        for i in 3..self.buffer.len() {
            if &self.buffer[i - 3..=i] == b"\r\n\r\n" {
                return Some(i + 1);
            }
        }
        return None;
    }

    fn extract_content_length(&self, headers: &[u8]) -> Option<usize> {
        let headers_str = String::from_utf8(headers.to_vec()).ok()?;
        let mut content_length = None;

        for line in headers_str.split("\r\n") {
            if line.is_empty() {
                continue;
            }

            trace!("Processing header line: '{}'", line);
            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim();
                let value = line[colon_pos + 1..].trim();

                trace!("Header key: '{}', value: '{}'", key, value);

                // Check for Content-Length with case-insensitive matching and handle truncated headers
                // TODO: WHY the first message is captured as `ontent-length`
                if key.eq_ignore_ascii_case("content-length") || key.eq_ignore_ascii_case("ontent-length") {
                    match value.parse::<usize>() {
                        Ok(len) => {
                            content_length = Some(len);
                            break;
                        }
                        Err(e) => {
                            error!("Failed to parse Content-Length '{}': {}", value, e);
                            return None;
                        }
                    }
                } else if !key.is_empty() {
                    warn!("Wrong content length header: {}", key);
                    return None;
                }
            } else {
                debug!("Header line without colon: '{}'", line);
            }
        }

        content_length
    }

    fn extract_body(&self, header_end: usize, content_length: usize) -> Option<(String, usize)> {
        let message_start = header_end;
        let message_end = header_end + content_length;

        if self.buffer.len() < message_end {
            return None;
        }

        let message = String::from_utf8(self.buffer[message_start..message_end].to_vec()).ok()?;

        Some((message, message_end))
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

#[cfg(test)]
mod tests {
    use super::*;

    struct MockReader(String);

    impl AsyncRead for MockReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            buf.put_slice(self.0.as_bytes());
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn test_read() {
        let lsp_message = r#"Content-Length: 119

{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Pyright language server 1.1.399 starting"}}Content-Length: 193

{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Server root directory: file:///Users/richard/.pyenv/versions/3.13.1/lib/python3.13/site-packages/pyright/dist/dist"}}"#;

        let lsp_message = lsp_message.replace("\n\n", "\r\n\r\n");

        let reader = MockReader(lsp_message.to_string());
        let mut frame_reader = LspFramedReader::new(reader);

        let msg = frame_reader.read_messages().await;

        let msg = match msg {
            Ok(msg) => msg,
            Err(e) => panic!("{}", e),
        };

        assert!(msg.is_some());
        let msgs = msg.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(
            msgs[0],
            r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Pyright language server 1.1.399 starting"}}"#
        );
        assert_eq!(
            msgs[1],
            r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Server root directory: file:///Users/richard/.pyenv/versions/3.13.1/lib/python3.13/site-packages/pyright/dist/dist"}}"#
        );
    }
}
