use std::error::Error;
use std::time::Duration;
use tokio::signal::unix::{SignalKind, signal};

use crate::lsp::parser::{direct_forwarding, read_message, send_message};
use crate::lsp::types::Pair;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};
use tracing::{Level, debug, error, info, span, trace};

use super::config::ProxyConfig;

pub async fn forward_proxy<W, R>(
    mut lsp_stdin: BufWriter<W>,
    mut lsp_stdout: BufReader<R>,
    config: ProxyConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    W: AsyncWrite + Unpin + Send + 'static,
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::BufWriter::new(tokio::io::stdout());

    let cancel = tokio_util::sync::CancellationToken::new();
    let signal_cancel = cancel.clone();

    let signal_task = tokio::spawn(async move {
        shutdown_signal().await.ok();
        signal_cancel.cancel();
        Ok(())
    });

    info!("LSP Proxy: Lsp listening for incoming messages...");

    // IDE -> SERVER
    let ide_cancel = cancel.clone();
    let client_config = config.clone();
    let ide_to_server = tokio::spawn(async move {
        let span = span!(Level::DEBUG, "IDE to Server");
        let _guard = span.enter();
        loop {
            tokio::select! {
                _ = ide_cancel.cancelled() => {
                    info!("IDE->SERVER task cancelled");
                    break;
                }

                message = read_message(&mut stdin, Pair::Client, &client_config) => {
                    match message {
                        Ok(Some(msg)) => {
                            handle_ide_message(msg, &mut lsp_stdin).await?;
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
    });

    // SERVER -> IDE
    let lsp_cancel = cancel.clone();
    let server_to_ide = tokio::spawn(async move {
        let span = span!(Level::DEBUG, "Server to IDE");
        let _guard = span.enter();
        loop {
            tokio::select! {
                _ = lsp_cancel.cancelled() => {
                    info!("SERVER->IDE task cancelled");
                    break;
                },

                message =  read_message(&mut lsp_stdout, Pair::Server, &config) => {
                    match message {
                        Ok(Some(msg)) => {
                            handle_server_message(msg, &mut stdout).await?;
                        }
                        Ok(None) => {
                            tokio::time::sleep(Duration::from_millis(30)).await;
                            trace!("Empty response received");
                            continue;
                        }
                        Err(_) => {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                            error!("Unrecognized Lsp Response, forwarding direclty");
                            if let Err(e) = direct_forwarding(&mut lsp_stdout, &mut stdout).await {
                                error!("Failed to forward response to IDE: {}", e);
                                return Err(e);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    });

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

async fn handle_ide_message(
    msg: String,
    writer: &mut BufWriter<impl AsyncWriteExt + Unpin>,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let span = span!(parent: None, Level::DEBUG, "ClientHandler");
    let _guard = span.enter();

    debug!("Incoming message from IDE");

    send_message(writer, msg).await.map_err(|e| {
        error!("Failed to forward the request to IDE: {}", e);
        e
    })?;

    Ok(())
}

async fn handle_server_message(
    msg: String,
    writer: &mut BufWriter<impl AsyncWriteExt + Unpin>,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let span = span!(parent: None, Level::DEBUG, "ServerHandler");
    let _guard = span.enter();

    debug!("Incoming message from LSP");

    send_message(writer, msg).await.map_err(|e| {
        error!("Failed to forward the request to LSP server: {}", e);
        e
    })?;

    Ok(())
}

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

