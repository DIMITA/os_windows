//! Config loading for the Windows bridge. We parse a tiny subset of
//! TOML with the `toml` crate — same shape the PowerShell launcher
//! wrote at install time, same path (%APPDATA%\wilai\config.toml on
//! Windows, $XDG_CONFIG_HOME/wilai/config.toml everywhere else so
//! the Linux build still parses it in tests).

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub bridge: BridgeConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self { bridge: BridgeConfig::default() }
    }
}

#[derive(Debug, Deserialize)]
pub struct BridgeConfig {
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub wsl: WslConfig,
    #[serde(default)]
    pub ssh: SshConfig,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            wsl: WslConfig::default(),
            ssh: SshConfig::default(),
        }
    }
}

fn default_mode() -> String {
    "wsl".to_string()
}

#[derive(Debug, Deserialize)]
pub struct WslConfig {
    #[serde(default)]
    pub distro: String,
    #[serde(default)]
    pub user: String,
    #[serde(default = "default_shell")]
    pub shell: String,
}

impl Default for WslConfig {
    fn default() -> Self {
        Self { distro: String::new(), user: String::new(), shell: default_shell() }
    }
}

fn default_shell() -> String {
    "wilai-shell".to_string()
}

#[derive(Debug, Deserialize)]
pub struct SshConfig {
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub key: String,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self { host: String::new(), port: default_port(), key: String::new() }
    }
}

fn default_port() -> u16 {
    22
}

/// Resolve the default config path. On Windows that is
/// %APPDATA%\wilai\config.toml, matching what install.ps1 wrote. On
/// other platforms we fall back to the XDG config dir so the Rust
/// code still parses on Linux (for `cargo test` and workspace
/// builds).
pub fn default_config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().ok_or_else(|| anyhow!("no config dir"))?;
    Ok(base.join("wilai").join("config.toml"))
}

pub fn load(path: Option<&Path>) -> Result<Config> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => default_config_path()?,
    };
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let cfg: Config = toml::from_str(&text)
        .with_context(|| format!("parse {}", path.display()))?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_wsl_config() {
        let toml = r#"
            [bridge]
            mode = "wsl"
            [bridge.wsl]
            distro = "WilOS"
            user   = "nora"
            shell  = "wilai-shell"
            [bridge.ssh]
            host = ""
            port = 22
            key  = ""
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.bridge.mode, "wsl");
        assert_eq!(cfg.bridge.wsl.distro, "WilOS");
        assert_eq!(cfg.bridge.wsl.user, "nora");
        assert_eq!(cfg.bridge.wsl.shell, "wilai-shell");
    }

    #[test]
    fn parses_full_ssh_config() {
        let toml = r#"
            [bridge]
            mode = "ssh"
            [bridge.ssh]
            host = "wilai@my-box.lan"
            port = 2222
            key  = "C:\\Users\\me\\.ssh\\wilai_ed25519"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.bridge.mode, "ssh");
        assert_eq!(cfg.bridge.ssh.host, "wilai@my-box.lan");
        assert_eq!(cfg.bridge.ssh.port, 2222);
    }

    #[test]
    fn empty_config_uses_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.bridge.mode, "wsl");
        assert_eq!(cfg.bridge.ssh.port, 22);
        assert_eq!(cfg.bridge.wsl.shell, "wilai-shell");
    }
}
