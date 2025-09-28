use crate::{config::ProxyConfig, proxy::Pair};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, str, sync::Arc};
use tokio::{
    fs::{File, create_dir_all},
    io::AsyncWriteExt,
    process::Command,
};
use tracing::{debug, error, trace};

use std::collections::HashMap;
use tokio::sync::RwLock;

/// Redirect the paths from the sender pair to the receiver pair; this is used
/// for matching the paths between the container and the host path.
pub fn redirect_uri(
    raw_str: &mut String,
    from: &Pair,
    config: &ProxyConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let from_path_str: &str;
    let to_path_str: &str;

    match from {
        Pair::Client => {
            from_path_str = &config.local_path;
            to_path_str = &config.docker_internal_path;
        }
        Pair::Server => {
            from_path_str = &config.docker_internal_path;
            to_path_str = &config.local_path;
        }
    }

    trace!(%from_path_str, %to_path_str);

    *raw_str = raw_str.replace(from_path_str, to_path_str);

    Ok(())
}

// When calling the textDocument/definition method, if the library is external, a different path is used.
// We need to:
// 1. Identify if the method is "textDocument/definition", "textDocument/declaration", "textDocument/typeDefinition" (TODO: research if another method is required)
// 2. Capture the path.
// 3. Copy the file to a temporary file locally
// 4. Redirect the URI to the new temporary file.
// 5. Redirect all other communication requests between the IDE and server to keep LSP working as expected.
//
// 6?. Detect when the editor is back in the project to return to normal behavior.
//
// Response from server when textDocument/definition is called:
//
// IDE to Server: lspdock::lsp::parser: Sending message len=177
// IDE to Server: lspdock::lsp::parser: {"id":4,"method":"textDocument/definition","jsonrpc":"2.0","params":{"textDocument":{"uri":"file:///usr/src/app/dirtystroke/settings.py"},"position":{"character":13,"line":15}}}
// Server to IDE: lspdock::lsp::parser: Raw headers headers_str=Content-Length: 190
// Server to IDE: lspdock::lsp::parser: Processing header line: 'Content-Length: 190'
// Server to IDE: lspdock::lsp::parser: Header key: 'Content-Length', value: '190'
// Server to IDE: lspdock::lsp::parser: Reading body content_length=190
// Server to IDE: lspdock::lsp::parser: body={"jsonrpc":"2.0","id":4,"result":[{"uri":"file:///usr/local/lib/python3.12/site-packages/django/conf/__init__.py","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}}]}
//
// HERE WE NEED TO COPY THE FILE
//
// Server to IDE: lspdock::proxy::io: Read message from LSP
// Server to IDE: lspdock::proxy::io: Incoming message from LSP
// Server to IDE: lspdock::lsp::parser: Sending message len=190
//
// THEN SEND THE MODIFIED PATH TO TEMPORARY FILE
// Server to IDE: lspdock::lsp::parser: {"jsonrpc":"2.0","id":4,"result":[{"uri":"file:///usr/local/lib/python3.12/site-packages/django/conf/__init__.py","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}}]}
//
// LIKE
// Server to IDE: lspdock::lsp::parser:
// {"jsonrpc":"2.0","id":4,"result":[{"uri":"file:///tmp/lspdock/django/conf/__init__.py","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}}]}
//
// THE CLIENT INIT AGAIN THE PROXY FOR ANOTHER ENVIRONMENT??
//
// lspdock: Connecting to LSP config.container=development-web-1 cmd_args=["exec", "-i", "--workdir", "/usr/src/app", "development-web-1", "pyright-langserver"]
// lspdock: args received args=Args { inner: ["/Users/richard/proj/lspdock/target/debug/lsproxy", "--stdio"] }
// lspdock: full command cmd_args=["exec", "-i", "--workdir", "/usr/src/app", "development-web-1", "pyright-lan

/// Track the definition method related requests for interchanging URIs and handling different requests.
/// Cloning is cheap, O(1).
#[derive(Clone)]
pub struct RequestTracker {
    map: Arc<RwLock<HashMap<u64, String>>>,
    config: Arc<ProxyConfig>,
}

impl RequestTracker {
    pub fn new(config: ProxyConfig) -> Self {
        Self {
            map: Arc::new(RwLock::new(HashMap::new())),
            config: Arc::new(config),
        }
    }

    async fn track(&self, id: u64, method: &str) {
        self.map.write().await.insert(id, method.to_string());
    }

    async fn take_if_match(&self, id: u64, expected: &str) -> bool {
        let mut map = self.map.write().await;
        let exists = map.get(&id).map(|m| m == expected).unwrap_or(false);
        if exists {
            map.remove(&id);
        }
        exists
    }

    pub async fn check_for_methods(
        &self,
        methods: &[&str],
        raw_str: &mut String,
        pair: &Pair,
    ) -> std::io::Result<()> {
        // If the LSP is not in a container, there is no need to track this.
        if !self.config.use_docker {
            return Ok(());
        }

        //textDocument/declaration

        match pair {
            Pair::Server => {
                let mut v: Value = serde_json::from_str(&raw_str)?;

                // Check if this is a response to a tracked request
                if let Some(id) = v.get("id").and_then(Value::as_u64) {
                    for method in methods {
                        debug!("Checking for {method} method");

                        let matches = self.take_if_match(id, *method).await;
                        debug!(%matches);
                        if matches {
                            trace!(%id, "matches");
                            if let Some(results) = v.get_mut("result").and_then(Value::as_array_mut)
                            {
                                trace!(?results);
                                for result in results {
                                    if let Some(uri_val) =
                                        result.get("uri").and_then(|u| u.as_str())
                                    {
                                        if !(uri_val.contains(&self.config.local_path)) {
                                            debug!(%uri_val);
                                            let new_uri =
                                                self.bind_library(uri_val.to_string()).await?;
                                            debug!("file://{}", new_uri);

                                            Self::modify_uri(result, &new_uri);
                                        }
                                    }
                                }
                                *raw_str = v.to_string(); // write back the modified JSON
                            }
                        }
                    }
                }
            }

            Pair::Client => {
                let v: Value = serde_json::from_str(&raw_str)?;

                debug!("Checking for id");
                if let Some(id) = v.get("id").and_then(Value::as_u64) {
                    debug!(%id);

                    if let Some(req_method) = v.get("method").and_then(Value::as_str) {
                        trace!(%req_method);
                        // Only track expected methods if URI matches
                        for method in methods {
                            if req_method == *method {
                                debug!(%id, "Storing");
                                self.track(id, *method).await;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn bind_library(&self, uri: String) -> std::io::Result<String> {
        let temp_dir = std::env::temp_dir().join("lspdock");
        trace!(temp_dir=%temp_dir.to_string_lossy());

        let safe_path = PathBuf::from(uri.strip_prefix("file://").unwrap_or(&uri));
        let safe_path = safe_path.to_string_lossy();

        debug!(%safe_path);
        // If the file is in the temp dir used as a binding, means that the editor called to the LSP
        // method from that file, then we don't want to recalculate the path, use it directly instead
        let temp_uri = if safe_path.contains(&temp_dir.to_string_lossy().to_string()) {
            PathBuf::from(safe_path.to_string())
        } else {
            let relative_path = safe_path.strip_prefix("/").unwrap_or(&safe_path);
            trace!(%relative_path);
            let tmp_file_path = relative_path.to_string();
            temp_dir.join(tmp_file_path)
        };

        // Create the directories if they do not exist
        if let Some(parent) = temp_uri.parent() {
            trace!(dir=%parent.to_string_lossy(), "creating directories");
            create_dir_all(parent).await?;
        }

        let temp_uri = temp_uri.to_string_lossy().to_string();
        trace!(%temp_uri);

        let temp_uri_path = PathBuf::from(&temp_uri);
        debug!(%temp_uri);
        if !temp_uri_path.exists() {
            self.copy_file(safe_path.to_string(), &temp_uri).await?;
        } else {
            debug!("File already exists, skipping copy. {}", temp_uri);
        }

        Ok(temp_uri)
    }

    /// Copies a file from either the local filesystem or a Docker container.
    async fn copy_file(&self, path: String, destination: &str) -> std::io::Result<()> {
        // Only copy the file if the LSP is in a container
        debug!("Starting file copy from {} to {}", path, destination);
        let cmd = Command::new("docker")
            .args(&["exec", &self.config.container, "cat", &path])
            .stdout(Stdio::piped())
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn docker command");

        let status = cmd.wait_with_output().await?;

        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            error!("Command failed with status {}: {}", status.status, stderr);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("command failed: {}", stderr),
            ));
        }

        let mut file = File::create(destination).await?;
        file.write_all(&status.stdout).await?;

        debug!(
            "Successfully wrote {} bytes to {}",
            status.stdout.len(),
            destination
        );
        Ok(())
    }

    fn modify_uri(result: &mut Value, new_uri: &str) {
        if let Some(uri) = result.get_mut("uri") {
            *uri = json!(format!("file://{}", new_uri));
        };
    }
}
