use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub audit: AuditConfig,
    #[serde(default)]
    pub mode: ModeConfig,
}

impl Default for Config {
    fn default() -> Self {
        let mut providers = BTreeMap::new();
        providers.insert(
            "ollama".to_string(),
            ProviderConfig {
                kind: "ollama".to_string(),
                url: Some("http://127.0.0.1:11434".to_string()),
                api_key_env: None,
                local: true,
            },
        );
        Self {
            general: General::default(),
            providers,
            tools: ToolsConfig::default(),
            audit: AuditConfig::default(),
            mode: ModeConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    pub default_provider: String,
    pub default_model: String,
    pub confirm_timeout_s: u32,
    pub system_prompt: Option<String>,
}

impl Default for General {
    fn default() -> Self {
        Self {
            default_provider: "ollama".to_string(),
            default_model: "mistral:7b-instruct".to_string(),
            confirm_timeout_s: 30,
            system_prompt: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(rename = "type")]
    pub kind: String,
    pub url: Option<String>,
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub local: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsConfig {
    #[serde(default)]
    pub extra_dirs: Vec<PathBuf>,
    #[serde(default)]
    pub disabled: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    pub dir: Option<PathBuf>,
    #[serde(default = "default_true")]
    pub chattr_append: bool,
    #[serde(default = "default_true")]
    pub include_output_sample: bool,
    #[serde(default = "default_max_size_mb")]
    pub max_size_mb: u64,
}

fn default_true() -> bool { true }
fn default_max_size_mb() -> u64 { 256 }

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            dir: None,
            chattr_append: true,
            include_output_sample: true,
            max_size_mb: 256,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeConfig {
    #[serde(default = "default_true")]
    pub auto_detect: bool,
    #[serde(default = "default_threshold")]
    pub threshold: u32,
}

fn default_threshold() -> u32 { 100 }

impl Default for ModeConfig {
    fn default() -> Self {
        Self { auto_detect: true, threshold: 100 }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = crate::paths::config_file()?;
        if !path.exists() {
            tracing::debug!("no config file at {}; using defaults", path.display());
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("parse {}", path.display()))?;
        Ok(cfg)
    }
}
