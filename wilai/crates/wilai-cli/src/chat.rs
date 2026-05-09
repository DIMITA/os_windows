use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::time::Instant;
use wilai_audit::entry::EntryPayload;
use wilai_audit::writer::{AuditWriter, WriteRequest};
use wilai_core::{types::new_ulid, Mode};
use wilai_providers::{
    AnthropicProvider, ChatRequest, Message, MessageRole, OllamaProvider, Provider, ToolCall,
};
use wilai_tools::confirm::{ConfirmAnswer, ConfirmDefault, Confirmer, TtyConfirmer};
use wilai_tools::guards::{evaluate, GuardOutcome};
use wilai_tools::{Executor, Registry};

const MAX_TOOL_TURNS: usize = 8;
const MAX_REPAIR_TURNS: usize = 2;

pub async fn run(once: Option<String>, model: Option<String>, provider: Option<String>) -> Result<()> {
    let cfg = wilai_core::Config::load()?;
    let dirs = crate::collect_tool_dirs(&cfg);
    let registry = Registry::load_from_dirs(&dirs, &cfg.tools.disabled)?;
    tracing::info!("loaded {} tools", registry.len());

    let provider_name = provider.unwrap_or(cfg.general.default_provider.clone());
    let model = model.unwrap_or(cfg.general.default_model.clone());
    let provider = build_provider(&provider_name, &cfg)?;
    let confirmer: Box<dyn Confirmer> = Box::new(TtyConfirmer);
    let confirm_timeout_s = cfg.general.confirm_timeout_s;

    let audit_dir = cfg
        .audit
        .dir
        .clone()
        .map(Ok)
        .unwrap_or_else(wilai_core::paths::audit_dir)?;
    let mut audit = AuditWriter::open(&audit_dir)
        .with_context(|| format!("open audit dir {}", audit_dir.display()))?;

    let session = new_ulid();
    let cwd = std::env::current_dir()?.display().to_string();
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    audit.write(WriteRequest {
        session: session.clone(),
        mode: Mode::Normal,
        payload: EntryPayload::SessionStart {
            entry: "wilai-cli".to_string(),
            cwd,
            user,
        },
    })?;

    let system_prompt = build_system_prompt(&registry, cfg.general.system_prompt.as_deref());
    let mut history: Vec<Message> = Vec::new();

    let result = if let Some(prompt) = once {
        run_turn(
            &*provider,
            &model,
            &registry,
            &mut audit,
            &session,
            &system_prompt,
            &mut history,
            prompt,
            &*confirmer,
            confirm_timeout_s,
        )
        .await
    } else {
        run_interactive(
            &*provider,
            &model,
            &registry,
            &mut audit,
            &session,
            &system_prompt,
            &mut history,
            &*confirmer,
            confirm_timeout_s,
        )
        .await
    };

    let _ = audit.write(WriteRequest {
        session,
        mode: Mode::Normal,
        payload: EntryPayload::SessionEnd,
    });

    result
}

fn build_provider(
    name: &str,
    cfg: &wilai_core::Config,
) -> Result<Box<dyn Provider>> {
    let pcfg = cfg
        .providers
        .get(name)
        .ok_or_else(|| anyhow!("provider {name} not in config"))?;
    match pcfg.kind.as_str() {
        "ollama" => {
            let url = pcfg
                .url
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:11434".to_string());
            Ok(Box::new(OllamaProvider::new(name, url)?))
        }
        "anthropic" => {
            let env_var = pcfg
                .api_key_env
                .clone()
                .unwrap_or_else(|| "ANTHROPIC_API_KEY".to_string());
            let key = std::env::var(&env_var).map_err(|_| {
                anyhow!("env var {env_var} unset; required for anthropic provider")
            })?;
            Ok(Box::new(AnthropicProvider::new(name, key, pcfg.url.clone())?))
        }
        other => Err(anyhow!("provider type {other} not implemented yet")),
    }
}

fn build_system_prompt(registry: &Registry, user_extra: Option<&str>) -> String {
    let mut s = String::new();
    s.push_str(
        "You are Wilai, a system agent on WilOS Aurora. You assist by calling \
         typed tools that the host serializes for you. Never produce raw shell \
         commands; if no tool fits, say so and suggest the closest available tool. \
         Be concise. After each tool result, decide whether to call another tool \
         or to answer the user.\n\n",
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

async fn run_interactive(
    provider: &dyn Provider,
    model: &str,
    registry: &Registry,
    audit: &mut AuditWriter,
    session: &str,
    system_prompt: &str,
    history: &mut Vec<Message>,
    confirmer: &dyn Confirmer,
    confirm_timeout_s: u32,
) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    println!("Wilai v0.6 - {} via {}. Ctrl-D to quit.", model, provider.name());
    loop {
        {
            let mut h = stdout.lock();
            write!(h, "\n> ")?;
            h.flush()?;
        }
        let mut line = String::new();
        let n = stdin.lock().read_line(&mut line)?;
        if n == 0 {
            println!();
            break;
        }
        let prompt = line.trim().to_string();
        if prompt.is_empty() {
            continue;
        }
        if prompt == ":quit" || prompt == ":exit" {
            break;
        }
        if let Err(e) = run_turn(
            provider, model, registry, audit, session, system_prompt, history, prompt,
            confirmer, confirm_timeout_s,
        )
        .await
        {
            eprintln!("error: {e:#}");
        }
    }
    Ok(())
}

async fn run_turn(
    provider: &dyn Provider,
    model: &str,
    registry: &Registry,
    audit: &mut AuditWriter,
    session: &str,
    system_prompt: &str,
    history: &mut Vec<Message>,
    user_text: String,
    confirmer: &dyn Confirmer,
    confirm_timeout_s: u32,
) -> Result<()> {
    history.push(Message {
        role: MessageRole::User,
        content: user_text,
        tool_call_id: None,
        tool_calls: vec![],
    });

    let tool_schemas: Vec<wilai_providers::ToolSchema> = registry
        .iter()
        .map(|(_, spec)| crate::schema::tool_to_schema(spec))
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

        let resp = match resp {
            Ok(r) => {
                audit.write(WriteRequest {
                    session: session.to_string(),
                    mode: Mode::Normal,
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
                })?;
                r
            }
            Err(e) => {
                audit.write(WriteRequest {
                    session: session.to_string(),
                    mode: Mode::Normal,
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
                })?;
                return Err(e);
            }
        };

        if resp.message.tool_calls.is_empty() {
            if !resp.message.content.is_empty() {
                println!("{}", resp.message.content);
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
            let result = execute_call(
                tc,
                registry,
                audit,
                session,
                provider.name(),
                model,
                &turn_id,
                confirmer,
                confirm_timeout_s,
            )
            .await;
            match result {
                Ok(text) => {
                    history.push(Message {
                        role: MessageRole::Tool,
                        content: text,
                        tool_call_id: Some(tc.id.clone()),
                        tool_calls: vec![],
                    });
                }
                Err(e) => {
                    any_invalid = true;
                    history.push(Message {
                        role: MessageRole::Tool,
                        content: format!("error: {e:#}"),
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

async fn execute_call(
    tc: &ToolCall,
    registry: &Registry,
    audit: &mut AuditWriter,
    session: &str,
    provider_name: &str,
    model: &str,
    turn_id: &str,
    confirmer: &dyn Confirmer,
    confirm_timeout_s: u32,
) -> Result<String> {
    let spec = registry
        .get(&tc.name)
        .ok_or_else(|| anyhow!("unknown tool: {}", tc.name))?;

    let validated = match wilai_tools::validator::validate_args(spec, &tc.arguments) {
        Ok(v) => v,
        Err(e) => {
            let reason = format!("validation failed: {e}");
            audit.write(WriteRequest {
                session: session.to_string(),
                mode: Mode::Normal,
                payload: EntryPayload::ToolDeny {
                    tool_name: tc.name.clone(),
                    args: tc.arguments.clone(),
                    reason: reason.clone(),
                    guard: "validator".to_string(),
                },
            })?;
            return Err(anyhow!(reason));
        }
    };

    match evaluate(spec, &validated, Mode::Normal)? {
        GuardOutcome::Allow => {}
        GuardOutcome::Deny { reason, guard } => {
            audit.write(WriteRequest {
                session: session.to_string(),
                mode: Mode::Normal,
                payload: EntryPayload::ToolDeny {
                    tool_name: spec.name.clone(),
                    args: validated.clone(),
                    reason: reason.clone(),
                    guard,
                },
            })?;
            return Err(anyhow!(reason));
        }
        GuardOutcome::Confirm { prompt, default_no, guard: _ } => {
            let default = if default_no { ConfirmDefault::No } else { ConfirmDefault::Yes };
            let start = Instant::now();
            let answer = confirmer.ask(&prompt, default, confirm_timeout_s).await?;
            let latency_ms = start.elapsed().as_millis() as u64;
            let answer_str = match answer {
                ConfirmAnswer::Yes => "yes",
                ConfirmAnswer::No => "no",
                ConfirmAnswer::Timeout => "timeout",
            };
            audit.write(WriteRequest {
                session: session.to_string(),
                mode: Mode::Normal,
                payload: EntryPayload::ToolConfirm {
                    tool_name: spec.name.clone(),
                    args: validated.clone(),
                    prompt: prompt.clone(),
                    answer: answer_str.to_string(),
                    latency_ms,
                },
            })?;
            if !matches!(answer, ConfirmAnswer::Yes) {
                return Err(anyhow!("confirmation refused: {prompt}"));
            }
        }
    }

    let result = Executor::run(spec, &validated).await?;

    let exec_kind = match &spec.executor {
        wilai_tools::ExecutorSpec::Subprocess(_) => "subprocess",
        wilai_tools::ExecutorSpec::Builtin(_) => "builtin",
    };

    let logged_args = filter_args(&validated, spec);
    let sample = if result.output.is_empty() {
        None
    } else {
        let limit = result.output.len().min(1024);
        Some(safe_slice(&result.output, limit).to_string())
    };

    audit.write(WriteRequest {
        session: session.to_string(),
        mode: Mode::Normal,
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
    })?;

    Ok(result.output)
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
