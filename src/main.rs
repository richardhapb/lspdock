use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
mod config;
mod lsp;
mod proxy;

use tokio::io::{BufReader, BufWriter};

use config::ProxyConfig;
use proxy::forward_proxy;

use crate::config::resolve_config_path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = resolve_config_path()?;
    let mut config = ProxyConfig::from_file(&config_path).map_err(|e| {
        error!("Error retrieving config: {e}");
        e
    })?;

    let file;

    // Initialize file logging instead of stdout/stderr
    #[cfg(unix)]
    {
        file = std::fs::File::create("/tmp/lspdock_trace.log")
            .map_err(|e| format!("Failed to create log file: {}", e))?;
    }

    #[cfg(windows)]
    {
        let log_file_path = std::env::temp_dir().join("lspdock_trace.log");
        file = std::fs::File::create(log_file_path)
            .map_err(|e| format!("Failed to create log file: {}", e))?;
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            config
                .log_level
                .clone()
                .unwrap_or_else(|| std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into())),
        ))
        .with(tracing_subscriber::fmt::layer().with_writer(file))
        .init();

    debug!(?config, "configuration file");

    let args = std::env::args();

    let mut lsp_args: Vec<String> = Vec::new();
    let mut exec_arg = config.executable.clone();
    let mut exec_arg_passed = false;

    for (i, extra_arg) in args.skip(1).enumerate() {
        if i == 0 && extra_arg == "--exec" {
            exec_arg_passed = true;
            debug!("--exec argument received");
            continue;
        }

        if i == 1 && exec_arg_passed {
            exec_arg = extra_arg;
            // Set the executable if it is passed in the argument
            config.update_executable(exec_arg.clone());
            debug!(%exec_arg, "Captured custom executable from argument");
            continue;
        }

        lsp_args.push(extra_arg);
    }

    // Call docker only if the pattern matches.
    let (cmd, mut cmd_args) = if config.use_docker {
        let cmd = vec![
            "exec".into(),
            "-i".into(),
            "--workdir".into(),
            config.docker_internal_path.clone(),
            config.container.clone(),
            exec_arg,
        ];
        ("docker".into(), cmd)
    } else {
        (config.executable.clone(), vec![])
    };

    debug!(%config.container, ?cmd_args, "Connecting to LSP");
    debug!(?lsp_args, "args received");
    cmd_args.extend(lsp_args);
    debug!(?cmd_args, "full command");

    info!("Initializing LSP");

    let mut child = Command::new(&cmd)
        .args(cmd_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let stdout = BufReader::new(child.stdout.take().unwrap());
    let stdin = BufWriter::new(child.stdin.take().unwrap());

    if config.use_docker {
        info!(%config.container, "Attached to stdout/stdin");
    } else {
        info!(%config.executable, "Attached to stdout/stdin");
    }

    // Main proxy handler
    if let Err(e) = forward_proxy(stdin, stdout, config).await {
        error!("Connection error {e}");
    };

    Ok(())
}
