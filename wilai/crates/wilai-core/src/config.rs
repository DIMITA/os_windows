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
    #[serde(default)]
    pub mcp: BTreeMap<String, McpServerConfig>,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub persona: PersonaConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// When true, non-local providers (anything with `local = false`) are
    /// refused even in Normal mode — not just in Pentest. This is the
    /// hard commitment to a local-only agent: no cloud egress for the
    /// LLM, embeddings, STT, TTS, or any other model call. Default true.
    #[serde(default = "default_true")]
    pub local_only: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self { Self { local_only: true } }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaConfig {
    /// How the agent addresses the user. "" means no address prefix.
    #[serde(default = "default_address")]
    pub address: String,
    /// Short style tag the system prompt layers on. Known values:
    /// "concise_butler" (Jarvis-like, posed, formal), "neutral".
    #[serde(default = "default_style")]
    pub style: String,
    /// piper voice model file name (without extension). Resolved from
    /// $XDG_DATA_HOME/wilai/models/.
    #[serde(default = "default_voice")]
    pub voice_model: String,
    /// How often the agent is allowed to open a dialogue on its own
    /// per hour (watchers, scheduled nudges). 0 = never proactive.
    #[serde(default = "default_quota")]
    pub quota_per_hour: u32,
    #[serde(default = "default_proactivity")]
    pub proactivity: String,
}

fn default_address() -> String { "Monsieur".to_string() }
fn default_style() -> String { "concise_butler".to_string() }
fn default_voice() -> String { "en_GB-alan-medium".to_string() }
fn default_quota() -> u32 { 3 }
fn default_proactivity() -> String { "medium".to_string() }

impl Default for PersonaConfig {
    fn default() -> Self {
        Self {
            address: default_address(),
            style: default_style(),
            voice_model: default_voice(),
            quota_per_hour: default_quota(),
            proactivity: default_proactivity(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Executable to spawn (e.g. "npx", "python", "/path/to/server").
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables for the server process. Inherited
    /// environment is filtered down to PATH/LANG/LC_ALL/HOME by the daemon.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// If true (default), this server is not spawned in pentest mode and
    /// any of its tools are gated out. Set false only for fully-local
    /// servers that you have audited (e.g. a local-files MCP server).
    #[serde(default = "default_true")]
    pub gate_in_pentest: bool,
    /// If false, the server is declared but not actually spawned. Useful
    /// for staging or troubleshooting.
    #[serde(default = "default_true")]
    pub enabled: bool,
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
            mcp: BTreeMap::new(),
            security: SecurityConfig::default(),
            persona: PersonaConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    #[serde(default = "default_provider_name")]
    pub default_provider: String,
    #[serde(default = "default_model_name")]
    pub default_model: String,
    #[serde(default = "default_confirm_timeout_s")]
    pub confirm_timeout_s: u32,
    #[serde(default)]
    pub system_prompt: Option<String>,
}

fn default_provider_name() -> String { "ollama".to_string() }
fn default_model_name() -> String { "mistral:7b-instruct".to_string() }
fn default_confirm_timeout_s() -> u32 { 30 }

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
