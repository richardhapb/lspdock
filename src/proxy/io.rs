use std::error::Error;
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::lsp::parser::{direct_forwarding, read_message, redirect_uri, send_message};
use crate::lsp::types::LspMessage;
use tokio::io::{BufReader, BufWriter};
use tracing::{debug, error, info};

pub async fn forward_proxy(
    mut lsp_stdin: BufWriter<ChildStdin>,
    mut lsp_stdout: BufReader<ChildStdout>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut ide_stdout = tokio::io::BufReader::new(tokio::io::stdin());
    let mut ide_stdin = tokio::io::BufWriter::new(tokio::io::stdout());

    info!("LSP Proxy: Lsp listening for incoming messages...");

    // IDE -> SERVER
    let ide_to_server = tokio::spawn(async move {
        loop {
            match read_message(&mut ide_stdout).await {
                Ok(Some(msg)) => {
                    debug!(message = ?msg, "Incoming LSP message from IDE");

                    match msg {
                        LspMessage::Request { id, method, params } => {
                            debug!("Received params: {:?}", params);
                            let params = redirect_uri(params);
                            let params = match &params {
                                Ok(p) => p,
                                Err(e) => {
                                    error!("URI redirection error: {}", e);
                                    &params.unwrap()
                                }
                            };
                            debug!("Forwarded params: {:?}", params);
                            if let Err(e) = send_message(
                                &mut lsp_stdin,
                                LspMessage::Request {
                                    id,
                                    method,
                                    params: params.to_owned(),
                                },
                            )
                            .await
                            {
                                error!("Failed to forward the request to LSP server: {}", e);
                                continue;
                            }
                        }
                        _ => {
                            if let Err(e) = send_message(&mut lsp_stdin, msg).await {
                                error!("Failed to forward message to LSP server: {}", e);
                                continue;
                            }
                        }
                    };
                }
                Ok(None) => {
                    info!("End of input stream (EOF). Exiting proxy.");
                    break;
                }
                Err(_) => {
                    if let Err(e) = direct_forwarding(&mut ide_stdout, &mut lsp_stdin).await {
                        error!("Failed to forward message to LSP server: {}", e);
                        return Err(e);
                    }
                }
            }
        }
        Ok(())
    });

    // SERVER -> IDE
    let server_to_ide = tokio::spawn(async move {
        loop {
            match read_message(&mut lsp_stdout).await {
                Ok(Some(msg)) => {
                    debug!(message = ?msg, "Incoming LSP message from LSP server");

                    match msg {
                        LspMessage::Response {
                            id,
                            mut result,
                            error,
                        } => {
                            debug!("Received params: {:?}", result);
                            result = result.and_then(|r| match redirect_uri(r) {
                                Ok(value) => Some(value),
                                _ => None,
                            });
                            debug!("Forwarded params: {:?}", result);
                            if let Err(e) = send_message(
                                &mut ide_stdin,
                                LspMessage::Response { id, result, error },
                            )
                            .await
                            {
                                error!("Failed to forward the response to IDE: {}", e);
                                continue;
                            }
                        }
                        _ => {
                            if let Err(e) = send_message(&mut ide_stdin, msg).await {
                                error!("Failed to forward message to IDE: {}", e);
                                continue;
                            }
                        }
                    };
                }
                Ok(None) => {
                    info!("End of input stream (EOF). Exiting proxy.");
                    break;
                }
                Err(_) => {
                    if let Err(e) = direct_forwarding(&mut lsp_stdout, &mut ide_stdin).await {
                        error!("Failed to forward response to IDE: {}", e);
                        return Err(e);
                    }
                }
            }
        }

        Ok(())
    });

    let (j1, j2) = tokio::join!(ide_to_server, server_to_ide);

    j1??;
    j2??;

    Ok(())
}

pub async fn lsp_stdio(
    mut child: Child,
) -> Result<(BufWriter<ChildStdin>, BufReader<ChildStdout>), Box<dyn Error>> {
    Ok((
        BufWriter::new(child.stdin.take().expect("Cannot take command stdin")),
        BufReader::new(child.stdout.take().expect("Cannot take command stdout")),
    ))
}
