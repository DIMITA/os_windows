use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no XDG_CONFIG_HOME")?;
    Ok(base.join("wilai"))
}

pub fn config_file() -> Result<PathBuf> {
    // 1. Explicit override via WILAI_CONFIG env var.
    if let Ok(p) = std::env::var("WILAI_CONFIG") {
        return Ok(PathBuf::from(p));
    }
    // 2. Per-user XDG config.
    let user = config_dir()?.join("wilai.toml");
    if user.exists() {
        return Ok(user);
    }
    // 3. System-wide fallback (shipped by the .deb postinst).
    let system = PathBuf::from("/etc/wilai/config.toml");
    if system.exists() {
        return Ok(system);
    }
    // 4. None found yet — return the per-user path so the error
    //    message names a sensible location.
    Ok(user)
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
