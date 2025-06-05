use std::process::Stdio;
use tokio::process::Command;
use tracing::{Level, error, span, trace};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
mod lsp;
mod proxy;

use tokio::io::AsyncReadExt;

use proxy::io::{forward_proxy, lsp_stdio};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize file logging instead of stdout/stderr
    let file = std::fs::File::create("/tmp/lsproxy_trace.log")
        .map_err(|e| format!("Failed to create log file: {}", e))?;

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer().with_writer(file))
        .init();

    let main_span = span!(Level::TRACE, "LSPROXY");
    let _guard = main_span.enter();

    trace!("Initializing LSP");

    let mut child = Command::new("docker")
        .args(&["exec", "-i", "debug-web-1", "pyright-langserver", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut lsp_stderr_raw = child.stderr.take().expect("Cannot take LSP server stderr");

    // Task to read stderr from the LSP server
    tokio::spawn(async move {
        let mut stderr_buf = Vec::new();
        match lsp_stderr_raw.read_to_end(&mut stderr_buf).await {
            Ok(_) => {
                let stderr_str = String::from_utf8_lossy(&stderr_buf);
                if !stderr_str.is_empty() {
                    error!("LSP server STDERR: {}", stderr_str);
                }
            }
            Err(e) => {
                error!("Error reading LSP server stderr: {}", e);
            }
        }
    });

    let (stdio, stdout) = lsp_stdio(child).await?;

    if let Err(e) = forward_proxy(stdio, stdout).await {
        error!("Connection error {e}");
    }

    Ok(())
}
