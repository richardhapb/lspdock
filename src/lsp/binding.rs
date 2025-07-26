use crate::{config::ProxyConfig, proxy::Pair};
use std::{process::Stdio, str};
use tokio::{fs::{create_dir_all, File}, io::{AsyncReadExt, AsyncWriteExt, BufReader}, process::Command};
use tracing::trace;

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
// 1. Identify if the method is textDocument/definition (TODO: research if another method is required)
// 2. Capture the path.
// 3. Copy the file to a temporary file locally
// 4. Redirect the URI to the new temporary file.
// 5. Redirect all other communication requests between the IDE and server to keep LSP working as expected.
//
// 6?. Detect when the editor is back in the project to return to normal behavior.
//
// Response from server when textDocument/definition is called:
//
// IDE to Server: lsproxy::lsp::parser: Sending message len=177
// IDE to Server: lsproxy::lsp::parser: {"id":4,"method":"textDocument/definition","jsonrpc":"2.0","params":{"textDocument":{"uri":"file:///usr/src/app/dirtystroke/settings.py"},"position":{"character":13,"line":15}}}
// Server to IDE: lsproxy::lsp::parser: Raw headers headers_str=Content-Length: 190
// Server to IDE: lsproxy::lsp::parser: Processing header line: 'Content-Length: 190'
// Server to IDE: lsproxy::lsp::parser: Header key: 'Content-Length', value: '190'
// Server to IDE: lsproxy::lsp::parser: Reading body content_length=190
// Server to IDE: lsproxy::lsp::parser: body={"jsonrpc":"2.0","id":4,"result":[{"uri":"file:///usr/local/lib/python3.12/site-packages/django/conf/__init__.py","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}}]}
//
// HERE WE NEED TO COPY THE FILE
//
// Server to IDE: lsproxy::proxy::io: Read message from LSP
// Server to IDE: lsproxy::proxy::io: Incoming message from LSP
// Server to IDE: lsproxy::lsp::parser: Sending message len=190
//
// THEN SEND THE MODIFIED PATH TO TEMPORARY FILE
// Server to IDE: lsproxy::lsp::parser: {"jsonrpc":"2.0","id":4,"result":[{"uri":"file:///usr/local/lib/python3.12/site-packages/django/conf/__init__.py","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}}]}
//
// LIKE
// Server to IDE: lsproxy::lsp::parser: {"jsonrpc":"2.0","id":4,"result":[{"uri":"file:///tmp/lsproxy/django/conf/__init__.py","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}}}]}
//
// THE CLIENT INIT AGAIN THE PROXY FOR ANOTHER ENVIRONMENT??
//
// lsproxy: Connecting to LSP config.container=development-web-1 cmd_args=["exec", "-i", "--workdir", "/usr/src/app", "development-web-1", "pyright-langserver"]
// lsproxy: args received args=Args { inner: ["/Users/richard/proj/lsproxy/target/debug/lsproxy", "--stdio"] }
// lsproxy: full command cmd_args=["exec", "-i", "--workdir", "/usr/src/app", "development-web-1", "pyright-lan

pub async fn bind_library(uri: String, config: &ProxyConfig) -> std::io::Result<()> {
    let temp_dir = std::env::temp_dir().join("lsproxy");
    let temp_uri = temp_dir.join(&uri);

    // Create the directories if they do not exist
    if let Some(parent) = temp_uri.parent() {
        create_dir_all(parent).await?;
    }

    copy_file(uri, temp_uri.to_str().expect("convert the uri to string"), config).await?;

    Ok(())
}

async fn copy_file(path: String, destination: &str, config: &ProxyConfig) -> std::io::Result<()> {
    let mut cmd = if config.use_docker {
        Command::new("docker")
            .args(vec![
                "exec".into(),
                config.container.clone(),
                "cat".into(),
                path,
            ])
            .stdout(Stdio::piped())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("execute docker command")
    } else {
        Command::new("cat")
            .args(vec![path])
            .stdout(Stdio::piped())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("read the file")
    };

    let mut stdout = BufReader::new(cmd.stdout.take().expect("take stdout"));

    let mut buf = vec![];
    stdout.read_to_end(&mut buf).await?;

    let mut file = File::create(destination).await?;
    file.write(&buf).await?;

    Ok(())
}
