use memchr::memmem::find;
use std::error::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_util::bytes::{Buf, Bytes, BytesMut};
use tracing::{debug, trace};

pub struct LspFramedReader<R> {
    reader: BufReader<R>,
    buffer: BytesMut,
}

const MAX_CONTENT_LENGTH: usize = 16 * 1024 * 1024;

impl<R: AsyncRead + Unpin> LspFramedReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            reader: BufReader::new(inner),
            buffer: BytesMut::with_capacity(8192),
        }
    }

    /// Read messages from the sender and capture their content. Returns a [`Vec<String>`] with the
    /// messages or None if there are not messages.
    ///
    /// Reading the messages as a Finite State Machine model (FSM) (https://en.wikipedia.org/wiki/Finite-state_machine).
    /// When we are reading the header, the the state doesn't change until the header is completely read. Therefore,
    /// when reading the header, the state finishes and transitions to the reading body state, but does not
    /// transition back to the reading header state until the body has been read.
    ///
    /// ```text
    ///         +-------------read ends-------------+     +------------------+
    ///         |                                   |     |                  |
    ///         v                                   |     v                  |
    /// +-----------+                            +------------               |
    /// |           |------read ends------------>|           |               |
    /// |  Reading  |                            |  Reading  |-----reading---+
    /// |  Header   |-----reading--+             |   Body    |
    /// |           |              |             |           |
    /// +-----------+              |             +-----------+
    ///         ^                  |
    ///         |                  |
    ///         +------------------+
    /// ```
    pub async fn read_messages(
        &mut self,
    ) -> Result<Option<Vec<Bytes>>, Box<dyn Error + Send + Sync>> {
        let mut messages = Vec::new();

        loop {
            // parse all complete frames currently in buffer
            let mut made_progress = false;
            while let Some((message, advance)) = self.try_parse_message()? {
                self.buffer.advance(advance);
                messages.push(message);
                made_progress = true;
            }
            if made_progress {
                // return as a batch once we’ve produced something
                return Ok(Some(messages));
            }

            // need more bytes to complete the next frame
            let n = self.reader.read_buf(&mut self.buffer).await?;
            if n == 0 {
                // EOF
                if self.buffer.is_empty() {
                    return Ok(None); // clean end
                } else {
                    return Err("unexpected EOF while reading LSP message".into());
                }
            }
        }
    }

    /// Capture a message from the buffer.
    /// If the header, content-length, or body is None, return None and reset the FSM's state.
    fn try_parse_message(&self) -> Result<Option<(Bytes, usize)>, Box<dyn Error + Send + Sync>> {
        let header_end = match find(&self.buffer, b"\r\n\r\n") {
            Some(h) => h + 4,
            None => return Ok(None), // header incomplete
        };
        trace!(header_end);

        let headers = &self.buffer[..header_end];
        let content_length = self.extract_content_length(headers)?;
        if content_length > MAX_CONTENT_LENGTH {
            return Err(format!(
                "Content-Length {} exceeds limit {}",
                content_length, MAX_CONTENT_LENGTH
            )
            .into());
        }

        // body complete?
        let message_start = header_end;
        let message_end = match header_end.checked_add(content_length) {
            Some(x) => x,
            None => return Err("Content-Length overflow".into()),
        };
        if self.buffer.len() < message_end {
            return Ok(None); // need more bytes
        }

        // Zero-copy-ish: slice then to_string
        let body = self.buffer[message_start..message_end].to_vec();

        Ok(Some((Bytes::from(body), message_end)))
    }

    /// Extract the content length value from the header
    fn extract_content_length(
        &self,
        headers: &[u8],
    ) -> Result<usize, Box<dyn Error + Send + Sync>> {
        let s = std::str::from_utf8(headers)?; // no allocation
        for line in s.split("\r\n") {
            if let Some(colon) = line.find(':') {
                let (k, v) = (&line[..colon], &line[colon + 1..]);
                if k.trim().eq_ignore_ascii_case("content-length") {
                    return Ok(v.trim().parse::<usize>()?);
                }
                // ignore other headers like Content-Type
            }
        }
        Err("missing Content-Length header".into())
    }
}

/// Send a message from the proxy to the destination
pub async fn send_message(
    writer: &mut tokio::io::BufWriter<impl tokio::io::AsyncWriteExt + Unpin>,
    message: &Bytes,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let len = message.len();
    debug!(%len, "Sending message");
    trace!(?message);
    let msg = &[
        b"Content-Length: ",
        len.to_string().as_bytes(),
        b"\r\n\r\n",
        message,
    ]
    .concat();

    writer.write_all(msg).await?;
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
