use serde::{Deserialize, Serialize};
use serde_json::Value;
use wilai_core::Mode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub v: u32,
    pub seq: u64,
    pub ts: String,
    pub id: String,
    pub prev_hash: String,
    pub kind: String,
    pub session: String,
    pub mode: Mode,
    #[serde(flatten)]
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub enum EntryPayload {
    SessionStart {
        entry: String,
        cwd: String,
        user: String,
    },
    SessionEnd,
    ToolExec {
        tool_name: String,
        tool_version: u32,
        category: String,
        risk: String,
        args: Value,
        executor: String,
        cmd: Option<Vec<String>>,
        exit_code: i32,
        duration_ms: u64,
        output_bytes: usize,
        output_truncated: bool,
        output_sample: Option<String>,
        provider: String,
        model: String,
        turn_id: String,
        tool_call_id: String,
    },
    ToolDeny {
        tool_name: String,
        args: Value,
        reason: String,
        guard: String,
    },
    ToolConfirm {
        tool_name: String,
        args: Value,
        prompt: String,
        answer: String,
        latency_ms: u64,
    },
    ProviderCall {
        provider: String,
        model: String,
        prompt_tokens: Option<u32>,
        output_tokens: Option<u32>,
        duration_ms: u64,
        n_tools: u32,
        n_messages: u32,
        error: Option<String>,
    },
    ModeChange {
        from: Mode,
        to: Mode,
        trigger: String,
    },
    SystemDaemonStart,
    SystemDaemonStop,
    SystemRotatePre,
    SystemRotatePost { prev_file: Option<String> },
    SystemChainGenesis,
    SystemChainResume { recovered_hash: String },
    SystemChattrApplied { file: String },
    SystemChattrUnavailable { fs: String },
}

impl EntryPayload {
    pub fn kind(&self) -> &'static str {
        match self {
            EntryPayload::SessionStart { .. } => "session.start",
            EntryPayload::SessionEnd => "session.end",
            EntryPayload::ToolExec { .. } => "tool.exec",
            EntryPayload::ToolDeny { .. } => "tool.deny",
            EntryPayload::ToolConfirm { .. } => "tool.confirm",
            EntryPayload::ProviderCall { .. } => "provider.call",
            EntryPayload::ModeChange { .. } => "mode.change",
            EntryPayload::SystemDaemonStart => "system.daemon_start",
            EntryPayload::SystemDaemonStop => "system.daemon_stop",
            EntryPayload::SystemRotatePre => "system.rotate_pre",
            EntryPayload::SystemRotatePost { .. } => "system.rotate_post",
            EntryPayload::SystemChainGenesis => "system.chain_genesis",
            EntryPayload::SystemChainResume { .. } => "system.chain_resume",
            EntryPayload::SystemChattrApplied { .. } => "system.chattr_applied",
            EntryPayload::SystemChattrUnavailable { .. } => "system.chattr_unavailable",
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            EntryPayload::SessionStart { entry, cwd, user } => serde_json::json!({
                "entry": entry, "cwd": cwd, "user": user
            }),
            EntryPayload::SessionEnd => Value::Object(Default::default()),
            EntryPayload::ToolExec {
                tool_name, tool_version, category, risk, args, executor, cmd,
                exit_code, duration_ms, output_bytes, output_truncated, output_sample,
                provider, model, turn_id, tool_call_id,
            } => {
                let mut o = serde_json::json!({
                    "tool": {
                        "name": tool_name,
                        "version": tool_version,
                        "category": category,
                        "risk": risk,
                    },
                    "args": args,
                    "executor": executor,
                    "exit_code": exit_code,
                    "duration_ms": duration_ms,
                    "output": {
                        "bytes": output_bytes,
                        "truncated": output_truncated,
                    },
                    "provider": provider,
                    "model": model,
                    "turn_id": turn_id,
                    "tool_call_id": tool_call_id,
                });
                if let Some(c) = cmd {
                    o["cmd"] = serde_json::json!(c);
                }
                if let Some(s) = output_sample {
                    o["output"]["sample"] = serde_json::json!(s);
                }
                o
            }
            EntryPayload::ToolDeny { tool_name, args, reason, guard } => serde_json::json!({
                "tool": { "name": tool_name },
                "args": args,
                "reason": reason,
                "guard": guard,
            }),
            EntryPayload::ToolConfirm { tool_name, args, prompt, answer, latency_ms } => {
                serde_json::json!({
                    "tool": { "name": tool_name },
                    "args": args,
                    "prompt": prompt,
                    "answer": answer,
                    "latency_ms": latency_ms,
                })
            }
            EntryPayload::ProviderCall {
                provider, model, prompt_tokens, output_tokens, duration_ms,
                n_tools, n_messages, error,
            } => {
                let mut o = serde_json::json!({
                    "provider": provider,
                    "model": model,
                    "duration_ms": duration_ms,
                    "n_tools": n_tools,
                    "n_messages": n_messages,
                });
                if let Some(t) = prompt_tokens { o["prompt_tokens"] = serde_json::json!(t); }
                if let Some(t) = output_tokens { o["output_tokens"] = serde_json::json!(t); }
                if let Some(e) = error { o["error"] = serde_json::json!(e); }
                o
            }
            EntryPayload::ModeChange { from, to, trigger } => serde_json::json!({
                "from": from, "to": to, "trigger": trigger,
            }),
            EntryPayload::SystemDaemonStart
            | EntryPayload::SystemDaemonStop
            | EntryPayload::SystemRotatePre
            | EntryPayload::SystemChainGenesis => Value::Object(Default::default()),
            EntryPayload::SystemRotatePost { prev_file } => match prev_file {
                Some(f) => serde_json::json!({ "prev_file": f }),
                None => Value::Object(Default::default()),
            },
            EntryPayload::SystemChainResume { recovered_hash } => serde_json::json!({
                "recovered_hash": recovered_hash,
            }),
            EntryPayload::SystemChattrApplied { file } => serde_json::json!({ "file": file }),
            EntryPayload::SystemChattrUnavailable { fs } => serde_json::json!({ "fs": fs }),
        }
    }
}
