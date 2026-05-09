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
