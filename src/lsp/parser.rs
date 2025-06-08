use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use url::Url;

use tracing::{debug, error, info, trace};

use super::types::Pair;

pub(crate) async fn read_message<M>(
    reader: &mut BufReader<impl AsyncReadExt + Unpin>,
) -> Result<Option<M>, Box<dyn std::error::Error + Send + Sync>>
where
    M: for<'a> Deserialize<'a>,
{
    let mut headers = HashMap::new();
    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line).await?;

        trace!(%bytes_read, "Read line");

        if bytes_read == 0 {
            trace!(%bytes_read, "Breaking");
            break;
        }

        let trimmed = line.trim();
        trace!(%trimmed, "Reading line");

        if trimmed.is_empty() {
            trace!("Empty line detected");
            break; // End of headers
        }

        if let Some((key, value)) = trimmed.split_once(":") {
            let key = key.trim();
            let value = value.trim();

            trace!(%key, %value, "Header");

            headers.insert(key.to_lowercase(), value.to_lowercase());

            if key.eq_ignore_ascii_case("content-length")
                || key.eq_ignore_ascii_case("ontent-length")
            {
                content_length = Some(value.parse()?);
            }
        }
    }

    if content_length.is_none() {
        trace!("Content-Length header not found, returning None");
        return Ok(None);
    }

    let length = content_length.ok_or("Content-Length header not found")?;
    info!("Content-Length: {}", length);

    let mut buf = vec![0; length];

    reader.read_exact(&mut buf).await?;
    trace!("RAW: {}", String::from_utf8_lossy(&buf));
    let msg = serde_json::from_slice(&buf)?;

    Ok(msg)
}

pub(crate) async fn send_message<M>(
    writer: &mut tokio::io::BufWriter<impl tokio::io::AsyncWriteExt + Unpin>,
    message: M,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    M: Serialize,
{
    let msg = serde_json::to_string(&message)?;
    let msg = format!("Content-Length: {}\r\n\r\n{}", msg.len(), msg);

    writer.write_all(msg.as_bytes()).await?;
    writer.flush().await?;

    Ok(())
}
const HOST_WORKSPACE_ROOT: &str = "/Users/richard/dev/ddirt/debug/app";
const CONTAINER_WORKSPACE_ROOT: &str = "/usr/src/app";

pub(crate) fn redirect_uri(
    mut value: Value,
    from: &Pair,
    to: &Pair,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let from_path_str: &str;
    let to_path_str: &str;

    match from {
        Pair::Client => {
            from_path_str = HOST_WORKSPACE_ROOT;
            to_path_str = CONTAINER_WORKSPACE_ROOT;
        }
        Pair::Server => {
            from_path_str = CONTAINER_WORKSPACE_ROOT;
            to_path_str = HOST_WORKSPACE_ROOT;
        }
    }

    if let Some(uri_str) = value.get_mut("uri").and_then(|u| u.as_str()) {
        trace!(%uri_str, "Received URI");
        match Url::parse(uri_str) {
            Ok(url) => {
                let scheme = url.scheme();
                trace!(%scheme, "Detected scheme");
                if scheme == "file" {
                    if let Some(path) = url.to_file_path().ok() {
                        trace!(?path, "Transformed");
                        let from_path = Path::new(from_path_str);
                        let to_path = Path::new(to_path_str);

                        if let Ok(relative_path) = path.strip_prefix(from_path) {
                            trace!(?relative_path, "Prefix stripped");

                            let new_path = to_path.join(relative_path);
                            trace!(?new_path, "Joined path");

                            let new_uri = Url::from_file_path(&new_path)
                                .map_err(|_| {
                                    format!("Failed to convert path to URI: {:?}", new_path)
                                })?
                                .to_string();

                            debug!("Redirected URI: {} -> {}", uri_str, new_uri);
                            *value.get_mut("uri").unwrap() = Value::String(new_uri);
                        } else {
                            // If the path doesn't start with the host_root, it might be an external file
                            // or already a container path if this is a response from LSP to IDE.
                            debug!("URI not within host workspace root: {}", uri_str);
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to parse URI {}: {}", uri_str, e);
            }
        }
    }

    // Recursively search for "uri" fields in nested JSON objects/arrays
    if let Some(obj) = value.as_object_mut() {
        trace!(?obj, "Searching nested uris in object");
        for (_, v) in obj.iter_mut() {
            trace!(?v, "Nested");
            *v = redirect_uri(v.take(), from, to)?;
        }
    } else if let Some(arr) = value.as_array_mut() {
        trace!(?arr, "Searching nested uris in array");
        for v in arr.iter_mut() {
            trace!(?v, "Nested");
            *v = redirect_uri(v.take(), from, to)?;
        }
    }

    Ok(value)
}

pub(crate) async fn direct_forwarding(
    reader: &mut BufReader<impl AsyncReadExt + Unpin>,
    writer: &mut BufWriter<impl AsyncWriteExt + Unpin>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await?;
    writer.write_all(&buf).await?;
    writer.flush().await?;

    Ok(())
}
