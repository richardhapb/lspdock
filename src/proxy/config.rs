use std::{error::Error, fmt::Display};

use serde::Deserialize;

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
}

impl ProxyConfig {
    pub fn from_file(path: &str) -> Result<Self, ConfigParseError> {
        let file_str = std::fs::read_to_string(path)?;

        Ok(toml::from_str(&file_str)?)
    }
}
