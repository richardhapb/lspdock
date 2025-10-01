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
        eprintln!("Error retrieving config: {e}");
        e
    })?;

    let args = std::env::args();

    let mut lsp_args: Vec<String> = Vec::new();
    let mut exec_arg = config.executable.clone();
    let mut exec_arg_passed = false;

    for (i, extra_arg) in args.skip(1).enumerate() {
        if i == 0 && extra_arg == "--exec" {
            exec_arg_passed = true;
            continue;
        }

        if i == 1 && exec_arg_passed {
            exec_arg = extra_arg;
            // Set the executable if it is passed in the argument
            config.update_executable(exec_arg.clone());
            continue;
        }

        lsp_args.push(extra_arg);
    }

    let temp_path;

    // Initialize file logging instead of standard output/error
    #[cfg(unix)]
    {
        use std::path::PathBuf;
        temp_path = PathBuf::from("/tmp");
    }

    #[cfg(windows)]
    {
        temp_path = std::env::temp_dir();
    }

    let file = format!("lspdock_{}.log", config.executable);
    let file_path = std::fs::File::create(temp_path.join(&file))
        .map_err(|e| format!("Failed to create log file: {}", e))?;

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            config
                .log_level
                .clone()
                .unwrap_or_else(|| std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into())),
        ))
        .with(tracing_subscriber::fmt::layer().with_writer(file_path))
        .init();

    if exec_arg_passed {
        debug!("--exec argument received");
        debug!(exec=%config.executable, "Captured custom executable from argument");
    }
    debug!(?config, "configuration file");

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
