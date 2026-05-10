use crate::ipc::{IpcConfirmer, PendingConfirms};
use crate::protocol::{ClientOp, ServerEvent};
use crate::service::Service;
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use wilai_audit::entry::EntryPayload;
use wilai_audit::writer::WriteRequest;
use wilai_core::types::new_ulid;
use wilai_core::{Category, Mode};
use wilai_providers::{ChatRequest, Message, MessageRole, Provider, ToolCall};
use wilai_tools::confirm::{ConfirmAnswer, ConfirmDefault, Confirmer};
use wilai_tools::guards::{evaluate, GuardOutcome};
use wilai_tools::Executor;

const MAX_TOOL_TURNS: usize = 8;
const MAX_REPAIR_TURNS: usize = 2;

pub async fn run_session(stream: UnixStream, service: Arc<Service>) -> Result<()> {
    let (read_half, write_half) = stream.into_split();
    let (event_tx, mut event_rx) = mpsc::channel::<ServerEvent>(32);
    let pending = Arc::new(PendingConfirms::default());

    let mut writer = write_half;
    let writer_pending = pending.clone();

    // Writer task: drains events and writes them to the socket.
    let writer_task = tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            let line = match serde_json::to_vec(&ev) {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!("encode event: {e}");
                    continue;
                }
            };
            if writer.write_all(&line).await.is_err() {
                break;
            }
            if writer.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    // Session bookkeeping: announce session id; ensure session.start audit.
    let session_id = new_ulid();
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "/".to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    let _ = service
        .audit_tx
        .send(WriteRequest {
            session: session_id.clone(),
            mode: service.mode.current().await,
            payload: EntryPayload::SessionStart {
                entry: "wilai-daemon".to_string(),
                cwd,
                user,
            },
        })
        .await;
    let _ = event_tx
        .send(ServerEvent::SessionStart {
            session: session_id.clone(),
        })
        .await;
    // Subscribe to mode changes so this connection learns about auto-switches
    // happening elsewhere in the daemon.
    let mode_changes = service.mode.subscribe().await;
    spawn_mode_relay(mode_changes, event_tx.clone(), service.mode.clone());

    // Reader loop: parse client ops; for ConfirmAnswer, deliver via pending; for
    // Prompt, run the agent loop; for Quit, break.
    let confirmer: Box<dyn Confirmer> = Box::new(IpcConfirmer {
        event_tx: event_tx.clone(),
        pending: pending.clone(),
    });
    let mut history: Vec<Message> = Vec::new();
    let system_prompt = build_system_prompt(&service.registry, service.cfg.general.system_prompt.as_deref());

    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    let result: Result<()> = loop {
        line.clear();
        let n = match reader.read_line(&mut line).await {
            Ok(n) => n,
            Err(e) => break Err(e.into()),
        };
        if n == 0 {
            break Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let op: ClientOp = match serde_json::from_str(trimmed) {
            Ok(o) => o,
            Err(e) => {
                let _ = event_tx
                    .send(ServerEvent::Error {
                        message: format!("bad op: {e}"),
                    })
                    .await;
                continue;
            }
        };

        match op {
            ClientOp::Quit => break Ok(()),
            ClientOp::ConfirmAnswer { id, answer } => {
                let delivered = pending.deliver(&id, answer);
                if !delivered {
                    let _ = event_tx
                        .send(ServerEvent::Error {
                            message: format!("no pending confirm with id {id}"),
                        })
                        .await;
                }
            }
            ClientOp::ModeGet => {
                let cur = service.mode.current().await;
                let _ = event_tx
                    .send(ServerEvent::Mode {
                        current: cur.to_string(),
                        pentest_in_flight: service.mode.pentest_in_flight(),
                    })
                    .await;
            }
            ClientOp::ToolsList => {
                let tools: Vec<crate::protocol::ToolSummary> = service
                    .registry
                    .iter()
                    .map(|(name, spec)| crate::protocol::ToolSummary {
                        name: name.to_string(),
                        version: spec.version,
                        category: spec.category.to_string(),
                        risk: spec.risk.to_string(),
                        description: spec.description.clone(),
                        origin: if name.starts_with("mcp.") {
                            "mcp".to_string()
                        } else {
                            "yaml".to_string()
                        },
                    })
                    .collect();
                let _ = event_tx.send(ServerEvent::Tools { tools }).await;
            }
            ClientOp::ModeSet { to, trigger } => {
                let target: Mode = match to.parse() {
                    Ok(m) => m,
                    Err(e) => {
                        let _ = event_tx
                            .send(ServerEvent::Error {
                                message: format!("bad mode: {e}"),
                            })
                            .await;
                        continue;
                    }
                };
                let trig = trigger.unwrap_or_else(|| "manual".to_string());
                match service.mode.set(target, &trig).await {
                    Ok(_) => {
                        let _ = event_tx
                            .send(ServerEvent::Mode {
                                current: service.mode.current().await.to_string(),
                                pentest_in_flight: service.mode.pentest_in_flight(),
                            })
                            .await;
                    }
                    Err(e) => {
                        let _ = event_tx
                            .send(ServerEvent::Error {
                                message: format!("{e}"),
                            })
                            .await;
                    }
                }
            }
            ClientOp::Prompt { text, model, provider } => {
                let model = model.unwrap_or(service.default_model.clone());
                let provider_name = provider.unwrap_or(service.default_provider.clone());
                let provider = match service.build_provider(&provider_name).await {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = event_tx
                            .send(ServerEvent::Error {
                                message: format!("provider: {e}"),
                            })
                            .await;
                        continue;
                    }
                };
                if let Err(e) = run_turn(
                    &service,
                    &session_id,
                    &system_prompt,
                    &mut history,
                    text,
                    &*provider,
                    &model,
                    &*confirmer,
                    &event_tx,
                )
                .await
                {
                    let _ = event_tx
                        .send(ServerEvent::Error {
                            message: format!("{e:#}"),
                        })
                        .await;
                }
                let _ = event_tx.send(ServerEvent::TurnDone).await;
            }
        }
    };

    let _ = service
        .audit_tx
        .send(WriteRequest {
            session: session_id,
            mode: service.mode.current().await,
            payload: EntryPayload::SessionEnd,
        })
        .await;
    let _ = event_tx.send(ServerEvent::Bye).await;
    drop(event_tx);
    let _ = writer_task.await;
    let _ = writer_pending; // keep alive to end of fn
    result
}

fn build_system_prompt(registry: &wilai_tools::Registry, user_extra: Option<&str>) -> String {
    let mut s = String::new();
    s.push_str(
        "You are Wilai, a system agent on WilOS Aurora. You assist by calling \
         typed tools that the host serializes for you. Never produce raw shell \
         commands; if no tool fits, say so. Be concise.\n\n",
    );
    s.push_str("Available tools:\n");
    for (name, spec) in registry.iter() {
        s.push_str(&format!("- {name} (v{}): {}\n", spec.version, spec.description));
    }
    if let Some(extra) = user_extra {
        s.push_str("\n");
        s.push_str(extra);
    }
    s
}

async fn run_turn(
    service: &Service,
    session_id: &str,
    system_prompt: &str,
    history: &mut Vec<Message>,
    user_text: String,
    provider: &dyn Provider,
    model: &str,
    confirmer: &dyn Confirmer,
    event_tx: &mpsc::Sender<ServerEvent>,
) -> Result<()> {
    history.push(Message {
        role: MessageRole::User,
        content: user_text,
        tool_call_id: None,
        tool_calls: vec![],
    });

    let tool_schemas: Vec<wilai_providers::ToolSchema> = service
        .registry
        .iter()
        .map(|(_, spec)| schema_for(spec))
        .collect();

    let turn_id = format!("t{}", history.len());
    let mut repair_left = MAX_REPAIR_TURNS;

    for hop in 0..MAX_TOOL_TURNS {
        let req = ChatRequest {
            model: model.to_string(),
            system: Some(system_prompt.to_string()),
            messages: history.clone(),
            tools: tool_schemas.clone(),
            max_tokens: None,
        };
        let start = Instant::now();
        let resp = provider.chat(req).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        let mode_now = service.mode.current().await;
        let resp = match resp {
            Ok(r) => {
                let _ = service
                    .audit_tx
                    .send(WriteRequest {
                        session: session_id.to_string(),
                        mode: mode_now,
                        payload: EntryPayload::ProviderCall {
                            provider: provider.name().to_string(),
                            model: model.to_string(),
                            prompt_tokens: r.prompt_tokens,
                            output_tokens: r.output_tokens,
                            duration_ms,
                            n_tools: tool_schemas.len() as u32,
                            n_messages: history.len() as u32,
                            error: None,
                        },
                    })
                    .await;
                r
            }
            Err(e) => {
                let _ = service
                    .audit_tx
                    .send(WriteRequest {
                        session: session_id.to_string(),
                        mode: mode_now,
                        payload: EntryPayload::ProviderCall {
                            provider: provider.name().to_string(),
                            model: model.to_string(),
                            prompt_tokens: None,
                            output_tokens: None,
                            duration_ms,
                            n_tools: tool_schemas.len() as u32,
                            n_messages: history.len() as u32,
                            error: Some(format!("{e:#}")),
                        },
                    })
                    .await;
                return Err(e);
            }
        };

        if resp.message.tool_calls.is_empty() {
            if !resp.message.content.is_empty() {
                let _ = event_tx
                    .send(ServerEvent::Text {
                        content: resp.message.content.clone(),
                    })
                    .await;
            }
            history.push(Message {
                role: MessageRole::Assistant,
                content: resp.message.content,
                tool_call_id: None,
                tool_calls: vec![],
            });
            return Ok(());
        }

        history.push(Message {
            role: MessageRole::Assistant,
            content: resp.message.content.clone(),
            tool_call_id: None,
            tool_calls: resp.message.tool_calls.clone(),
        });

        let mut any_invalid = false;
        for tc in &resp.message.tool_calls {
            let _ = event_tx
                .send(ServerEvent::ToolCall {
                    name: tc.name.clone(),
                    args: tc.arguments.clone(),
                })
                .await;
            let result = execute_call(
                tc,
                service,
                session_id,
                provider.name(),
                model,
                &turn_id,
                confirmer,
                service.confirm_timeout_s,
            )
            .await;
            match result {
                Ok(text) => {
                    let _ = event_tx
                        .send(ServerEvent::ToolResult {
                            name: tc.name.clone(),
                            ok: true,
                            output: text.clone(),
                        })
                        .await;
                    history.push(Message {
                        role: MessageRole::Tool,
                        content: text,
                        tool_call_id: Some(tc.id.clone()),
                        tool_calls: vec![],
                    });
                }
                Err(e) => {
                    any_invalid = true;
                    let msg = format!("error: {e:#}");
                    let _ = event_tx
                        .send(ServerEvent::ToolResult {
                            name: tc.name.clone(),
                            ok: false,
                            output: msg.clone(),
                        })
                        .await;
                    history.push(Message {
                        role: MessageRole::Tool,
                        content: msg,
                        tool_call_id: Some(tc.id.clone()),
                        tool_calls: vec![],
                    });
                }
            }
        }

        if any_invalid {
            if repair_left == 0 {
                return Err(anyhow!("repeated invalid tool calls; aborting turn"));
            }
            repair_left -= 1;
        }

        if hop + 1 == MAX_TOOL_TURNS {
            return Err(anyhow!("tool-call hop limit reached ({MAX_TOOL_TURNS})"));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn execute_call(
    tc: &ToolCall,
    service: &Service,
    session: &str,
    provider_name: &str,
    model: &str,
    turn_id: &str,
    confirmer: &dyn Confirmer,
    confirm_timeout_s: u32,
) -> Result<String> {
    let spec = service
        .registry
        .get(&tc.name)
        .ok_or_else(|| anyhow!("unknown tool: {}", tc.name))?;
    let mode_now = service.mode.current().await;

    let validated = match wilai_tools::validator::validate_args(spec, &tc.arguments) {
        Ok(v) => v,
        Err(e) => {
            let reason = format!("validation failed: {e}");
            let _ = service
                .audit_tx
                .send(WriteRequest {
                    session: session.to_string(),
                    mode: mode_now,
                    payload: EntryPayload::ToolDeny {
                        tool_name: tc.name.clone(),
                        args: tc.arguments.clone(),
                        reason: reason.clone(),
                        guard: "validator".to_string(),
                    },
                })
                .await;
            return Err(anyhow!(reason));
        }
    };

    match evaluate(spec, &validated, mode_now)? {
        GuardOutcome::Allow => {}
        GuardOutcome::Deny { reason, guard } => {
            let _ = service
                .audit_tx
                .send(WriteRequest {
                    session: session.to_string(),
                    mode: mode_now,
                    payload: EntryPayload::ToolDeny {
                        tool_name: spec.name.clone(),
                        args: validated.clone(),
                        reason: reason.clone(),
                        guard,
                    },
                })
                .await;
            return Err(anyhow!(reason));
        }
        GuardOutcome::Confirm { prompt, default_no, guard: _ } => {
            let default = if default_no {
                ConfirmDefault::No
            } else {
                ConfirmDefault::Yes
            };
            let start = Instant::now();
            let answer = confirmer.ask(&prompt, default, confirm_timeout_s).await?;
            let latency_ms = start.elapsed().as_millis() as u64;
            let answer_str = match answer {
                ConfirmAnswer::Yes => "yes",
                ConfirmAnswer::No => "no",
                ConfirmAnswer::Timeout => "timeout",
            };
            let _ = service
                .audit_tx
                .send(WriteRequest {
                    session: session.to_string(),
                    mode: mode_now,
                    payload: EntryPayload::ToolConfirm {
                        tool_name: spec.name.clone(),
                        args: validated.clone(),
                        prompt: prompt.clone(),
                        answer: answer_str.to_string(),
                        latency_ms,
                    },
                })
                .await;
            if !matches!(answer, ConfirmAnswer::Yes) {
                return Err(anyhow!("confirmation refused: {prompt}"));
            }
        }
    }

    // Track pentest tools in flight so we can refuse exit-from-pentest
    // while one is running.
    let is_pentest_tool = matches!(spec.category, Category::Pentest);
    if is_pentest_tool {
        service.mode.pentest_started();
    }
    // Route MCP-prefixed tools to their backing client; everything else
    // goes through the local Executor.
    let result_outcome = if let Some((server, mcp_tool)) = parse_mcp_name(&spec.name) {
        run_mcp_tool(service, server, mcp_tool, &validated).await
    } else {
        Executor::run(spec, &validated)
            .await
            .with_context(|| format!("execute {}", spec.name))
    };
    if is_pentest_tool {
        service.mode.pentest_finished();
    }
    let result = result_outcome?;

    let exec_kind = match &spec.executor {
        wilai_tools::ExecutorSpec::Subprocess(_) => "subprocess",
        wilai_tools::ExecutorSpec::Builtin(_) => "builtin",
    };

    // In pentest mode the audit log is verbose: full args (no field filter)
    // and a larger output sample for forensic replay.
    let pentest_mode = mode_now == Mode::Pentest;
    let logged_args = if pentest_mode {
        validated.clone()
    } else {
        filter_args(&validated, spec)
    };
    let sample_limit = if pentest_mode { 65536 } else { 1024 };
    let sample = if result.output.is_empty() {
        None
    } else {
        let limit = result.output.len().min(sample_limit);
        Some(safe_slice(&result.output, limit).to_string())
    };

    let _ = service
        .audit_tx
        .send(WriteRequest {
            session: session.to_string(),
            mode: mode_now,
            payload: EntryPayload::ToolExec {
                tool_name: spec.name.clone(),
                tool_version: spec.version,
                category: spec.category.to_string(),
                risk: spec.risk.to_string(),
                args: logged_args,
                executor: exec_kind.to_string(),
                cmd: result.rendered_cmd.clone(),
                exit_code: result.exit_code,
                duration_ms: result.duration_ms,
                output_bytes: result.output_bytes,
                output_truncated: result.truncated,
                output_sample: sample,
                provider: provider_name.to_string(),
                model: model.to_string(),
                turn_id: turn_id.to_string(),
                tool_call_id: tc.id.clone(),
            },
        })
        .await;

    Ok(result.output)
}

fn parse_mcp_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("mcp.")?;
    let dot = rest.find('.')?;
    Some((&rest[..dot], &rest[dot + 1..]))
}

async fn run_mcp_tool(
    service: &Service,
    server: &str,
    tool: &str,
    args: &Value,
) -> Result<wilai_tools::ExecResult> {
    use std::time::Instant;
    let client = service
        .mcp
        .get(server)
        .ok_or_else(|| anyhow!("mcp server `{server}` not registered"))?;
    let start = Instant::now();
    let res = client.call_tool(tool, args.clone()).await?;
    let duration_ms = start.elapsed().as_millis() as u64;
    let mut text = String::new();
    for block in &res.content {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&block.render());
    }
    let exit_code = if res.is_error { 1 } else { 0 };
    let total = text.len();
    let truncated = false; // mcp content already serialized; left as-is
    Ok(wilai_tools::ExecResult {
        exit_code,
        output: text,
        output_bytes: total,
        truncated,
        duration_ms,
        rendered_cmd: Some(vec![format!("mcp:{server}.{tool}")]),
    })
}

fn spawn_mode_relay(
    mut rx: mpsc::Receiver<crate::mode_mgr::ModeChange>,
    event_tx: mpsc::Sender<ServerEvent>,
    mode: std::sync::Arc<crate::mode_mgr::ModeManager>,
) {
    tokio::spawn(async move {
        while let Some(_change) = rx.recv().await {
            let cur = mode.current().await;
            if event_tx
                .send(ServerEvent::Mode {
                    current: cur.to_string(),
                    pentest_in_flight: mode.pentest_in_flight(),
                })
                .await
                .is_err()
            {
                break;
            }
        }
    });
}

fn filter_args(args: &Value, spec: &wilai_tools::ToolSpec) -> Value {
    if spec.audit.fields.is_empty() {
        return args.clone();
    }
    let obj = match args.as_object() {
        Some(o) => o,
        None => return args.clone(),
    };
    let mut out = serde_json::Map::new();
    for (k, v) in obj {
        if spec.audit.fields.iter().any(|f| f == k) {
            out.insert(k.clone(), v.clone());
        } else {
            out.insert(
                k.clone(),
                Value::String(format!("<{} len={}>", type_name(v), value_len(v))),
            );
        }
    }
    Value::Object(out)
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn value_len(v: &Value) -> usize {
    match v {
        Value::String(s) => s.len(),
        Value::Array(a) => a.len(),
        Value::Object(o) => o.len(),
        _ => 0,
    }
}

fn safe_slice(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn schema_for(spec: &wilai_tools::ToolSpec) -> wilai_providers::ToolSchema {
    use wilai_tools::spec::InputType;
    let mut props = serde_json::Map::new();
    let mut required = Vec::new();
    for (name, ispec) in &spec.inputs {
        let mut prop = serde_json::Map::new();
        let ty = match ispec.kind {
            InputType::String | InputType::Enum => "string",
            InputType::Integer => "integer",
            InputType::Number => "number",
            InputType::Boolean => "boolean",
            InputType::Array => "array",
            InputType::Object => "object",
        };
        prop.insert("type".into(), Value::String(ty.into()));
        if let Some(desc) = &ispec.description {
            prop.insert("description".into(), Value::String(desc.clone()));
        }
        if let Some(values) = &ispec.values {
            prop.insert("enum".into(), Value::Array(values.clone()));
        }
        if matches!(ispec.kind, InputType::Array) {
            if let Some(items) = &ispec.items {
                let item_ty = match items.kind {
                    InputType::String | InputType::Enum => "string",
                    InputType::Integer => "integer",
                    InputType::Number => "number",
                    InputType::Boolean => "boolean",
                    _ => "string",
                };
                prop.insert(
                    "items".into(),
                    serde_json::json!({ "type": item_ty }),
                );
            }
        }
        props.insert(name.clone(), Value::Object(prop));
        if ispec.required {
            required.push(Value::String(name.clone()));
        }
    }

    wilai_providers::ToolSchema {
        name: spec.name.clone(),
        description: spec.description.clone(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": Value::Object(props),
            "required": Value::Array(required),
        }),
    }
}
