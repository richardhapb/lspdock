use crate::{config::ProxyConfig, proxy::Pair};
use memchr::memmem::{find, find_iter};
use serde_json::{Value, json};
use std::{future::Future, path::PathBuf, pin::Pin, process::Stdio, sync::Arc};
use tokio::{
    fs::{File, create_dir_all},
    io::AsyncWriteExt,
    process::Command,
};
use tokio_util::bytes::Bytes;
use tracing::{debug, error, trace};

use std::collections::HashMap;
use tokio::sync::RwLock;

/// Redirect the paths from the sender pair to the receiver pair
pub fn redirect_uri(
    raw_bytes: &mut Bytes,
    from: &Pair,
    config: &ProxyConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (from_path, to_path): (&[u8], &[u8]) = match from {
        Pair::Client => (
            config.local_path.as_bytes(),
            config.docker_internal_path.as_bytes(),
        ),
        Pair::Server => (
            config.docker_internal_path.as_bytes(),
            config.local_path.as_bytes(),
        ),
    };

    trace!(from=?String::from_utf8_lossy(from_path), to=?String::from_utf8_lossy(to_path));

    let mut new_bytes = Vec::new();
    let mut last = 0;

    for pos in find_iter(raw_bytes, from_path) {
        new_bytes.extend_from_slice(&raw_bytes[last..pos]);
        new_bytes.extend_from_slice(to_path);
        last = pos + from_path.len();
    }
    new_bytes.extend_from_slice(&raw_bytes[last..]);

    *raw_bytes = Bytes::from(new_bytes);
    Ok(())
}

type ActionFn = for<'a> fn(
    &'a RequestTracker,
    &'a mut Value,
) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + 'a>>;

struct MethodHandler {
    methods: &'static [&'static str],
    action: ActionFn,
}

pub struct PluginRegistry {
    handlers: Vec<MethodHandler>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self { handlers: vec![] }
    }

    pub fn register(&mut self, methods: &'static [&'static str], action: ActionFn) {
        self.handlers.push(MethodHandler { methods, action });
    }

    async fn process(
        &self,
        tracker: &RequestTracker,
        v: &mut Value,
        tracked_method: &str,
    ) -> std::io::Result<()> {
        for handler in &self.handlers {
            if handler.methods.contains(&tracked_method) {
                (handler.action)(tracker, v).await?;
            }
        }
        Ok(())
    }
}

pub struct RequestTracker {
    map: Arc<RwLock<HashMap<u64, String>>>,
    plugins: Arc<PluginRegistry>,
    config: Arc<ProxyConfig>,
}

impl Clone for RequestTracker {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
            plugins: self.plugins.clone(),
            config: self.config.clone(),
        }
    }
}

impl RequestTracker {
    pub fn new(config: ProxyConfig, plugins: PluginRegistry) -> Self {
        Self {
            map: Arc::new(RwLock::new(HashMap::new())),
            plugins: Arc::new(plugins),
            config: Arc::new(config),
        }
    }

    async fn track(&self, id: u64, method: &str) {
        self.map.write().await.insert(id, method.to_string());
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
                if self.map.read().await.is_empty() {
                    trace!("Nothing expecting response, skipping");
                    return Ok(());
                }

                let mut v: Value = serde_json::from_slice(raw_bytes.as_ref())?;
                trace!(server_response=%v, "received");

                // Check if this is a response to a tracked request
                if let Some(id) = v.get("id").and_then(Value::as_u64) {
                    if let Some(tracked_method) = self.map.write().await.remove(&id) {
                        self.plugins.process(self, &mut v, &tracked_method).await?;
                        *raw_bytes = Bytes::from(serde_json::to_vec(&v)?);
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
                    return Ok(());
                }

                debug!(%method_found);

                let v: Value = serde_json::from_slice(raw_bytes.as_ref())?;
                trace!(client_request=%v, "received");

                debug!("Checking for id");
                if let Some(id) = v.get("id").and_then(Value::as_u64) {
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
            temp_dir.join(relative_path)
        };

        // Create the directories if they do not exist
        if let Some(parent) = temp_uri.parent() {
            trace!(dir=%parent.to_string_lossy(), "creating directories");
            create_dir_all(parent).await?;
        }

        let temp_uri = temp_uri.to_string_lossy().to_string();
        trace!(%temp_uri);

        if !PathBuf::from(&temp_uri).exists() {
            self.copy_file(&safe_path, &temp_uri).await?;
        } else {
            debug!("File already exists, skipping copy. {}", temp_uri);
        }

        Ok(temp_uri)
    }

    async fn copy_file(&self, path: &str, destination: &str) -> std::io::Result<()> {
        // Only copy the file if the LSP is in a container
        debug!("Starting file copy from {} to {}", path, destination);
        let output = Command::new("docker")
            .args(["exec", &self.config.container, "cat", path])
            .stdout(Stdio::piped())
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Command failed with status {}: {}", output.status, stderr);
            return Err(std::io::Error::other(format!("command failed: {}", stderr)));
        }

        let mut file = File::create(destination).await?;
        file.write_all(&output.stdout).await?;

        debug!(
            "Successfully wrote {} bytes to {}",
            output.stdout.len(),
            destination
        );
        Ok(())
    }

    fn modify_uri(result: &mut Value, new_uri: &str) {
        if let Some(uri) = result.get_mut("uri") {
            *uri = json!(format!("file://{}", new_uri));
        }
    }
}

// Plugin actions - return pinned futures with proper lifetime
pub fn redirect_goto_methods<'a>(
    tracker: &'a RequestTracker,
    v: &'a mut Value,
) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        if let Some(results) = v.get_mut("result").and_then(Value::as_array_mut) {
            for result in results {
                if let Some(uri_val) = result.get("uri").and_then(|u| u.as_str()) {
                    if !uri_val.contains(&tracker.config.local_path) {
                        let new_uri = tracker.bind_library(uri_val).await?;
                        RequestTracker::modify_uri(result, &new_uri);
                    }
                }
            }
        }
        Ok(())
    })
}

pub fn test_hover<'a>(
    _tracker: &'a RequestTracker,
    v: &'a mut Value,
) -> Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        if let Some(result) = v.get_mut("result").filter(|r| !r.is_null()) {
            if let Some(value) = result.get_mut("contents").and_then(|c| c.get_mut("value")) {
                if let Some(s) = value.as_str() {
                    *value = Value::String(s.replace("a", "e"));
                }
            }
        }
        Ok(())
    })
}

pub fn ensure_root(msg: &mut Bytes, config: &ProxyConfig) {
    let docker_uri = format!("file://{}", config.docker_internal_path);

    for key in [b"\"rootUri\":\"".as_slice(), b"\"rootPath\":\""] {
        if let Some(beg) = find(msg, key).map(|p| p + key.len()) {
            if let Some(end) = find(&msg[beg..], b"\"").map(|p| p + beg) {
                let before = &msg[..beg];
                let after = &msg[end..];
                *msg = Bytes::from([before, docker_uri.as_bytes(), after].concat());
            }
        }
    }

    let key = b"\"workspaceFolders\":[";
    if let Some(beg) = find(msg, key).map(|p| p + key.len()) {
        if let Some(end) = find(&msg[beg..], b"]").map(|p| p + beg) {
            if let Some(ws) = patch_workspace_folders(&msg[beg..end], &docker_uri) {
                let before = &msg[..beg];
                let after = &msg[end..];
                *msg = Bytes::from([before, &ws, after].concat());
            }
        }
    }
}

// pub fn ensure_root(msg: &mut Bytes, config: &ProxyConfig) {
//     let docker_uri = format!("file://{}", config.docker_internal_path);
//
//     // Patch rootUri
//     let key = b"\"rootUri\":\"";
//     if let Some(mut beg) = find(msg, key) {
//         beg += key.len();
//         if let Some(mut end) = find(&msg[beg..], b"\"") {
//             end += beg; // Make it absolute position
//             let before = &msg[..beg];
//             let after = &msg[end..];
//             *msg = Bytes::from([before, docker_uri.as_bytes(), after].concat());
//         }
//     }
//
//     // Patch rootPath if present
//     let key = b"\"rootPath\":\"";
//     if let Some(mut beg) = find(msg, key) {
//         beg += key.len();
//         if let Some(mut end) = find(&msg[beg..], b"\"") {
//             end += beg;
//             let before = &msg[..beg];
//             let after = &msg[end..];
//             *msg = Bytes::from([before, docker_uri.as_bytes(), after].concat());
//         }
//     }
//
//     let key = b"\"workspaceFolders\":[";
//     if let Some(mut beg) = find(msg, key) {
//         beg += key.len();
//         if let Some(mut end) = find(&msg[beg..], b"]") {
//             end += beg;
//             let before = &msg[..beg];
//             let after = &msg[end..];
//             if let Some(ws) = patch_workspace_folders(&msg[beg..end], &docker_uri) {
//                 *msg = Bytes::from([before, &ws, after].concat());
//             }
//         }
//     }
// }

fn patch_workspace_folders(msg: &[u8], docker_uri: &str) -> Option<Bytes> {
    let key = b"\"uri\":\"";
    let mut result = None;
    for uri_beg in find_iter(msg, key) {
        let beg = uri_beg + key.len();
        if let Some(end) = find(&msg[beg..], b"\"").map(|p| p + beg) {
            let before = &msg[..beg];
            let after = &msg[end..];
            result = Some(Bytes::from([before, docker_uri.as_bytes(), after].concat()));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::parser::lsp_utils::{lspbody, lspmsg};

    fn construct_config() -> ProxyConfig {
        ProxyConfig {
            local_path: "/test/path".into(),
            docker_internal_path: "/usr/home/app".into(),
            ..Default::default()
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

    #[test]
    fn ensure_root_patches_correctly() {
        let mut config = construct_config();
        config.local_path = "/test/path/app".to_string();

        let msg = lspmsg!(
            "method": "initialize",
            "rootUri": "file:///test/path",
            "rootPath": "/test/path"
        );

        let mut request = Bytes::from(msg);
        ensure_root(&mut request, &config);

        let body = lspbody!(&request => "string");
        assert!(find(body, b"\"rootUri\":\"file:///usr/home/app\"").is_some());
        assert!(find(body, b"\"rootPath\":\"file:///usr/home/app\"").is_some());
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
            local_path: "/c%3A/Users/testUser/dev".into(),
            docker_internal_path: "/usr/home/app".into(),
            ..Default::default()
        };

        let msg_with_sc = lspmsg!("uri": "/c%3A/Users/testUser/dev/somefile.rs");
        let msg_expected = lspmsg!("uri": "/usr/home/app/somefile.rs");
        let mut msg_bytes = Bytes::from(msg_with_sc);

        redirect_uri(&mut msg_bytes, &Pair::Client, &config).expect("Must be redirected");
        let new_msg = String::from_utf8_lossy(&msg_bytes);

        assert_eq!(lspbody!(new_msg), lspbody!(msg_expected));
    }
}
