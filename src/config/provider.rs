use memchr::memmem::find;
use std::env;
use std::path::Path;
use std::{env::current_dir, error::Error, fmt::Display};
use tokio_util::bytes::Bytes;

use serde::Deserialize;

use crate::config::variables::{VariableCwd, VariableHome, VariableParent, VariableResolver};
use crate::config::{Cli, ConfigPath, PathType};

#[derive(Debug)]
pub enum ConfigParseError {
    FileError(std::io::Error),
    DeserializationError(toml::de::Error),
    MissingField(&'static str),
}

impl Error for ConfigParseError {}

impl Display for ConfigParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::FileError(e) => format!("File cannot be readed: {e}"),
            Self::DeserializationError(e) => format!("Error parsing config file: {e}"),
            Self::MissingField(e) => format!("{e} must be provided"),
        };
        write!(f, "{text}")
    }
}

impl From<std::io::Error> for ConfigParseError {
    fn from(value: std::io::Error) -> Self {
        Self::FileError(value)
    }
}

impl From<toml::de::Error> for ConfigParseError {
    fn from(value: toml::de::Error) -> Self {
        Self::DeserializationError(value)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProxyConfig {
    pub container: String,
    pub docker_internal_path: String,
    pub local_path: String,
    pub executable: String,

    /// Indicates whether to patch the PID to null; this is used when the LSP tries to track the IDE and
    /// auto-kill when it can't detect it. The listed executables in this list will be patched
    pub patch_pid: Option<Vec<String>>,
    pub log_level: String,
    pub use_docker: bool,
}

impl ProxyConfig {
    pub fn from_proxy_config_toml(
        config: ProxyConfigToml,
        mut use_docker: bool,
    ) -> Result<Self, ConfigParseError> {
        #[allow(unused_mut)]
        let local_path = config
            .local_path
            .or_else(|| current_dir().ok()?.to_str().map(String::from));

        // Normalize local path for Windows
        #[cfg(windows)]
        let local_path = local_path.map(|p| normalize_win_local(&p));

        let local_path = local_path.ok_or(ConfigParseError::MissingField("local_path"))?;

        let mut executable = extract_binary_name(&env::args().next().unwrap_or("".to_string()));

        // If the binary has not been renamed, use the config.
        // Panic if the config doesn't provide it.
        if executable == "lspdock" {
            executable = config
                .executable
                .ok_or(ConfigParseError::MissingField("executable"))?;
        }

        // Auto-disable Docker if required fields missing (zero-config mode)
        if config.container.is_none() || config.docker_internal_path.is_none() {
            use_docker = false;
        }

        // Use empty strings as placeholders when Docker is disabled
        let container = config.container.unwrap_or_default();
        let docker_internal_path = config.docker_internal_path.unwrap_or_default();

        Ok(Self {
            container,
            docker_internal_path,
            local_path: local_path.clone(),
            executable,
            patch_pid: config.patch_pid,
            log_level: config
                .log_level
                .unwrap_or_else(|| std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into())),
            use_docker,
        })
    }

    /// Indicate if the executable requires patch to the pid
    pub fn requires_patch_pid(&self) -> bool {
        match &self.patch_pid {
            Some(patch_pid) => {
                if let Some(name) = Path::new(&self.executable).file_name() {
                    // compare against list of binaries
                    patch_pid.contains(&name.to_string_lossy().into_owned())
                } else {
                    false
                }
            }
            None => false,
        }
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ProxyConfigToml {
    pub(super) container: Option<String>,
    pub(super) docker_internal_path: Option<String>,
    pub(super) local_path: Option<String>,
    pub(super) executable: Option<String>,
    /// This serves as a pattern for the proxy to Docker; if the pattern doesn't match, the proxy will
    /// forward requests directly to the local LSP.
    pub(super) pattern: Option<String>,

    /// Indicates whether to patch the PID to null; this is used when the LSP tries to track the IDE and
    /// auto-kill when it can't detect it. The listed executables in this list will be patched
    pub(super) patch_pid: Option<Vec<String>>,
    pub(super) log_level: Option<String>,
}

impl ProxyConfigToml {
    pub fn from_file(
        config_path: Option<&ConfigPath>,
        cli: &mut Cli,
    ) -> Result<ProxyConfig, ConfigParseError> {
        let mut config = Self::default();
        if let Some(cp) = config_path {
            let file_str = std::fs::read_to_string(&cp.path)?;
            config = toml::from_str(&file_str)?;
        }

        // Cli has precedence in priority
        config.container = cli.container.take().or(config.container);
        config.local_path = cli.local_path.take().or(config.local_path);
        config.docker_internal_path = cli.docker_path.take().or(config.docker_internal_path);
        config.executable = cli.exec.take().or(config.executable);
        config.pattern = cli.pattern.take().or(config.pattern);
        config.patch_pid = cli.pids.take().or(config.patch_pid);
        config.log_level = cli.log_level.take().or(config.log_level);

        let cwd_var = VariableCwd::default();
        let parent_var = VariableParent::default();
        let home_var = VariableHome::default();
        cwd_var.expand(&mut config).unwrap();
        parent_var.expand(&mut config).unwrap();
        home_var.expand(&mut config).unwrap();

        let use_docker = match config_path {
            Some(cp) => match cp.r#type {
                PathType::Home => {
                    let cwd = current_dir()?;
                    cwd_matches_pattern(&cwd, config.pattern.as_deref())
                }
                PathType::Cwd => true, // In cwd always the pattern matches
            },
            None => true,
        };

        ProxyConfig::from_proxy_config_toml(config, use_docker)
    }
}

fn extract_binary_name(full_path: &str) -> String {
    let name = std::path::Path::new(full_path)
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("lspdock");
    name.to_string()
}

// Helper function to normalize paths
#[allow(dead_code)] // In unix is not used
fn normalize_win_local(path: &str) -> String {
    let mut s = std::path::Path::new(path).to_string_lossy().to_string();
    s = s.replace('\\', "/");
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        let mut it = s.chars();
        let drive = it.next().unwrap().to_ascii_lowercase();
        let rest: String = it.collect();
        s = format!("/{}{}", drive, rest);
    }
    s
}

fn norm_for_match<S: AsRef<str>>(s: S) -> String {
    #[allow(unused_mut)]
    let mut t = s.as_ref().replace('\\', "/");

    #[cfg(windows)]
    {
        t.make_ascii_lowercase();
    }

    t
}

fn cwd_matches_pattern(cwd: &Path, pattern: Option<&str>) -> bool {
    let cwd_s = norm_for_match(cwd.to_string_lossy());
    match pattern {
        Some(p) if !p.is_empty() => {
            let p = norm_for_match(p);
            cwd_s.contains(&p)
        }
        _ => true, // no pattern → default to Docker
    }
}

#[allow(dead_code)] // Not used in Unix
pub fn encode_path(msg: &Bytes, config: &mut ProxyConfig) {
    config.local_path = if find(msg, b"%3A").is_some() {
        config.local_path.replace(":", "%3A")
    } else {
        config.local_path.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_binary_name_properly() {
        let full_path = "/home/someone/bin/lspdock";
        let expect = "lspdock";

        assert_eq!(extract_binary_name(full_path), expect);

        let full_path = "/home/someone/bin/rust-analyzer";
        let expect = "rust-analyzer";
        assert_eq!(extract_binary_name(full_path), expect);

        let full_path = "/";
        let expect = "lspdock";
        assert_eq!(extract_binary_name(full_path), expect);

        let full_path = "";
        let expect = "lspdock";
        assert_eq!(extract_binary_name(full_path), expect);
    }
}

#[cfg(test)]
mod windows_tests {
    use crate::lsp::parser::lsp_utils::lspmsg;

    use super::*;

    #[test]
    fn windows_normalize_local() {
        let local_path = "C:\\Users\\testUser\\dev";
        let normalized = normalize_win_local(local_path);

        assert_eq!(normalized, "/c:/Users/testUser/dev");
    }

    #[test]
    fn windows_detect_colon_type() {
        let config_toml = ProxyConfigToml {
            container: Some("test".into()),
            local_path: Some("/c:/Users/testUser/dev".into()),
            docker_internal_path: Some("/usr/home/app".into()),
            ..Default::default()
        };

        let mut config = ProxyConfig::from_proxy_config_toml(config_toml.clone(), true).unwrap();

        // Encoded

        let msg_with_colon = lspmsg!("uri": "/c%3A/Users/testUser/dev/somefile.rs");
        let msg_bytes = Bytes::from(msg_with_colon);

        encode_path(&msg_bytes, &mut config);
        assert_eq!(config.local_path, "/c%3A/Users/testUser/dev".to_string());

        // With raw colon

        let mut config = ProxyConfig::from_proxy_config_toml(config_toml, true).unwrap();
        let msg_with_colon = lspmsg!("uri": "/c:/Users/testUser/dev/somefile.rs");
        let msg_bytes = Bytes::from(msg_with_colon);

        encode_path(&msg_bytes, &mut config);

        assert_eq!(config.local_path, "/c:/Users/testUser/dev".to_string());
    }

    #[cfg(windows)]
    #[test]
    fn windows_extact_binary_name() {
        let full_path = "\\home\\someone\\bin\\rust-analyzer.exe";
        let expect = "rust-analyzer";
        assert_eq!(extract_binary_name(&full_path), expect);

        let full_path = "\\home\\someone\\bin\\rust-analyzer";
        let expect = "rust-analyzer";
        assert_eq!(extract_binary_name(&full_path), expect);
    }

    #[cfg(windows)]
    #[test]
    fn windows_use_backslash_on_cwd() {
        assert!(current_dir().unwrap().to_str().unwrap().contains("\\"));
    }
}
