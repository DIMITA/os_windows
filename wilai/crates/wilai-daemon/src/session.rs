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
    // Resolve the session role from the peer uid of the socket. RBAC
    // is opt-in: when rbac.enabled is false, every session is admin,
    // which matches the pre-v5.2 behavior.
    let role: String = if service.cfg.rbac.enabled {
        match stream.peer_cred() {
            Ok(cred) => {
                let uid = cred.uid();
                let r = service.cfg.rbac.role_for(uid);
                let normalized = crate::rbac::normalize_role(r);
                tracing::info!(uid, role = normalized, "session role resolved");
                normalized.to_string()
            }
            Err(e) => {
                tracing::warn!("peer_cred failed: {e}; falling back to guest");
                "guest".to_string()
            }
        }
    } else {
        "admin".to_string()
    };
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
    // Subscribe to tool registry changes (backfill, manual McpResync).
    spawn_tools_relay(
        service.tools_changed.clone(),
        service.registry.clone(),
        event_tx.clone(),
    );
    // Subscribe to proactive nudges and forward them as ServerEvent::Nudge.
    spawn_nudge_relay(
        service.nudges.subscribe(),
        service.audit_tx.clone(),
        session_id.clone(),
        service.mode.current().await,
        event_tx.clone(),
    );

    // Reader loop: parse client ops; for ConfirmAnswer, deliver via pending; for
    // Prompt, run the agent loop; for Quit, break.
    let confirmer: Box<dyn Confirmer> = Box::new(IpcConfirmer {
        event_tx: event_tx.clone(),
        pending: pending.clone(),
    });
    let mut history: Vec<Message> = Vec::new();
    // Track (provider_name, model) pairs this session has loaded, so we
    // can tell Ollama to drop them from RAM when the session ends.
    let mut touched_models: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let system_prompt = {
        let reg = service.registry.read().await;
        build_system_prompt_with_persona(
            &reg,
            service.cfg.general.system_prompt.as_deref(),
            &service.cfg.persona,
        )
    };

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
                let reg = service.registry.read().await;
                let tools: Vec<crate::protocol::ToolSummary> = reg
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
            ClientOp::McpList => {
                let servers = build_mcp_snapshot(&service).await;
                let _ = event_tx.send(ServerEvent::McpServers { servers }).await;
            }
            ClientOp::McpRestart { server } => {
                match service.mcp.get(&server) {
                    Some(sup) => {
                        sup.restart_now();
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        let servers = build_mcp_snapshot(&service).await;
                        let _ = event_tx.send(ServerEvent::McpServers { servers }).await;
                    }
                    None => {
                        let _ = event_tx
                            .send(ServerEvent::Error {
                                message: format!("mcp server `{server}` not configured"),
                            })
                            .await;
                    }
                }
            }
            ClientOp::McpResync => {
                let added = crate::service::backfill_mcp_tools(
                    &service.cfg,
                    &service.registry,
                    &service.mcp,
                )
                .await;
                tracing::info!(count = added.len(), "mcp resync");
                if !added.is_empty() {
                    service.tools_changed.notify_waiters();
                }
                let servers = build_mcp_snapshot(&service).await;
                let _ = event_tx.send(ServerEvent::McpServers { servers }).await;
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
                touched_models.insert((provider_name.clone(), model.clone()));
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
                if let Err(e) = run_turn_with_role(
                    &service,
                    &session_id,
                    &system_prompt,
                    &mut history,
                    text,
                    &*provider,
                    &model,
                    &*confirmer,
                    &event_tx,
                    &role,
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

    // Session ended — free any Ollama models this session loaded, so
    // the VM gets its RAM back instead of waiting for OLLAMA_KEEP_ALIVE
    // (5 min by default). Best-effort, bounded by a 5s timeout each.
    for (provider_name, model) in touched_models {
        service.unload_model_from_provider(&provider_name, &model).await;
    }

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

pub fn build_system_prompt(registry: &wilai_tools::Registry, user_extra: Option<&str>) -> String {
    build_system_prompt_with_persona(registry, user_extra, &wilai_core::PersonaConfig::default())
}

pub fn build_system_prompt_with_persona(
    registry: &wilai_tools::Registry,
    user_extra: Option<&str>,
    persona: &wilai_core::PersonaConfig,
) -> String {
    let mut s = String::new();
    let addr = if persona.address.is_empty() {
        "the user".to_string()
    } else {
        persona.address.clone()
    };
    if persona.style == "concise_butler" {
        s.push_str(&format!(
            "You are Wilai, a local system agent on WilOS Aurora. You address {addr} \
             with the composure of a long-serving butler: precise, concise, deferential \
             without being obsequious. Prefer one or two short sentences. State what you \
             are about to do before doing it, state what you did after. If a tool fits, \
             use it; never invent commands. All your providers run on the host — no \
             cloud. You have access to a persistent local memory: `memory.note` to \
             record facts worth remembering, `memory.recall` to search by meaning, \
             `memory.fact_set` / `memory.fact_get` for typed preferences. Use it \
             naturally; part of your value is that you remember.\n\n"
        ));
    } else {
        s.push_str(
            "You are Wilai, a system agent on WilOS Aurora. You assist by calling \
             typed tools that the host serializes for you. Never produce raw shell \
             commands; if no tool fits, say so. Be concise.\n\n",
        );
    }
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

pub async fn run_turn(
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
    run_turn_with_role(
        service, session_id, system_prompt, history, user_text,
        provider, model, confirmer, event_tx, "admin",
    ).await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_turn_with_role(
    service: &Service,
    session_id: &str,
    system_prompt: &str,
    history: &mut Vec<Message>,
    user_text: String,
    provider: &dyn Provider,
    model: &str,
    confirmer: &dyn Confirmer,
    event_tx: &mpsc::Sender<ServerEvent>,
    role: &str,
) -> Result<()> {
    history.push(Message {
        role: MessageRole::User,
        content: user_text,
        tool_call_id: None,
        tool_calls: vec![],
    });

    let tool_schemas: Vec<wilai_providers::ToolSchema> = {
        let reg = service.registry.read().await;
        reg.iter()
            .filter(|(_, spec)| crate::rbac::role_allows(role, spec))
            .map(|(_, spec)| schema_for(spec))
            .collect()
    };

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
            let result = execute_call_with_role(role,
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
#[allow(clippy::too_many_arguments)]
async fn execute_call_with_role(
    role: &str,
    tc: &ToolCall,
    service: &Service,
    session: &str,
    provider_name: &str,
    model: &str,
    turn_id: &str,
    confirmer: &dyn Confirmer,
    confirm_timeout_s: u32,
) -> Result<String> {
    let spec = {
        let reg = service.registry.read().await;
        reg.get(&tc.name)
            .cloned()
            .ok_or_else(|| anyhow!("unknown tool: {}", tc.name))?
    };
    let spec = &spec;
    let mode_now = service.mode.current().await;

    // RBAC check: even if the LLM found the tool in its schema list,
    // double-check at execution time. The LLM could have remembered a
    // tool from the system prompt and tried to call it anyway.
    if !crate::rbac::role_allows(role, spec) {
        let reason = format!("role `{role}` is not allowed to call `{}`", spec.name);
        let _ = service
            .audit_tx
            .send(WriteRequest {
                session: session.to_string(),
                mode: mode_now,
                payload: EntryPayload::ToolDeny {
                    tool_name: spec.name.clone(),
                    args: tc.arguments.clone(),
                    reason: reason.clone(),
                    guard: format!("rbac/{role}"),
                },
            })
            .await;
        return Err(anyhow!(reason));
    }

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
    // Route MCP-prefixed tools to their backing client; memory.*
    // tools to the local SQLite store; everything else goes through
    // the local Executor.
    let result_outcome = if let Some((server, mcp_tool)) = parse_mcp_name(&spec.name) {
        run_mcp_tool(service, server, mcp_tool, &validated).await
    } else if spec.name.starts_with("memory.") {
        run_memory_tool(service, &spec.name, &validated).await
    } else if spec.name.starts_with("pt.") {
        crate::pentest_tool::run(service, &spec.name, &validated)
            .await
            .with_context(|| format!("execute {}", spec.name))
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

async fn run_memory_tool(
    service: &Service,
    name: &str,
    args: &Value,
) -> Result<wilai_tools::ExecResult> {
    use std::time::Instant;
    let store = service
        .memory
        .as_ref()
        .ok_or_else(|| anyhow!("memory not configured"))?;
    let embedder = service
        .embedder
        .as_ref()
        .ok_or_else(|| anyhow!("memory embedder not configured"))?;
    let start = Instant::now();

    let text = match name {
        "memory.note" => {
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("memory.note: content missing"))?;
            let tags: Vec<String> = args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let session = args.get("session").and_then(|v| v.as_str());
            let id = store.note(session, content, &tags, embedder.as_ref()).await?;
            format!("recorded note {id}")
        }
        "memory.recall" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("memory.recall: query missing"))?;
            let k = args.get("k").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
            let hits = store.recall(query, k, embedder.as_ref()).await?;
            if hits.is_empty() {
                "no matches".to_string()
            } else {
                let mut out = String::new();
                for h in hits {
                    out.push_str(&format!(
                        "- [{:.2}] ({}) {}\n",
                        h.score,
                        h.note.created_at,
                        h.note.content
                    ));
                }
                out
            }
        }
        "memory.fact_set" => {
            let key = args
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("memory.fact_set: key missing"))?;
            let value = args
                .get("value")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("memory.fact_set: value missing"))?;
            store.fact_set(key, value)?;
            format!("set {key}")
        }
        "memory.fact_get" => {
            if let Some(key) = args.get("key").and_then(|v| v.as_str()) {
                match store.fact_get(key)? {
                    Some(f) => format!("{} = {} (updated {})", f.key, f.value, f.updated_at),
                    None => format!("(no fact for {key})"),
                }
            } else if let Some(prefix) = args.get("prefix").and_then(|v| v.as_str()) {
                let facts = store.fact_list(Some(prefix))?;
                if facts.is_empty() {
                    "(no facts)".to_string()
                } else {
                    facts.iter().map(|f| format!("{} = {}", f.key, f.value)).collect::<Vec<_>>().join("\n")
                }
            } else {
                return Err(anyhow!("memory.fact_get: pass `key` or `prefix`"));
            }
        }
        other => return Err(anyhow!("unknown memory tool: {other}")),
    };

    let duration_ms = start.elapsed().as_millis() as u64;
    let total = text.len();
    Ok(wilai_tools::ExecResult {
        exit_code: 0,
        output: text,
        output_bytes: total,
        truncated: false,
        duration_ms,
        rendered_cmd: Some(vec![format!("memory:{name}")]),
    })
}

async fn build_mcp_snapshot(service: &Service) -> Vec<crate::protocol::McpServerInfo> {
    let mut out = Vec::with_capacity(service.mcp.len());
    for (name, sup) in &service.mcp {
        let status = sup.status().await;
        let gated = service
            .cfg
            .mcp
            .get(name)
            .map(|c| c.gate_in_pentest)
            .unwrap_or(false);
        out.push(crate::protocol::McpServerInfo {
            name: status.name,
            alive: status.alive,
            uptime_s: status.uptime_s,
            restart_count: status.restart_count,
            tool_count: status.tool_count,
            last_error: status.last_error,
            gated_in_pentest: gated,
            fatal: status.fatal,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

async fn run_mcp_tool(
    service: &Service,
    server: &str,
    tool: &str,
    args: &Value,
) -> Result<wilai_tools::ExecResult> {
    use std::time::Instant;
    let sup = service
        .mcp
        .get(server)
        .ok_or_else(|| anyhow!("mcp server `{server}` not registered"))?;
    let start = Instant::now();
    let res = sup.call_tool(tool, args.clone()).await?;
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
    let truncated = false;
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

fn spawn_nudge_relay(
    mut rx: tokio::sync::broadcast::Receiver<crate::nudge::Nudge>,
    audit_tx: mpsc::Sender<WriteRequest>,
    session: String,
    mode: Mode,
    event_tx: mpsc::Sender<ServerEvent>,
) {
    tokio::spawn(async move {
        loop {
            let n = match rx.recv().await {
                Ok(n) => n,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!(skipped, "nudge relay lagged");
                    continue;
                }
                Err(_) => break,
            };
            // Audit the emission so the hash chain reflects the
            // daemon's proactive output just like it does for tool runs.
            let _ = audit_tx
                .send(WriteRequest {
                    session: session.clone(),
                    mode,
                    payload: EntryPayload::Nudge {
                        kind: n.kind.clone(),
                        severity: n.severity.as_str().to_string(),
                        message: n.message.clone(),
                        context: n.context.clone(),
                    },
                })
                .await;
            let ev = ServerEvent::Nudge {
                id: n.id.clone(),
                kind: n.kind.clone(),
                severity: n.severity.as_str().to_string(),
                message: n.message.clone(),
                context: n.context.clone(),
            };
            if event_tx.send(ev).await.is_err() {
                break;
            }
        }
    });
}

fn spawn_tools_relay(
    notify: std::sync::Arc<tokio::sync::Notify>,
    registry: std::sync::Arc<tokio::sync::RwLock<wilai_tools::Registry>>,
    event_tx: mpsc::Sender<ServerEvent>,
) {
    tokio::spawn(async move {
        // Track names we have already announced so re-fires only push
        // the delta. A dedicated set is cheaper than resending the full
        // list every time.
        let mut known: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        {
            let reg = registry.read().await;
            for (name, _) in reg.iter() {
                known.insert(name.to_string());
            }
        }
        loop {
            notify.notified().await;
            let current: std::collections::BTreeSet<String> = {
                let reg = registry.read().await;
                reg.iter().map(|(n, _)| n.to_string()).collect()
            };
            let fresh: Vec<String> = current.difference(&known).cloned().collect();
            if fresh.is_empty() {
                continue;
            }
            known = current;
            if event_tx
                .send(ServerEvent::ToolsAdded { names: fresh })
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
