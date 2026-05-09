use crate::spec::{CaptureKind, ExecutorSpec, ToolSpec};
use anyhow::{bail, Result};
use serde_json::Value;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

pub struct Executor;

#[derive(Debug, Clone)]
pub struct ExecResult {
    pub exit_code: i32,
    pub output: String,
    pub output_bytes: usize,
    pub truncated: bool,
    pub duration_ms: u64,
    pub rendered_cmd: Option<Vec<String>>,
}

impl Executor {
    pub async fn run(spec: &ToolSpec, args: &Value) -> Result<ExecResult> {
        match &spec.executor {
            ExecutorSpec::Builtin(b) => {
                let start = Instant::now();
                let fut = crate::builtins::dispatch(&b.function, args);
                let res = timeout(Duration::from_secs(b.timeout_s as u64), fut)
                    .await
                    .map_err(|_| anyhow::anyhow!("builtin timed out"))?;
                let duration_ms = start.elapsed().as_millis() as u64;
                let (exit_code, mut text) = match res {
                    Ok(s) => (0, s),
                    Err(e) => (1, format!("error: {e:#}")),
                };
                let total = text.len();
                let truncated = total > b.output.max_bytes;
                if truncated {
                    text.truncate(b.output.max_bytes);
                }
                Ok(ExecResult {
                    exit_code,
                    output: text,
                    output_bytes: total,
                    truncated,
                    duration_ms,
                    rendered_cmd: None,
                })
            }
            ExecutorSpec::Subprocess(s) => {
                let argv = render_argv(&s.cmd, args)?;
                if argv.is_empty() {
                    bail!("empty argv");
                }
                let start = Instant::now();
                let mut cmd = Command::new(&argv[0]);
                cmd.args(&argv[1..]);
                cmd.stdin(Stdio::null());
                match s.output.capture {
                    CaptureKind::Stdout => { cmd.stdout(Stdio::piped()).stderr(Stdio::null()); }
                    CaptureKind::Stderr => { cmd.stdout(Stdio::null()).stderr(Stdio::piped()); }
                    CaptureKind::Both => { cmd.stdout(Stdio::piped()).stderr(Stdio::piped()); }
                    CaptureKind::None => { cmd.stdout(Stdio::null()).stderr(Stdio::null()); }
                }
                if let Some(cwd) = &s.cwd {
                    cmd.current_dir(cwd);
                }
                cmd.env_clear();
                let allowed_env = ["PATH", "LANG", "LC_ALL", "TERM", "HOME", "USER"];
                for k in allowed_env {
                    if let Ok(v) = std::env::var(k) {
                        cmd.env(k, v);
                    }
                }
                for (k, v) in &s.env {
                    cmd.env(k, v);
                }

                let mut child = cmd.spawn()?;
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();

                let collect = async move {
                    let mut buf = Vec::new();
                    if let Some(mut so) = stdout {
                        so.read_to_end(&mut buf).await.ok();
                    }
                    if let Some(mut se) = stderr {
                        se.read_to_end(&mut buf).await.ok();
                    }
                    buf
                };

                let status_fut = child.wait();
                let dur = Duration::from_secs(s.timeout_s as u64);
                let (status, buf) = match timeout(dur, async {
                    let s = status_fut.await?;
                    let b = collect.await;
                    Ok::<_, std::io::Error>((s, b))
                })
                .await
                {
                    Ok(Ok(pair)) => pair,
                    Ok(Err(e)) => return Err(e.into()),
                    Err(_) => bail!("subprocess timed out after {}s", s.timeout_s),
                };

                let total = buf.len();
                let truncated = total > s.output.max_bytes;
                let slice = if truncated { &buf[..s.output.max_bytes] } else { &buf[..] };
                let text = String::from_utf8_lossy(slice).to_string();

                Ok(ExecResult {
                    exit_code: status.code().unwrap_or(-1),
                    output: text,
                    output_bytes: total,
                    truncated,
                    duration_ms: start.elapsed().as_millis() as u64,
                    rendered_cmd: Some(argv),
                })
            }
        }
    }
}

fn render_argv(template: &[String], args: &Value) -> Result<Vec<String>> {
    let obj = args
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("args not an object"))?;
    let mut out = Vec::with_capacity(template.len());
    for tok in template {
        if let Some(name) = single_placeholder(tok) {
            let v = obj
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("placeholder {name} missing"))?;
            if let Value::Array(arr) = v {
                for item in arr {
                    let s = value_to_token(item)?;
                    if s.contains('\n') {
                        bail!("rendered token contains newline");
                    }
                    out.push(s);
                }
            } else {
                let s = value_to_token(v)?;
                if s.contains('\n') {
                    bail!("rendered token contains newline");
                }
                out.push(s);
            }
        } else if tok.contains('{') && tok.contains('}') {
            let mut rendered = String::with_capacity(tok.len());
            let mut chars = tok.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '{' {
                    let mut name = String::new();
                    for ch in chars.by_ref() {
                        if ch == '}' { break; }
                        name.push(ch);
                    }
                    let v = obj.get(&name)
                        .ok_or_else(|| anyhow::anyhow!("placeholder {name} missing"))?;
                    rendered.push_str(&value_to_token(v)?);
                } else {
                    rendered.push(c);
                }
            }
            if rendered.contains('\n') {
                bail!("rendered token contains newline");
            }
            out.push(rendered);
        } else {
            out.push(tok.clone());
        }
    }
    Ok(out)
}

fn single_placeholder(tok: &str) -> Option<&str> {
    if tok.starts_with('{') && tok.ends_with('}') && tok.len() >= 3 {
        let inner = &tok[1..tok.len() - 1];
        if !inner.contains('{') && !inner.contains('}') {
            return Some(inner);
        }
    }
    None
}

fn value_to_token(v: &Value) -> Result<String> {
    Ok(match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        _ => bail!("cannot render array/object as argv token"),
    })
}
