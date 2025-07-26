use std::time::Duration;
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;

use crate::lsp::{
    binding::redirect_uri,
    parser::{LspFramedReader, send_message},
    pid::patch_initialize_process_id,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, BufReader, BufWriter};
use tracing::{Instrument, Level, debug, error, info, span, trace};

use crate::config::ProxyConfig;

#[derive(Debug)]
pub enum Pair {
    Server,
    Client,
}

/// Main handler for forwarding and transforming messages between IDE and LSP
pub async fn forward_proxy<W, R>(
    lsp_stdin: BufWriter<W>,
    lsp_stdout: BufReader<R>,
    config: ProxyConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    W: AsyncWrite + Unpin + Send + 'static,
    R: AsyncRead + Unpin + Send + 'static,
{
    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    let stdout = tokio::io::BufWriter::new(tokio::io::stdout());

    let cancel = CancellationToken::new();
    let signal_cancel = cancel.clone();

    let signal_task = tokio::spawn(async move {
        shutdown_signal().await.ok();
        signal_cancel.cancel();
        Ok(())
    });

    // The client writes to proxy stdin and proxy writes to LSP stdin
    let ide_to_server = main_loop(Pair::Client, &cancel, &config, stdin, lsp_stdin);
    // The LSP writes to stdout, and the proxy reads from it. The proxy also writes to its stdout
    // and the client reads from it
    let server_to_ide = main_loop(Pair::Server, &cancel, &config, lsp_stdout, stdout);

    info!("LSP Proxy: Lsp listening for incoming messages...");

    // This handles the concurrency for tasks, because if one of these tasks
    // when finished, we need to cancel any other task and end properly.
    let result = tokio::select! {
        r = ide_to_server => {
            info!("IDE->SERVER task completed");
            r?
        },
        r = server_to_ide => {
            info!("SERVER->IDE task completed");
            r?
        },
        r = signal_task => {
            info!("Signal handler task completed");
            r?
        },
    };

    // Cancel all remaining tasks
    cancel.cancel();

    info!("LSP proxy shutdown complete");

    result
}

/// Handle the main loop for reading and writing to and from the server/client
fn main_loop<W, R>(
    pair: Pair,
    cancel: &CancellationToken,
    config: &ProxyConfig,
    reader: BufReader<R>,
    mut writer: BufWriter<W>,
) -> tokio::task::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>
where
    W: AsyncWrite + Unpin + Send + 'static,
    R: AsyncRead + Unpin + Send + 'static,
{
    let cancel_clone = cancel.clone();
    let config_clone = config.clone();
    let span = match pair {
        Pair::Client => {
            span!(Level::DEBUG, "IDE to SERVER")
        }
        Pair::Server => {
            span!(Level::DEBUG, "SERVER to IDE")
        }
    };
    tokio::spawn(
        async move {
            let mut reader = LspFramedReader::new(reader);
            // The PID patch is required only on the client side for the `initialize` method.
            let mut require_pid_patch = matches!(pair, Pair::Client);
            loop {
                tokio::select! {
                    _ = cancel_clone.cancelled() => {
                        info!("task cancelled");
                        break;
                    }

                    message = reader.read_message() => {
                        debug!("The message has been read");
                        match message {
                            Ok(Some(mut msg)) => {
                                if require_pid_patch {
                                    trace!("Trying to patch initialize method");
                                    // The function returns true if the patch succeeds
                                    require_pid_patch = !patch_initialize_process_id(&mut msg);
                                }

                                if config_clone.use_docker {
                                    redirect_uri(&mut msg, &pair, &config_clone)?;
                                }
                                send_message(&mut writer, msg).await.map_err(|e| {
                                    error!("Failed to forward the request: {}", e);
                                    e
                                })?;
                            }
                            Ok(None) => {
                                tokio::time::sleep(Duration::from_millis(30)).await;
                                debug!("Empty request, connection closed");
                                break;
                            }
                            Err(e) => {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                                error!("Error reading message: {}", e);
                                return Err(e);
                            }
                        }
                    }
                }
            }
            Ok(())
        }
        .instrument(span),
    )
}

/// Handles the shutdown signal from the IDE
async fn shutdown_signal() -> Result<(), tokio::io::Error> {
    // Create signal streams
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    let mut hup = signal(SignalKind::hangup())?;

    // Wait for any signal
    tokio::select! {
        _ = term.recv() => info!("SIGTERM received"),
        _ = int.recv() => info!("SIGINT received"),
        _ = hup.recv() => info!("SIGHUP received"),
        _ = async {
            let mut buf = [0u8; 1];
            match tokio::io::stdin().read(&mut buf).await {
                Ok(0) | Err(_) => (), // EOF or error, either is fine
                _ => tokio::time::sleep(Duration::from_secs(3600*24*365)).await, // Wait forever if data received
            }
        } => info!("Stdin closed"),
    }

    debug!("Shutdown signal received");
    Ok(())
}
