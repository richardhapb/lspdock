use std::path::Path;
use std::{env::current_dir, error::Error, fmt::Display};

use serde::Deserialize;

use crate::config::variables::{VariableCwd, VariableHome, VariableParent, VariableResolver};
use crate::config::{ConfigPath, PathType};

#[derive(Debug)]
pub enum ConfigParseError {
    FileError(std::io::Error),
    DeserializationError(toml::de::Error),
}

impl Error for ConfigParseError {}

impl Display for ConfigParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::FileError(e) => format!("File cannot be readed: {e}"),
            Self::DeserializationError(e) => format!("Error parsing config file: {e}"),
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

#[derive(Debug, Deserialize, Clone)]
pub struct ProxyConfig {
    pub container: String,
    pub local_path: String,
    pub docker_internal_path: String,
    pub executable: String,
    /// This serves as a pattern for the proxy to Docker; if the pattern doesn't match, the proxy will
    /// forward requests directly to the local LSP.
    pub pattern: Option<String>,
    #[serde(skip)]
    pub use_docker: bool,

    /// Indicates whether to patch the PID to null; this is used when the LSP tries to track the IDE and
    /// auto-kill when it can't detect it. The listed executables in this list will be patched
    pub patch_pid: Option<Vec<String>>,
    pub log_level: Option<String>,
}

impl ProxyConfig {
    pub fn from_file(config_path: &ConfigPath) -> Result<Self, ConfigParseError> {
        let file_str = std::fs::read_to_string(&config_path.path)?;
        let mut config = toml::from_str(&file_str)?;

        let cwd_var = VariableCwd::default();
        let parent_var = VariableParent::default();
        let home_var = VariableHome::default();
        cwd_var.expand(&mut config).unwrap();
        parent_var.expand(&mut config).unwrap();
        home_var.expand(&mut config).unwrap();

        // Normalize local path for Windows
        #[cfg(windows)]
        {
            config.local_path = normalize_win_local(&config.local_path);
        }

        config.use_docker = match config_path.r#type {
            PathType::Home => {
                let cwd = current_dir()?;
                cwd_matches_pattern(&cwd, config.pattern.as_deref())
            }
            PathType::Cwd => true, // In cwd always the pattern matches
        };

        Ok(config)
    }

    pub fn update_executable(&mut self, exec: String) {
        self.executable = exec
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

// Helper function to normalize paths
#[cfg(windows)]
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
