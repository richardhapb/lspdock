mod cli;
mod provider;
mod variables;

use std::{env::current_dir, path::PathBuf};

pub use cli::Cli;
#[allow(unused)] // In unix encode_path is not used
pub use provider::{ProxyConfig, ProxyConfigToml, encode_path};

const CONFIG_NAME: &str = "lspdock.toml";

pub enum PathType {
    Cwd,
    Home,
}

pub struct ConfigPath {
    path: PathBuf,
    r#type: PathType,
}

/// Get the configuration using the hierarchy order:
///
/// 1. Project path
/// 2. .config directory in the home
pub fn resolve_config_path() -> Option<ConfigPath> {
    let cwd = current_dir().ok()?;
    let cwd_config = cwd.join(CONFIG_NAME);
    if cwd_config.exists() {
        return Some(ConfigPath {
            path: cwd_config,
            r#type: PathType::Cwd,
        });
    }

    let home = dirs::home_dir().unwrap_or_default();
    let home_config = home.join(".config").join("lspdock").join(CONFIG_NAME);

    if home_config.exists() {
        return Some(ConfigPath {
            path: home_config,
            r#type: PathType::Home,
        });
    }

    None
}
