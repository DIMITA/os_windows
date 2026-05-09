use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

const SCAFFOLD: &str = r#"name: {NAME}
version: 1
description: |-
  Briefly describe what this tool does. The LLM sees this verbatim.
category: {CATEGORY}
risk: {RISK}

inputs:
  example:
    type: string
    required: true
    description: Replace with real arguments. Delete this stub.

# Optional guards. Each guard short-circuits to Allow / Deny / Confirm.
# guards:
#   - if: "path_outside_home(path)"
#     confirm: "Operate on {path} outside $HOME?"

executor:
  type: builtin           # or "subprocess"
  fn: change.me.v1        # for builtin, name a registered function
  # cmd: ["bin", "{example}"]   # for subprocess, argv with placeholders
  timeout_s: 5
  output:
    capture: stdout
    max_bytes: 65536

audit:
  fields: [example]

examples:
  - args: { example: "value" }
    note: "Describe the canonical usage."
"#;

pub fn new(name: &str, out: Option<&Path>, category: &str, risk: &str) -> Result<()> {
    if !is_valid_dotted_name(name) {
        return Err(anyhow!("name must be lowercase dotted (e.g. fs.read)"));
    }
    validate_category(category)?;
    validate_risk(risk)?;

    let target: PathBuf = match out {
        Some(p) => p.to_path_buf(),
        None => {
            let dir = wilai_core::paths::user_tools_dir()?;
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("create {}", dir.display()))?;
            dir.join(format!("{name}.yaml"))
        }
    };
    if target.exists() {
        return Err(anyhow!("{} already exists", target.display()));
    }

    let body = SCAFFOLD
        .replace("{NAME}", name)
        .replace("{CATEGORY}", category)
        .replace("{RISK}", risk);
    std::fs::write(&target, body)
        .with_context(|| format!("write {}", target.display()))?;
    println!("scaffolded {}", target.display());
    Ok(())
}

pub fn validate(path: &Path) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let spec: wilai_tools::ToolSpec = serde_yaml::from_str(&text)
        .with_context(|| format!("parse {}", path.display()))?;
    println!(
        "ok: {} v{} ({}/{}; {} input(s), {} guard(s))",
        spec.name,
        spec.version,
        spec.category,
        spec.risk,
        spec.inputs.len(),
        spec.guards.len(),
    );
    Ok(())
}

pub fn dry_run(path: &Path, args_json: &str) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let spec: wilai_tools::ToolSpec = serde_yaml::from_str(&text)
        .with_context(|| format!("parse {}", path.display()))?;
    let args: serde_json::Value = serde_json::from_str(args_json)
        .with_context(|| format!("parse --args: {args_json}"))?;
    let validated = wilai_tools::validator::validate_args(&spec, &args)
        .with_context(|| "validation")?;

    let outcome = wilai_tools::guards::evaluate(&spec, &validated, wilai_core::Mode::Normal)?;
    match &outcome {
        wilai_tools::guards::GuardOutcome::Allow => println!("guard: allow"),
        wilai_tools::guards::GuardOutcome::Deny { reason, guard } => {
            println!("guard: deny ({guard}): {reason}");
        }
        wilai_tools::guards::GuardOutcome::Confirm { prompt, default_no, guard } => {
            let def = if *default_no { "no" } else { "yes" };
            println!("guard: confirm ({guard}, default={def}): {prompt}");
        }
    }

    match &spec.executor {
        wilai_tools::ExecutorSpec::Subprocess(s) => {
            let argv = render_argv_for_inspection(&s.cmd, &validated)?;
            println!("executor: subprocess");
            println!("argv:    {}", format_argv(&argv));
            println!("timeout: {}s", s.timeout_s);
        }
        wilai_tools::ExecutorSpec::Builtin(b) => {
            println!("executor: builtin");
            println!("fn:      {}", b.function);
            println!("timeout: {}s", b.timeout_s);
            println!("args:    {}", validated);
        }
    }
    Ok(())
}

fn render_argv_for_inspection(template: &[String], args: &serde_json::Value) -> Result<Vec<String>> {
    let obj = args
        .as_object()
        .ok_or_else(|| anyhow!("args not an object"))?;
    let mut out = Vec::new();
    for tok in template {
        let placeholder = if tok.starts_with('{') && tok.ends_with('}') && tok.len() >= 3 {
            let inner = &tok[1..tok.len() - 1];
            if !inner.contains('{') && !inner.contains('}') {
                Some(inner)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(name) = placeholder {
            match obj.get(name) {
                Some(serde_json::Value::Array(arr)) => {
                    for v in arr {
                        out.push(value_token(v));
                    }
                }
                Some(v) => out.push(value_token(v)),
                None => return Err(anyhow!("placeholder {name} missing")),
            }
        } else if tok.contains('{') && tok.contains('}') {
            let mut s = String::new();
            let mut chars = tok.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '{' {
                    let mut name = String::new();
                    for ch in chars.by_ref() {
                        if ch == '}' { break; }
                        name.push(ch);
                    }
                    let v = obj.get(&name).ok_or_else(|| anyhow!("placeholder {name} missing"))?;
                    s.push_str(&value_token(v));
                } else {
                    s.push(c);
                }
            }
            out.push(s);
        } else {
            out.push(tok.clone());
        }
    }
    Ok(out)
}

fn value_token(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        _ => v.to_string(),
    }
}

fn format_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|s| if s.contains(' ') || s.is_empty() {
            format!("'{s}'")
        } else {
            s.clone()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_valid_dotted_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_')
        && !s.starts_with('.')
        && !s.ends_with('.')
}

fn validate_category(s: &str) -> Result<()> {
    match s {
        "read" | "write" | "network" | "destructive" | "privileged" | "pentest" | "keyring" => Ok(()),
        _ => Err(anyhow!("category must be one of read|write|network|destructive|privileged|pentest|keyring")),
    }
}

fn validate_risk(s: &str) -> Result<()> {
    match s {
        "none" | "low" | "medium" | "high" | "critical" => Ok(()),
        _ => Err(anyhow!("risk must be one of none|low|medium|high|critical")),
    }
}
