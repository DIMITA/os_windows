use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no XDG_CONFIG_HOME")?;
    Ok(base.join("wilai"))
}

pub fn config_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("wilai.toml"))
}

pub fn data_dir() -> Result<PathBuf> {
    let base = dirs::data_dir().context("no XDG_DATA_HOME")?;
    Ok(base.join("wilai"))
}

pub fn audit_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("audit"))
}

pub fn user_tools_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("tools"))
}

pub fn shipped_tools_dir() -> PathBuf {
    if let Ok(env) = std::env::var("WILAI_TOOLS_DIR") {
        return PathBuf::from(env);
    }
    PathBuf::from("/usr/share/wilai/tools")
}

pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}
