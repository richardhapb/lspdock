use lsp::types::DockerStreamReader;
use tracing::{Level, error, span, trace};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
mod lsp;
mod proxy;

use tokio::io::{BufReader, BufWriter};

use bollard::Docker;
use bollard::exec::{CreateExecOptions, StartExecResults};
use proxy::{config::ProxyConfig, io::forward_proxy};

use std::default::Default;

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

    let config = CreateExecOptions {
        cmd: Some(vec!["pyright-langserver", "--stdio"]),
        attach_stdin: Some(true),
        attach_stdout: Some(true),
        ..Default::default()
    };

    let docker = Docker::connect_with_unix_defaults().unwrap();
    let exec = docker.create_exec("debug-web-1", config).await?;
    let stream = docker.start_exec(&exec.id, None).await?;

    match stream {
        StartExecResults::Attached { output, input } => {
            let config = ProxyConfig { timeout: 10 };

            let output_adapter = DockerStreamReader::new(output);

            if let Err(e) =
                forward_proxy(BufWriter::new(input), BufReader::new(output_adapter), config).await
            {
                error!("Connection error {e}");
            };

            Ok(())
        }
        StartExecResults::Detached => Ok(()),
    }
}
