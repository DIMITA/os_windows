//! End-to-end agent loop with a scripted MockProvider: feeds the loop a
//! tool_call -> text sequence, verifies that the local fs.read builtin
//! executes, that the result flows back into the next turn, and that the
//! streamed ServerEvent sequence matches what a client would see.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;
use wilai_audit::AuditWriter;
use wilai_core::{Config, Mode};
use wilai_daemon::mode_mgr::ModeManager;
use wilai_daemon::protocol::ServerEvent;
use wilai_daemon::service::Service;
use wilai_daemon::session::{build_system_prompt, run_turn};
use wilai_providers::{AssistantMessage, ChatResponse, MockProvider, ToolCall};
use wilai_tools::confirm::{ConfirmAnswer, FixedConfirmer};

fn tools_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("tools");
    p.push("core");
    p
}

/// Build an in-process Service with just the YAML tool registry and a
/// temp audit log.
async fn in_process_service() -> (Arc<Service>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let audit_dir = tmp.path().join("audit");
    std::fs::create_dir_all(&audit_dir).unwrap();
    let writer = AuditWriter::open(&audit_dir).unwrap();
    let (audit_tx, _h) = writer.spawn_task();

    let cfg = Config::default();
    let registry = wilai_tools::Registry::load_from_dirs(&[tools_dir()], &[]).unwrap();
    assert!(registry.len() > 0, "no YAML tools loaded from {:?}", tools_dir());

    let mode = ModeManager::new(Mode::Normal, audit_tx.clone());

    let service = Arc::new(Service {
        cfg,
        registry,
        audit_tx,
        mode,
        default_provider: "mock".to_string(),
        default_model: "mock-1".to_string(),
        confirm_timeout_s: 5,
        mcp: HashMap::new(),
    });
    (service, tmp)
}

async fn drain_events(mut rx: mpsc::Receiver<ServerEvent>) -> Vec<ServerEvent> {
    let mut out = Vec::new();
    while let Ok(Some(ev)) =
        tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
    {
        out.push(ev);
    }
    out
}

#[tokio::test]
async fn agent_loop_runs_tool_then_replies() {
    let (service, tmp) = in_process_service().await;

    // Write a file the agent will be asked to read.
    let probe = tmp.path().join("probe.txt");
    std::fs::write(&probe, "hello wilai").unwrap();

    // Script: first chat() returns a tool_call for fs.read on probe.txt;
    // second chat() returns a plain text summary.
    let provider = MockProvider::with_script(
        "mock",
        vec![
            ChatResponse {
                message: AssistantMessage {
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "fs.read".into(),
                        arguments: serde_json::json!({ "path": probe.to_string_lossy() }),
                    }],
                },
                prompt_tokens: Some(42),
                output_tokens: Some(5),
            },
            ChatResponse {
                message: AssistantMessage {
                    content: "The file says: hello wilai".into(),
                    tool_calls: vec![],
                },
                prompt_tokens: Some(60),
                output_tokens: Some(10),
            },
        ],
    );
    let provider: Arc<MockProvider> = Arc::new(provider);
    let confirmer = FixedConfirmer(ConfirmAnswer::Yes);

    let (tx, rx) = mpsc::channel::<ServerEvent>(32);
    let system_prompt = build_system_prompt(&service.registry, None);
    let mut history = Vec::new();

    run_turn(
        &service,
        "sess_test",
        &system_prompt,
        &mut history,
        "read probe.txt please".to_string(),
        &*provider,
        "mock-1",
        &confirmer,
        &tx,
    )
    .await
    .expect("run_turn ok");
    drop(tx);

    let events = drain_events(rx).await;

    // Must have called the provider twice (once for the tool call, once
    // for the final text).
    assert_eq!(provider.call_count(), 2, "events: {events:#?}");

    // Must have a ToolCall and ToolResult for fs.read in order.
    let tool_call_idx = events
        .iter()
        .position(|e| matches!(e, ServerEvent::ToolCall { name, .. } if name == "fs.read"));
    let tool_result_idx = events
        .iter()
        .position(|e| matches!(e, ServerEvent::ToolResult { name, .. } if name == "fs.read"));
    assert!(tool_call_idx.is_some(), "no ToolCall for fs.read in {events:#?}");
    assert!(tool_result_idx.is_some(), "no ToolResult for fs.read in {events:#?}");
    assert!(tool_call_idx.unwrap() < tool_result_idx.unwrap());

    // Tool result output must contain the file contents.
    let text_result = events.iter().find_map(|e| match e {
        ServerEvent::ToolResult { name, ok, output } if name == "fs.read" => {
            Some((*ok, output.clone()))
        }
        _ => None,
    });
    let (ok, output) = text_result.unwrap();
    assert!(ok, "fs.read tool result not ok");
    assert!(output.contains("hello wilai"), "fs.read output missing payload: {output}");

    // Final event is a Text with the model's second-turn summary.
    let final_text = events.iter().rev().find_map(|e| match e {
        ServerEvent::Text { content } => Some(content.clone()),
        _ => None,
    });
    assert_eq!(final_text.as_deref(), Some("The file says: hello wilai"));
}

#[tokio::test]
async fn agent_loop_handles_unknown_tool() {
    let (service, _tmp) = in_process_service().await;

    // Script: tool call for an unknown tool, then a fallback text reply.
    // The loop should audit a ToolDeny, bubble back as "error:" tool
    // message in the next request, and still complete the turn.
    let provider = MockProvider::with_script(
        "mock",
        vec![
            ChatResponse {
                message: AssistantMessage {
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "x".into(),
                        name: "no.such.tool".into(),
                        arguments: serde_json::json!({}),
                    }],
                },
                prompt_tokens: Some(1),
                output_tokens: Some(1),
            },
            ChatResponse {
                message: AssistantMessage {
                    content: "sorry, I can't do that".into(),
                    tool_calls: vec![],
                },
                prompt_tokens: Some(1),
                output_tokens: Some(1),
            },
        ],
    );
    let provider = Arc::new(provider);
    let confirmer = FixedConfirmer(ConfirmAnswer::No);

    let (tx, rx) = mpsc::channel::<ServerEvent>(32);
    let sys = build_system_prompt(&service.registry, None);
    let mut history = Vec::new();

    run_turn(
        &service,
        "sess_test",
        &sys,
        &mut history,
        "try the missing tool".to_string(),
        &*provider,
        "mock-1",
        &confirmer,
        &tx,
    )
    .await
    .expect("run_turn ok");
    drop(tx);

    let events = drain_events(rx).await;
    // Should have surfaced a tool error, then recovered with a text.
    let got_tool_err = events
        .iter()
        .any(|e| matches!(e, ServerEvent::ToolResult { ok: false, name, .. } if name == "no.such.tool"));
    assert!(got_tool_err, "expected ToolResult ok=false for unknown tool: {events:#?}");

    let final_text = events.iter().rev().find_map(|e| match e {
        ServerEvent::Text { content } => Some(content.clone()),
        _ => None,
    });
    assert_eq!(final_text.as_deref(), Some("sorry, I can't do that"));
}
