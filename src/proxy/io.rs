use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::lsp::{
    binding::{RequestTracker, redirect_uri},
    parser::{LspFramedReader, send_message},
    pid::PidHandler,
};
use tokio::io::{AsyncRead, AsyncWrite, BufReader, BufWriter};
use tracing::{Instrument, Level, debug, error, info, span, trace};

use crate::config::ProxyConfig;

const GOTO_METHODS: &[&str] = &["textDocument/definition", "textDocument/declaration", "textDocument/typeDefinition"];
// This prevents an infinite loop if the LSP or the IDE doesn't respond correctly
const MAX_EMPTY_RESPONSES_THRESHOLD: usize = 15;

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

    let tracker = RequestTracker::new(config.clone());

    // The client writes to proxy stdin and proxy writes to LSP stdin
    let ide_to_server = main_loop(
        Pair::Client,
        &cancel,
        &config,
        stdin,
        lsp_stdin,
        tracker.clone(),
    );
    // The LSP writes to stdout, and the proxy reads from it. The proxy also writes to its stdout
    // and the client reads from it
    let server_to_ide = main_loop(Pair::Server, &cancel, &config, lsp_stdout, stdout, tracker);

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
    tracker: RequestTracker,
) -> tokio::task::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>
where
    W: AsyncWrite + Unpin + Send + 'static,
    R: AsyncRead + Unpin + Send + 'static,
{
    let cancel_monitor = cancel.clone();
    let config_clone = config.clone();
    let tracker_inner = tracker.clone();
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
            let mut empty_counter = 0;
            let cancel_provider = cancel_monitor.clone();
            // The PID patch is required only on the client side for the `initialize` method.
            let mut pid_handler: Option<PidHandler> = match pair {
                Pair::Server => None,
                Pair::Client => Some(PidHandler::new(cancel_provider)),
            };
            loop {
                tokio::select! {
                    _ = cancel_monitor.cancelled() => {
                        info!("task cancelled");
                        break;
                    }

                    messages = reader.read_messages() => {
                        debug!("Messages has been read");
                        match messages {
                            Ok(Some(msgs)) => {
                                trace!(msgs_len=msgs.len());
                                trace!(?msgs);

                                // If the messages are empty, increase the counter
                                empty_counter = if msgs.is_empty() {
                                    empty_counter + 1
                                } else {
                                    0
                                };

                                if empty_counter >= MAX_EMPTY_RESPONSES_THRESHOLD {
                                    info!("The empty response has reached the threshold; exiting");
                                    break;
                                }

                                for mut msg in msgs {
                                    if config_clone.patch_pid.unwrap_or(false) {
                                        if let Some(ref mut pid_handler_ref) = pid_handler {
                                            trace!("Trying to take the PID from the initialize method");
                                            // The function returns true if the take succeeds
                                            if pid_handler_ref.try_take_initialize_process_id(&mut msg)? {
                                                debug!("The PID has been captured from the initialize method, setting pid_handler to None");
                                                // Set the pid_handler to None to avoid attempting to patch the PID again
                                                if let Some(pid_handler) = pid_handler.take() {
                                                    tokio::spawn(async move { pid_handler.monitor_pid().await });
                                                }
                                            }
                                        }
                                    }

                                    if config_clone.use_docker {
                                        redirect_uri(&mut msg, &pair, &config_clone)?;
                                        tracker_inner.check_for_methods(GOTO_METHODS, &mut msg, &pair).await?;
                                    }
                                    send_message(&mut writer, msg).await.map_err(|e| {
                                        error!("Failed to forward the request: {}", e);
                                        e
                                    })?;
                                }
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
#[cfg(unix)]
async fn shutdown_signal() -> Result<(), tokio::io::Error> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    let mut hup = signal(SignalKind::hangup())?;

    tokio::select! {
        _ = term.recv() => tracing::info!("SIGTERM received"),
        _ = int.recv() => tracing::info!("SIGINT received"),
        _ = hup.recv() => tracing::info!("SIGHUP received"),
    }

    debug!("Shutdown signal received");
    Ok(())
}

#[cfg(windows)]
async fn shutdown_signal() -> Result<(), tokio::io::Error> {
    tokio::signal::ctrl_c().await?;
    tracing::info!("Ctrl+C received");
    Ok(())
}
