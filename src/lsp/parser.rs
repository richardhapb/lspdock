use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};

use tracing::{info, trace, debug};

use crate::proxy::config::ProxyConfig;

use super::types::Pair;

pub(crate) async fn read_message(
    reader: &mut BufReader<impl AsyncReadExt + Unpin>,
    pair: Pair,
    config: &ProxyConfig,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
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

        if trimmed.is_empty() && content_length.is_some() {
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
    let mut raw_str = String::from_utf8_lossy(&buf).to_string();

    trace!(?pair, "RAW: {raw_str}");

    match pair {
        Pair::Server => redirect_uri(&mut raw_str, &pair, config)?,
        Pair::Client => redirect_uri(&mut raw_str, &pair, config)?,
    };

    trace!("REDIRECTED: {raw_str}");

    Ok(Some(raw_str))
}

pub(crate) async fn send_message(
    writer: &mut tokio::io::BufWriter<impl tokio::io::AsyncWriteExt + Unpin>,
    message: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let len = message.len();
    debug!(%len, "Sending message");
    let msg = format!("Content-Length: {}\r\n\r\n{}", len, message);

    writer.write_all(msg.as_bytes()).await?;
    writer.flush().await?;

    Ok(())
}
pub(crate) fn redirect_uri(
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
