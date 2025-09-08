use std::{env::current_dir, error::Error, fmt::Display, path::PathBuf};

use serde::Deserialize;

use crate::config::variables::{VariableCwd, VariableHome, VariableParent, VariableResolver};

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
    pub pattern: String,
    #[serde(skip)]
    pub use_docker: bool,

    /// Indicates whether to patch the PID to null; this is used when the LSP tries to track the IDE and
    /// auto-kill when it can't detect it. The listed executables in this list will be patched
    pub patch_pid: Option<Vec<String>>,
    pub log_level: Option<String>,
}

impl ProxyConfig {
    pub fn from_file(path: &PathBuf) -> Result<Self, ConfigParseError> {
        let file_str = std::fs::read_to_string(path)?;
        let mut config = toml::from_str(&file_str)?;

        let cwd_var = VariableCwd::default();
        let parent_var = VariableParent::default();
        let home_var = VariableHome::default();
        cwd_var.expand(&mut config).unwrap();
        parent_var.expand(&mut config).unwrap();
        home_var.expand(&mut config).unwrap();

       // Normalize paths for Windows
        #[cfg(windows)]
        {
            config.container = normalize_path(&config.container);
            config.local_path = normalize_path(&config.local_path);
            config.docker_internal_path = normalize_path(&config.docker_internal_path);
            config.pattern = normalize_path(&config.pattern);
            config.executable = normalize_path(&config.executable);
        }

        let cwd = current_dir()?;
        let cwd = cwd.to_str().expect("get current dir");
        config.use_docker = cwd.contains(&config.pattern);

        Ok(config)
    }

    pub fn update_executable(&mut self, exec: String) {
        self.executable = exec
    }

    /// Indicate if the executable requires patch to the pid
    pub fn requires_patch_pid(&self) -> bool {
        match &self.patch_pid {
            Some(patch_pid) => patch_pid.contains(&self.executable),
            None => false
        }
    }
}

// Helper function to normalize paths
#[cfg(windows)]
fn normalize_path(path: &str) -> String {
    std::path::Path::new(path)
        .to_string_lossy()
        .replace("/", "\\")
}

