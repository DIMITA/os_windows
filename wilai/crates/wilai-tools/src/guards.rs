use crate::spec::{GuardSpec, ToolSpec};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use wilai_core::{Mode, Risk};

#[derive(Debug, Clone)]
pub enum GuardOutcome {
    Allow,
    Deny { reason: String, guard: String },
    Confirm { prompt: String, default_no: bool, guard: String },
}

pub fn evaluate(spec: &ToolSpec, args: &Value, mode: Mode) -> Result<GuardOutcome> {
    if let Some(req) = spec.mode_required {
        if mode != req {
            return Ok(GuardOutcome::Deny {
                reason: format!("tool requires mode={req}, current={mode}"),
                guard: "mode_required".to_string(),
            });
        }
    }
    if let Some(forb) = spec.mode_forbidden {
        if mode == forb {
            return Ok(GuardOutcome::Deny {
                reason: format!("tool forbidden in mode={mode}"),
                guard: "mode_forbidden".to_string(),
            });
        }
    }

    for g in &spec.guards {
        match eval_one(g, args, mode)? {
            GuardOutcome::Allow => continue,
            other => return Ok(other),
        }
    }

    let default_outcome = match spec.risk {
        Risk::None | Risk::Low => GuardOutcome::Allow,
        Risk::Medium | Risk::High | Risk::Critical => GuardOutcome::Confirm {
            prompt: format!("Run {} with args {}?", spec.name, args),
            default_no: !matches!(spec.risk, Risk::Medium),
            guard: format!("risk={}", spec.risk),
        },
    };
    Ok(default_outcome)
}

fn eval_one(g: &GuardSpec, args: &Value, mode: Mode) -> Result<GuardOutcome> {
    if let Some(req) = g.require_mode {
        if mode != req {
            return Ok(GuardOutcome::Deny {
                reason: format!("guard require_mode={req}"),
                guard: "require_mode".to_string(),
            });
        }
        return Ok(GuardOutcome::Allow);
    }

    let cond = match &g.condition {
        Some(c) => c.as_str(),
        None => return Ok(GuardOutcome::Allow),
    };

    let triggered = eval_condition(cond, args)
        .map_err(|e| anyhow!("guard `{cond}`: {e}"))?;
    if !triggered {
        return Ok(GuardOutcome::Allow);
    }

    if let Some(reason_tmpl) = &g.deny {
        let reason = render_template(reason_tmpl, args);
        return Ok(GuardOutcome::Deny {
            reason,
            guard: cond.to_string(),
        });
    }
    if let Some(prompt_tmpl) = &g.confirm {
        let prompt = render_template(prompt_tmpl, args);
        return Ok(GuardOutcome::Confirm {
            prompt,
            default_no: true,
            guard: cond.to_string(),
        });
    }
    Ok(GuardOutcome::Allow)
}

fn eval_condition(expr: &str, args: &Value) -> Result<bool> {
    let trimmed = expr.trim();
    if trimmed == "true" {
        return Ok(true);
    }
    if trimmed == "false" {
        return Ok(false);
    }
    if let Some(rest) = trimmed.strip_prefix("not ") {
        return Ok(!eval_condition(rest.trim(), args)?);
    }
    if let Some((open, rest)) = trimmed.split_once('(') {
        let name = open.trim();
        let inner = rest.strip_suffix(')').ok_or_else(|| anyhow!("missing closing paren"))?;
        let parts: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
        let obj = args.as_object().ok_or_else(|| anyhow!("args not object"))?;
        return match name {
            "always" => Ok(true),
            "path_outside_home" => {
                let arg = parts.first().ok_or_else(|| anyhow!("path_outside_home(arg)"))?;
                let s = obj
                    .get(*arg)
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("missing arg {arg}"))?;
                Ok(!path_under_home(s))
            }
            "path_outside_roots" => {
                let arg = parts.first().ok_or_else(|| anyhow!("path_outside_roots(arg, root, ...)"))?;
                let s = obj
                    .get(*arg)
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("missing arg {arg}"))?;
                let roots: Vec<PathBuf> = parts[1..]
                    .iter()
                    .map(|r| PathBuf::from(unquote(r)))
                    .collect();
                let root_refs: Vec<&Path> = roots.iter().map(|p| p.as_path()).collect();
                Ok(!crate::patterns::path_under(Path::new(s), &root_refs))
            }
            "arg_gt" => num_compare(parts, obj, |a, b| a > b),
            "arg_ge" => num_compare(parts, obj, |a, b| a >= b),
            "arg_lt" => num_compare(parts, obj, |a, b| a < b),
            "arg_le" => num_compare(parts, obj, |a, b| a <= b),
            "arg_eq" => str_compare(parts, obj, |a, b| a == b),
            "arg_neq" => str_compare(parts, obj, |a, b| a != b),
            "arg_true" => {
                let arg = parts.first().ok_or_else(|| anyhow!("arg_true(name)"))?;
                Ok(obj.get(*arg).and_then(|v| v.as_bool()).unwrap_or(false))
            }
            other => bail!("unknown guard function: {other}"),
        };
    }
    bail!("unsupported condition: {expr}")
}

fn num_compare(
    parts: Vec<&str>,
    obj: &serde_json::Map<String, Value>,
    op: impl Fn(f64, f64) -> bool,
) -> Result<bool> {
    let arg = parts.first().ok_or_else(|| anyhow!("expected (name, value)"))?;
    let val: f64 = parts
        .get(1)
        .ok_or_else(|| anyhow!("missing value"))?
        .parse()
        .map_err(|e| anyhow!("bad number: {e}"))?;
    let actual = obj
        .get(*arg)
        .and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow!("arg {arg} not numeric"))?;
    Ok(op(actual, val))
}

fn str_compare(
    parts: Vec<&str>,
    obj: &serde_json::Map<String, Value>,
    op: impl Fn(&str, &str) -> bool,
) -> Result<bool> {
    let arg = parts.first().ok_or_else(|| anyhow!("expected (name, value)"))?;
    let val = unquote(parts.get(1).ok_or_else(|| anyhow!("missing value"))?);
    let actual = obj
        .get(*arg)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("arg {arg} not string"))?;
    Ok(op(actual, &val))
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn path_under_home(p: &str) -> bool {
    let home = match wilai_core::paths::home_dir() {
        Some(h) => h,
        None => return false,
    };
    let path = if let Some(stripped) = p.strip_prefix("~/") {
        home.join(stripped)
    } else if p == "~" {
        home.clone()
    } else {
        PathBuf::from(p)
    };
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let canon = std::fs::canonicalize(&path).unwrap_or(path);
    let home_canon = std::fs::canonicalize(&home).unwrap_or(home);
    canon.starts_with(&home_canon)
}

pub fn render_template(tmpl: &str, args: &Value) -> String {
    let obj = match args.as_object() {
        Some(o) => o,
        None => return tmpl.to_string(),
    };
    let mut out = String::with_capacity(tmpl.len());
    let mut chars = tmpl.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            for ch in chars.by_ref() {
                if ch == '}' {
                    break;
                }
                name.push(ch);
            }
            match obj.get(&name) {
                Some(Value::String(s)) => out.push_str(s),
                Some(other) => out.push_str(&other.to_string()),
                None => {
                    out.push('{');
                    out.push_str(&name);
                    out.push('}');
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}
