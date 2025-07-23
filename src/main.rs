use std::process::Stdio;
use tokio::process::Command;
use tracing::{Level, debug, error, info, span};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
mod lsp;
mod proxy;

use tokio::io::{BufReader, BufWriter};

use proxy::{config::ProxyConfig, forward_proxy};

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

    let config = ProxyConfig::from_file("lsproxy.toml").map_err(|e| {
        error!("Error retrieving config: {e}");
        e
    })?;

    debug!(?config, "configuration file");

    let args = std::env::args();
    let mut cmd = vec![
        "exec".into(),
        "-i".into(),
        "--workdir".into(),
        config.docker_internal_path.clone(),
        config.container.clone(),
        config.executable.clone(),
    ];

    debug!(?args, "args received");
    cmd.extend(args.skip(1));
    debug!(?cmd, "full command");

    info!("Initializing LSP");

    debug!(%config.container, ?cmd, "Connecting to docker container");

    let mut child = Command::new("docker")
        .args(cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let stdout = BufReader::new(child.stdout.take().unwrap());
    let stdin = BufWriter::new(child.stdin.take().unwrap());

    info!(%config.container, "Attached to stdout/stdin");

    if let Err(e) = forward_proxy(stdin, stdout, config).await {
        error!("Connection error {e}");
    };

    Ok(())
}
