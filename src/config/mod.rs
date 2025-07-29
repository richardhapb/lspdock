mod provider;
mod variables;

use std::{env::current_dir, path::PathBuf};

pub use provider::ProxyConfig;

const CONFIG_NAME: &str = "lsproxy.toml";

/// Get the configuration using the hierarchy order:
///
/// 1. Project path
/// 2. .config directory in the home
pub fn resolve_config_path() -> std::io::Result<PathBuf> {
    let cwd = current_dir()?;
    let cwd_config = cwd.join(CONFIG_NAME);
    if cwd_config.exists() {
        return Ok(cwd_config);
    }

    let home = dirs::home_dir().unwrap_or(PathBuf::new());
    let home_config = home.join(".config").join("lsproxy").join(CONFIG_NAME);

    if home_config.exists() {
        return Ok(home_config);
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "Config file not found",
    ))
}
