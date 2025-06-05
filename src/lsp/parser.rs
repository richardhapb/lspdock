use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use url::Url;

use tracing::{debug, error, info, trace};

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

        if bytes_read == 0 {
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            break; // End of headers
        }

        trace!(%trimmed, "Reading line");

        if let Some((key, value)) = trimmed.split_once(":") {
            let key = key.trim();
            let value = value.trim();

            headers.insert(key.to_lowercase(), value.to_lowercase());

            if key.eq_ignore_ascii_case("content-length") {
                content_length = Some(value.parse()?);
            }
        }
    }

    let length = content_length.ok_or("Content-Length header not found")?;
    info!("Content-Length: {}", length);

    let mut buf = vec![0; length];

    reader.read_exact(&mut buf).await?;
    info!("RAW: {}", String::from_utf8_lossy(&buf));
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
// Define your host and container base paths
const HOST_WORKSPACE_ROOT: &str = "/Users/richard/dev/ddirt/debug/app";
const CONTAINER_WORKSPACE_ROOT: &str = "/usr/src/app"; // Adjust this to your actual Docker mount point

pub(crate) fn redirect_uri(
    mut value: Value,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(uri_str) = value.get_mut("uri").and_then(|u| u.as_str()) {
        trace!(%uri_str, "Received URI");
        match Url::parse(uri_str) {
            Ok(url) => {
                let scheme = url.scheme();
                trace!(%scheme, "Detected scheme");
                if scheme == "file" {
                    if let Some(path) = url.to_file_path().ok() {
                        trace!(?path, "Transformed");
                        let host_root = Path::new(HOST_WORKSPACE_ROOT);
                        let container_root = Path::new(CONTAINER_WORKSPACE_ROOT);

                        if let Ok(relative_path) = path.strip_prefix(host_root) {
                            trace!(?relative_path, "Prefix stripped");

                            let new_path = container_root.join(relative_path);
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
        for (_, v) in obj.iter_mut() {
            *v = redirect_uri(v.take())?;
        }
    } else if let Some(arr) = value.as_array_mut() {
        for v in arr.iter_mut() {
            *v = redirect_uri(v.take())?;
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
