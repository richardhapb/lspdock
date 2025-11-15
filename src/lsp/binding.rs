use crate::{config::ProxyConfig, proxy::Pair};
use memchr::memmem::{find, find_iter};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, str, sync::Arc};
use tokio::{
    fs::{File, create_dir_all},
    io::AsyncWriteExt,
    process::Command,
};
use tokio_util::bytes::Bytes;
use tracing::{debug, error, trace};

use std::collections::HashMap;
use tokio::sync::RwLock;

/// Redirect the paths from the sender pair to the receiver pair; this is used
/// for matching the paths between the container and the host path.
pub fn redirect_uri(
    raw_bytes: &mut Bytes,
    from: &Pair,
    config: &ProxyConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let from_path: &[u8];
    let to_path: &[u8];

    match from {
        Pair::Client => {
            from_path = match &config.encoded_local_path {
                Some(p) => p.as_bytes(),
                None => config.local_path.as_bytes(),
            };

            to_path = config.docker_internal_path.as_bytes();
        }
        Pair::Server => {
            from_path = config.docker_internal_path.as_bytes();
            to_path = match &config.encoded_local_path {
                Some(p) => p.as_bytes(),
                None => config.local_path.as_bytes(),
            };
        }
    }

    trace!(from=?String::from_utf8(from_path.to_vec()), to=?String::from_utf8(to_path.to_vec()));

    let occurrences = find_iter(raw_bytes, from_path);
    let from_n = from_path.len();
    let mut new_bytes: Bytes = Bytes::new();
    let mut last = 0;

    for occurr in occurrences {
        let before = &raw_bytes[last..occurr];
        last = occurr + from_n;
        // add the new text and join
        new_bytes = Bytes::from([&new_bytes, before, to_path].concat());
    }
    let after = &raw_bytes[last..];
    new_bytes = Bytes::from([&new_bytes, after].concat());

    *raw_bytes = new_bytes;

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

    async fn take_if_match(&self, id: u64) -> bool {
        let mut map = self.map.write().await;
        if map.get(&id).is_some() {
            map.remove(&id);
            return true;
        }
        false
    }

    pub async fn check_for_methods(
        &self,
        methods: &[&str],
        raw_bytes: &mut Bytes,
        pair: &Pair,
    ) -> std::io::Result<()> {
        // If the LSP is not in a container, there is no need to track this.
        if !self.config.use_docker {
            return Ok(());
        }

        match pair {
            Pair::Server => {
                // Early return
                if self.map.read().await.is_empty() {
                    trace!("Nothing expecting response, skipping method");
                    return Ok(());
                }

                let mut v: Value = serde_json::from_slice(raw_bytes.as_ref())?;
                trace!(server_response=%v, "received");

                // Check if this is a response to a tracked request
                if let Some(id) = v.get("id").and_then(Value::as_u64) {
                    let matches = self.take_if_match(id).await;
                    debug!(%matches);
                    if matches {
                        trace!(%id, "matches");
                        if let Some(results) = v.get_mut("result").and_then(Value::as_array_mut) {
                            trace!(?results);
                            for result in results {
                                if let Some(uri_val) = result.get("uri").and_then(|u| u.as_str())
                                    && !(uri_val.contains(&self.config.local_path))
                                {
                                    debug!(%uri_val);
                                    let new_uri = self.bind_library(uri_val).await?;
                                    debug!("file://{}", new_uri);

                                    Self::modify_uri(result, &new_uri);
                                }
                            }

                            *raw_bytes = Bytes::from(serde_json::to_vec(&v)?);
                        } else {
                            trace!("result content not found");
                        }
                    }
                }
            }

            Pair::Client => {
                // Early check to avoid parsing
                let mut method_found = "";
                for method in methods {
                    debug!("Checking for {method} method");
                    let expected = &[b"\"method\":\"", method.as_bytes(), b"\""].concat();
                    if find(raw_bytes, expected).is_some() {
                        method_found = method;
                        break;
                    }
                }

                if method_found.is_empty() {
                    debug!("Any method that required redirection was not found, skipping patch");
                    return Ok(());
                }

                debug!(%method_found);

                let v: Value = serde_json::from_slice(raw_bytes.as_ref())?;
                trace!(client_request=%v, "received");

                debug!("Checking for id");
                if let Some(id) = v.get("id").and_then(Value::as_u64) {
                    debug!(%id);
                    // Only track expected methods if URI matches
                    self.track(id, method_found).await;
                    debug!(%id, "Storing");
                }
            }
        }

        Ok(())
    }

    async fn bind_library(&self, uri: &str) -> std::io::Result<String> {
        let temp_dir = std::env::temp_dir().join("lspdock");
        trace!(temp_dir=%temp_dir.to_string_lossy());

        let safe_path = PathBuf::from(uri.strip_prefix("file://").unwrap_or(uri));
        let safe_path = safe_path.to_string_lossy();

        debug!(%safe_path);
        // If the file is in the temp dir used as a binding, means that the editor called to the LSP
        // method from that file, then we don't want to recalculate the path, use it directly instead
        let temp_uri = if safe_path.contains(&temp_dir.to_string_lossy().to_string()) {
            PathBuf::from(safe_path.to_string())
        } else {
            let relative_path = safe_path.strip_prefix("/").unwrap_or(&safe_path);
            trace!(%relative_path);
            let tmp_file_path = relative_path;
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
            self.copy_file(&safe_path, &temp_uri).await?;
        } else {
            debug!("File already exists, skipping copy. {}", temp_uri);
        }

        Ok(temp_uri)
    }

    /// Copies a file from either the local filesystem or a Docker container.
    async fn copy_file(&self, path: &str, destination: &str) -> std::io::Result<()> {
        // Only copy the file if the LSP is in a container
        debug!("Starting file copy from {} to {}", path, destination);
        let cmd = Command::new("docker")
            .args(["exec", &self.config.container, "cat", path])
            .stdout(Stdio::piped())
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn docker command");

        let status = cmd.wait_with_output().await?;

        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            error!("Command failed with status {}: {}", status.status, stderr);
            return Err(std::io::Error::other(format!("command failed: {}", stderr)));
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

#[allow(dead_code)] // Not used in Unix
pub fn encode_path(msg: &Bytes, config: &mut ProxyConfig) {
    config.encoded_local_path = if find(msg, b"%3A").is_some() {
        Some(config.local_path.replace(":", "%3A"))
    } else {
        Some(config.local_path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::parser::lsp_utils::{lspbody, lspmsg};

    fn construct_config() -> ProxyConfig {
        ProxyConfig {
            container: "test".into(),
            local_path: "/test/path".into(),
            docker_internal_path: "/usr/home/app".into(),
            executable: "somelsp".into(),
            pattern: Some("/test".into()),
            use_docker: true,
            log_level: None,
            patch_pid: None,
            encoded_local_path: None,
        }
    }

    #[test]
    fn redirect_single_uri() {
        let config = construct_config();
        let rq = lspmsg!("uri": "/test/path");
        let ex = lspmsg!("uri": "/usr/home/app");
        let mut request = Bytes::from(rq);
        let expected = Bytes::from(ex);

        redirect_uri(&mut request, &Pair::Client, &config).unwrap();

        assert_eq!(
            lspbody!(&expected => "bytes"),
            lspbody!(&request => "bytes")
        );
    }

    #[test]
    fn redirect_multiples_uris() {
        let config = construct_config();

        // From Client to Server

        let rq = lspmsg!("uri": "/test/path", "method": "text/document", "workspaceFolder": "/test/path");
        let ex = lspmsg!("uri": "/usr/home/app", "method": "text/document", "workspaceFolder": "/usr/home/app");

        let mut request = Bytes::from(rq.clone());
        let mut expected = Bytes::from(ex);

        redirect_uri(&mut request, &Pair::Client, &config).unwrap();

        assert_eq!(
            lspbody!(&expected => "bytes"),
            lspbody!(&request => "bytes")
        );

        // From Server to Client

        redirect_uri(&mut expected, &Pair::Server, &config).unwrap();

        let request = Bytes::from(rq);
        assert_eq!(
            lspbody!(&request => "bytes"),
            lspbody!(&expected => "bytes")
        );
    }
}

#[cfg(test)]
mod windows_tests {
    use tokio_util::bytes::Bytes;

    use super::*;
    use crate::{
        lsp::{
            binding::redirect_uri,
            parser::lsp_utils::{lspbody, lspmsg},
        },
        proxy::Pair,
    };

    #[test]
    fn windows_use_escaped_colon() {
        let config = ProxyConfig {
            container: "test".into(),
            local_path: "C:/Users/testUser/dev".into(),
            docker_internal_path: "/usr/home/app".into(),
            executable: "somelsp".into(),
            pattern: Some("/test".into()),
            use_docker: true,
            log_level: None,
            patch_pid: None,
            encoded_local_path: Some("/c%3A/Users/testUser/dev".into()),
        };

        let msg_with_sc = lspmsg!("uri": "/c%3A/Users/testUser/dev/somefile.rs");
        let msg_expected = lspmsg!("uri": "/usr/home/app/somefile.rs");
        let mut msg_bytes = Bytes::from(msg_with_sc);

        redirect_uri(&mut msg_bytes, &Pair::Client, &config).expect("Must be redirected");
        let new_msg = String::from_utf8_lossy(&msg_bytes);

        assert_eq!(lspbody!(new_msg), lspbody!(msg_expected));
    }

    #[test]
    fn windows_detect_colon_type() {
        let mut config = ProxyConfig {
            container: "test".into(),
            local_path: "/c:/Users/testUser/dev".into(),
            docker_internal_path: "/usr/home/app".into(),
            executable: "somelsp".into(),
            pattern: Some("/test".into()),
            use_docker: true,
            log_level: None,
            patch_pid: None,
            encoded_local_path: None,
        };

        // Encoded

        let msg_with_sc = lspmsg!("uri": "/c%3A/Users/testUser/dev/somefile.rs");
        let msg_bytes = Bytes::from(msg_with_sc);

        encode_path(&msg_bytes, &mut config);

        assert!(config.encoded_local_path.is_some());
        assert_eq!(
            config.encoded_local_path,
            Some("/c%3A/Users/testUser/dev".into())
        );

        // With raw colon

        let msg_with_sc = lspmsg!("uri": "/c:/Users/testUser/dev/somefile.rs");
        let msg_bytes = Bytes::from(msg_with_sc);

        encode_path(&msg_bytes, &mut config);

        assert!(config.encoded_local_path.is_some());
        assert_eq!(
            config.encoded_local_path,
            Some("/c:/Users/testUser/dev".into())
        );
    }
}
