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
    #[serde(default)]
    pub voice: VoiceConfig,
    #[serde(default)]
    pub proactivity: ProactivityConfig,
    /// Named remote Wilai instances reachable over SSH. Each entry
    /// pairs a short name (e.g. "homelab", "vps") with the SSH target
    /// so the remote.* tool family can address them symbolically
    /// instead of asking the LLM to remember hostnames.
    #[serde(default)]
    pub remotes: BTreeMap<String, RemoteConfig>,
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

/// Voice I/O knobs read by wilai-shell. All engines are LOCAL subprocesses
/// (whisper.cpp for STT, piper for TTS, arecord/aplay for raw audio).
/// The shell runs them out-of-process so this binary carries no audio
/// dependencies; missing engines degrade gracefully — the shell falls
/// back to text and warns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceConfig {
    /// When true, wilai-shell boots with voice I/O enabled (equivalent
    /// to passing --voice on the CLI).
    #[serde(default)]
    pub enabled: bool,
    /// whisper.cpp binary (looked up on PATH if relative).
    #[serde(default = "default_whisper_bin")]
    pub whisper_bin: String,
    /// Path to the whisper.cpp model (.gguf or .bin). Resolved from
    /// $XDG_DATA_HOME/wilai/models/ when relative.
    #[serde(default = "default_whisper_model")]
    pub whisper_model: String,
    /// piper binary.
    #[serde(default = "default_piper_bin")]
    pub piper_bin: String,
    /// piper voice model (.onnx). Resolved from
    /// $XDG_DATA_HOME/wilai/models/ when relative. Falls back to
    /// `persona.voice_model` + ".onnx" if empty.
    #[serde(default)]
    pub piper_model: String,
    /// Recording binary (arecord by default). Receives the args
    /// `-q -d <secs> -f S16_LE -r <sample_rate> -c 1 <wav>`.
    #[serde(default = "default_record_bin")]
    pub record_bin: String,
    /// Playback binary (aplay by default).
    #[serde(default = "default_play_bin")]
    pub play_bin: String,
    /// Recording sample rate; must match the whisper model's expectation
    /// (16000 Hz for the standard base/small/medium models).
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    /// Max seconds per push-to-talk recording. VAD-trim happens inside
    /// whisper itself; this is the upper bound.
    #[serde(default = "default_record_secs")]
    pub record_secs: u32,
    /// Optional wake-word command. Each line on its stdout triggers a
    /// recording round. Example:
    ///   openwakeword --model hey_wil.onnx --emit-on-detect
    /// Set empty to use push-to-talk only (F2 inside the shell).
    #[serde(default)]
    pub wake_cmd: String,
    /// Minimum chars before TTS speaks a reply. Smaller than this is
    /// typed-only (silent). 0 means speak everything.
    #[serde(default = "default_tts_min_chars")]
    pub tts_min_chars: usize,
}

fn default_whisper_bin() -> String { "whisper-cli".to_string() }
fn default_whisper_model() -> String { "ggml-base.en.bin".to_string() }
fn default_piper_bin() -> String { "piper".to_string() }
fn default_record_bin() -> String { "arecord".to_string() }
fn default_play_bin() -> String { "aplay".to_string() }
fn default_sample_rate() -> u32 { 16000 }
fn default_record_secs() -> u32 { 6 }
fn default_tts_min_chars() -> usize { 1 }

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            whisper_bin: default_whisper_bin(),
            whisper_model: default_whisper_model(),
            piper_bin: default_piper_bin(),
            piper_model: String::new(),
            record_bin: default_record_bin(),
            play_bin: default_play_bin(),
            sample_rate: default_sample_rate(),
            record_secs: default_record_secs(),
            wake_cmd: String::new(),
            tts_min_chars: default_tts_min_chars(),
        }
    }
}

/// Proactivity knobs. When enabled, the daemon runs lightweight
/// background watchers (systemd units, disk free space, named
/// processes) and emits `ServerEvent::Nudge` to connected clients when
/// something crosses a threshold. The quota prevents flooding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactivityConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Max nudges emitted per rolling hour across all watchers. 0
    /// means "no proactive nudges". Falls back to persona.quota_per_hour
    /// when 0 and persona.quota_per_hour > 0.
    #[serde(default = "default_nudge_quota")]
    pub quota_per_hour: u32,
    /// Interval between systemd-failed polls, seconds.
    #[serde(default = "default_systemd_poll")]
    pub systemd_poll_s: u32,
    /// Interval between disk-free polls, seconds.
    #[serde(default = "default_disk_poll")]
    pub disk_poll_s: u32,
    /// Nudge when free space on a watched mount drops below this percent.
    #[serde(default = "default_disk_threshold")]
    pub disk_threshold_pct: u32,
    /// Mountpoints to watch (absolute paths). Default ["/"] when empty.
    #[serde(default)]
    pub watch_mounts: Vec<String>,
    /// Process names whose disappearance warrants a nudge.
    #[serde(default)]
    pub watch_processes: Vec<String>,
    /// Interval between process-watchlist polls, seconds.
    #[serde(default = "default_process_poll")]
    pub process_poll_s: u32,
}

fn default_nudge_quota() -> u32 { 6 }
fn default_systemd_poll() -> u32 { 30 }
fn default_disk_poll() -> u32 { 60 }
fn default_disk_threshold() -> u32 { 10 }
fn default_process_poll() -> u32 { 30 }

/// SSH endpoint for a federated Wilai instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    /// SSH destination (user@host or an alias from ~/.ssh/config).
    pub ssh: String,
    /// Human description shown by `/remotes`.
    #[serde(default)]
    pub description: String,
    /// Socket path on the remote side where its wilai-daemon listens.
    /// Default: /run/wilai.sock (the WilOS-Core system path).
    #[serde(default = "default_remote_socket")]
    pub socket: String,
    /// Allow this remote in Pentest mode. Off by default — pentest
    /// should not fan out to other machines accidentally.
    #[serde(default)]
    pub allow_in_pentest: bool,
}

fn default_remote_socket() -> String {
    "/run/wilai.sock".to_string()
}

impl Default for ProactivityConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            quota_per_hour: default_nudge_quota(),
            systemd_poll_s: default_systemd_poll(),
            disk_poll_s: default_disk_poll(),
            disk_threshold_pct: default_disk_threshold(),
            watch_mounts: vec![],
            watch_processes: vec![],
            process_poll_s: default_process_poll(),
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
            voice: VoiceConfig::default(),
            proactivity: ProactivityConfig::default(),
            remotes: BTreeMap::new(),
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
