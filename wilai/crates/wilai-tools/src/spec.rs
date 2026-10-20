use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wilai_core::{Category, Mode, Risk};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub version: u32,
    pub description: String,
    pub category: Category,
    pub risk: Risk,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub mode_required: Option<Mode>,
    #[serde(default)]
    pub mode_forbidden: Option<Mode>,
    pub inputs: BTreeMap<String, InputSpec>,
    #[serde(default)]
    pub guards: Vec<GuardSpec>,
    pub executor: ExecutorSpec,
    #[serde(default)]
    pub audit: AuditSpec,
    #[serde(default)]
    pub examples: Vec<Example>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSpec {
    #[serde(rename = "type")]
    pub kind: InputType,
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub values: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub items: Option<Box<InputSpec>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputType {
    String,
    Integer,
    Number,
    Boolean,
    Enum,
    Array,
    Object,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardSpec {
    #[serde(rename = "if", default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub require_mode: Option<Mode>,
    #[serde(default)]
    pub confirm: Option<String>,
    #[serde(default)]
    pub deny: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ExecutorSpec {
    Subprocess(SubprocessSpec),
    Builtin(BuiltinSpec),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubprocessSpec {
    pub cmd: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub stdin: Option<String>,
    pub timeout_s: u32,
    pub output: OutputSpec,
    /// Optional sandbox configuration. When present and the host has
    /// `bwrap` (bubblewrap) on PATH, the executor wraps the command in
    /// bwrap with the declared binds and network policy. If bwrap is
    /// missing, the daemon logs a warning on first use and falls back
    /// to running the command unsandboxed (graceful degradation).
    #[serde(default)]
    pub sandbox: Option<SandboxSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SandboxSpec {
    /// bubblewrap sandbox (Linux). Paths are passed verbatim to
    /// `--ro-bind` / `--bind`; network is `unshare_net` unless true.
    Bwrap {
        #[serde(default = "default_ro_bind")]
        bind_ro: Vec<String>,
        #[serde(default)]
        bind_rw: Vec<String>,
        #[serde(default)]
        tmpfs: Vec<String>,
        /// Whether the sandboxed process can touch the network. Default
        /// is false (unshare the net namespace).
        #[serde(default)]
        network: bool,
        /// Pass through specific env vars by name. The daemon already
        /// filters at the executor boundary; this is for white-listing
        /// vars like HOME inside the sandbox.
        #[serde(default)]
        pass_env: Vec<String>,
    },
}

fn default_ro_bind() -> Vec<String> {
    vec![
        "/usr".to_string(),
        "/lib".to_string(),
        "/lib64".to_string(),
        "/bin".to_string(),
        "/sbin".to_string(),
        "/etc".to_string(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinSpec {
    #[serde(rename = "fn")]
    pub function: String,
    pub timeout_s: u32,
    pub output: OutputSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputSpec {
    pub capture: CaptureKind,
    pub max_bytes: usize,
    #[serde(default)]
    pub redact: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureKind {
    Stdout,
    Stderr,
    Both,
    None,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditSpec {
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub redact: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Example {
    pub args: serde_json::Value,
    #[serde(default)]
    pub note: Option<String>,
}
