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

    let main_span = span!(Level::DEBUG, "LSPROXY");
    let _guard = main_span.enter();

    // TODO: Read this from a file, env or CLI
    let config = ProxyConfig {
        timeout: 10,
        container: "debug-web-1".into(),
        cmd: vec!["pyright-langserver".into(), "--stdio".into()],
    };

    trace!("Initializing LSP");

    let exec_config = CreateExecOptions {
        cmd: Some(config.cmd.clone()),
        attach_stdin: Some(true),
        attach_stdout: Some(true),
        attach_stderr: Some(true),
        working_dir: Some("/usr/src/app".into()),
        ..Default::default()
    };

    trace!(%config.container, ?config.cmd, "Connecting to docker container");
    let docker = Docker::connect_with_socket_defaults().expect("Error connecting to docker");
    let exec = docker.create_exec(&config.container, exec_config).await?;
    let stream = docker.start_exec(&exec.id, None).await?;
    trace!(%config.container, "Connected sucessfully");

    match stream {
        StartExecResults::Attached { output, input } => {
            let output_reader = DockerStreamReader::new(output);
            trace!("Attached to stdout/stdin");

            if let Err(e) =
                forward_proxy(BufWriter::new(input), BufReader::new(output_reader), config).await
            {
                error!("Connection error {e}");
            };

            Ok(())
        }
        StartExecResults::Detached => {
            error!("Docker not attached");
            Err("Cannot attach to Docker")?
        }
    }
}
