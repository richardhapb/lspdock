use std::time::Duration;
use tokio::signal::unix::{SignalKind, signal};

use crate::lsp::parser::{
    LspFramedReader, patch_initialize_process_id, redirect_uri, send_message,
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
    mut lsp_stdin: BufWriter<W>,
    lsp_stdout: BufReader<R>,
    config: ProxyConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    W: AsyncWrite + Unpin + Send + 'static,
    R: AsyncRead + Unpin + Send + 'static,
{
    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
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
    let ide_span = span!(Level::DEBUG, "IDE to Server");
    let ide_to_server = tokio::spawn(
        async move {
            let mut reader = LspFramedReader::new(stdin);
            let mut initialized = false;
            loop {
                tokio::select! {
                _ = ide_cancel.cancelled() => {
                    info!("IDE->SERVER task cancelled");
                    break;
                }

                    message = reader.read_message() => {
                        debug!("Read message from IDE");
                        match message {
                            Ok(Some(mut msg)) => {

                                debug!("Incoming message from IDE");
                                if !initialized {
                                    trace!("Trying to patch initialize method");
                                    initialized = patch_initialize_process_id(&mut msg);
                                    if !initialized {
                                        trace!("Initialize method not found, skipping patch");
                                    }
                                }

                                if client_config.use_docker {
                                    redirect_uri(&mut msg, &Pair::Client, &client_config)?;
                                }
                                send_message(&mut lsp_stdin, msg).await.map_err(|e| {
                                    error!("Failed to forward the request to IDE: {}", e);
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
        .instrument(ide_span),
    );

    // SERVER -> IDE
    let lsp_cancel = cancel.clone();
    let server_span = span!(Level::DEBUG, "Server to IDE");
    let server_to_ide = tokio::spawn(
        async move {
            let mut reader = LspFramedReader::new(lsp_stdout);
            loop {
                tokio::select! {
                    _ = lsp_cancel.cancelled() => {
                        info!("SERVER->IDE task cancelled");
                        break;
                    },
                        message =  reader.read_message() => {
                        debug!("Read message from LSP");
                        match message {
                            Ok(Some(mut msg)) => {
                                debug!("Incoming message from LSP");
                                if config.use_docker {
                                    redirect_uri(&mut msg, &Pair::Server, &config)?;
                                }
                                send_message(&mut stdout, msg).await.map_err(|e| {
                                    error!("Failed to forward the request to LSP: {}", e);
                                    e
                                })?;
                            }
                            Ok(None) => {
                                tokio::time::sleep(Duration::from_millis(30)).await;
                                trace!("Empty response received");
                                continue;
                            }
                            Err(_) => {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                                error!("Unrecognized Lsp Response");
                                break;
                            }
                        }
                    }
                }
            }
            Ok(())
        }
        .instrument(server_span),
    );

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
