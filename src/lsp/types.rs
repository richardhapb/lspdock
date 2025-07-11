use bollard::container::LogOutput;
use bollard::errors::Error as BollardError;
use futures_core::stream::Stream;
use std::io::ErrorKind;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{error::Error, fmt::Display};
use tokio::io::AsyncRead;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum LspMessage {
    Request {
        jsonrpc: String,
        id: i32,
        method: String,
        params: serde_json::Value,
    },
    Response {
        jsonrpc: String,
        id: i32,
        result: Option<serde_json::Value>,
        error: Option<serde_json::Value>,
    },
    Notification {
        jsonrpc: String,
        method: String,
        params: serde_json::Value,
    },
}

#[derive(Debug)]
pub enum Pair {
    Server,
    Client,
}

#[derive(Debug)]
pub struct MessageError;

impl Display for MessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Unexpected message")
    }
}

impl Error for MessageError {}

impl MessageError {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self
    }

    #[allow(dead_code)]
    pub fn boxed() -> Box<dyn Error + Send + Sync + 'static> {
        Box::new(Self)
    }

    pub fn err<T>() -> Result<T, Box<dyn Error + Send + Sync + 'static>> {
        Err(Box::new(Self))
    }
}

pub struct DockerStreamReader {
    stream: Pin<Box<dyn Stream<Item = Result<LogOutput, BollardError>> + Send>>,
    buffer: Vec<u8>,
    position: usize,
}

impl DockerStreamReader {
    pub fn new(
        stream: Pin<Box<dyn Stream<Item = Result<LogOutput, BollardError>> + Send>>,
    ) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            position: 0,
        }
    }
}

impl AsyncRead for DockerStreamReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // If we have data in the buffer, return it
        if self.position < self.buffer.len() {
            let bytes_to_copy = std::cmp::min(buf.remaining(), self.buffer.len() - self.position);
            buf.put_slice(&self.buffer[self.position..self.position + bytes_to_copy]);
            self.position += bytes_to_copy;
            return Poll::Ready(Ok(()));
        }

        // Otherwise poll the stream for more data
        match self.stream.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(output))) => {
                // Get bytes from LogOutput and reset buffer
                self.buffer = output.into_bytes().to_vec();
                self.position = 0;

                // Try to read from the new buffer
                if !self.buffer.is_empty() {
                    let bytes_to_copy = std::cmp::min(buf.remaining(), self.buffer.len());
                    buf.put_slice(&self.buffer[..bytes_to_copy]);
                    self.position = bytes_to_copy;
                }

                Poll::Ready(Ok(()))
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Err(std::io::Error::new(ErrorKind::Other, e.to_string()))),
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }
}
