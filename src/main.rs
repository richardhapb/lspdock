use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, error, info, warn};
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

    info!("Initializing LSP");

    let mut using_docker = config.use_docker;

    // Check if Docker container exists before trying to use it
    if using_docker {
        let container_check = Command::new("docker")
            .args(["inspect", "-f", "{{.State.Running}}", &config.container])
            .output();

        match container_check.await {
            Ok(output) if output.status.success() => {
                let running = String::from_utf8_lossy(&output.stdout).trim() == "true";
                if !running {
                    warn!(container=%config.container, "Container exists but is not running, falling back to local");
                    using_docker = false;
                } else {
                    debug!(container=%config.container, "Container is running");
                }
            }
            Ok(_) => {
                warn!(container=%config.container, "Container not found, falling back to local");
                using_docker = false;
            }
            Err(e) => {
                warn!(%e, "Failed to check Docker, falling back to local");
                using_docker = false;
            }
        }
    }

    let (cmd, cmd_args) = if using_docker {
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
        (get_fallback_exec(&config), vec![])
    };

    let mut final_args = cmd_args;
    final_args.extend(cli.args.clone());

    debug!(?cmd, ?final_args, "Spawning LSP");

    let mut child = Command::new(&cmd)
        .args(&final_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn LSP process");

    let stdout = BufReader::new(child.stdout.take().unwrap());
    let stdin = BufWriter::new(child.stdin.take().unwrap());

    if using_docker {
        info!(%config.container, "Attached to stdout/stdin");
    } else {
        info!(%config.executable, "Attached to stdout/stdin (local)");
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
