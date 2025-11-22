use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
mod config;
mod lsp;
mod proxy;

use tokio::io::{BufReader, BufWriter};

use config::ProxyConfigToml;
use proxy::forward_proxy;

use crate::config::{Cli, ProxyConfig, resolve_config_path};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut cli: Cli = Cli::parse();
    let config_path = resolve_config_path();
    let config = ProxyConfigToml::from_file(config_path.as_ref(), &mut cli).map_err(|e| {
        eprintln!("Error retrieving config: {e}");
        e
    })?;

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
        .with(tracing_subscriber::EnvFilter::new(config.log_level.clone()))
        .with(tracing_subscriber::fmt::layer().with_writer(file_path))
        .init();

    debug!(?config, "configuration file");

    // Call docker only if the pattern matches.
    let (cmd, mut cmd_args) = if config.use_docker {
        let cmd = vec![
            "exec".into(),
            "-i".into(),
            "--workdir".into(),
            config.docker_internal_path.clone(),
            config.container.clone(),
            config.executable.clone(),
        ];
        ("docker".into(), cmd)
    } else {
        (config.executable.clone(), vec![])
    };

    debug!(%config.container, ?cmd_args, "Connecting to LSP");
    debug!(?cli.args, "args received");
    cmd_args.extend(cli.args.clone());
    debug!(?cmd_args, "full command");

    info!("Initializing LSP");

    let mut using_docker = config.use_docker;
    let mut child = match Command::new(&cmd)
        .args(cmd_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(ch) => ch,
        Err(_) if config.use_docker => {
            // Fallback to local lsp
            let exec = get_fallback_exec(&config);
            using_docker = false;

            info!("Cannot connect to container, falling back to local");

            // Last try, panic if cannot initialize
            Command::new(&exec)
                .args(cli.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("failed to initialize in Docker and local")
        }

        Err(err) => {
            error!(%err, "initializing lsp");
            std::process::exit(1);
        }
    };

    let stdout = BufReader::new(child.stdout.take().unwrap());
    let stdin = BufWriter::new(child.stdin.take().unwrap());

    if using_docker {
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

#[cfg(unix)]
fn get_fallback_exec(config: &ProxyConfig) -> String {
    config.executable.clone()
}

#[cfg(windows)]
fn get_fallback_exec(config: &ProxyConfig) -> String {
    format!("{}.exe", config.executable.clone())
}
