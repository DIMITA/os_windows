use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

pub async fn dispatch(name: &str, args: &Value) -> Result<String> {
    match name {
        "fs.read.v1" => fs_read(args).await,
        "fs.list.v1" => fs_list(args).await,
        "sysinfo.get.v1" => sysinfo_get(args).await,
        other => Err(anyhow!("unknown builtin: {other}")),
    }
}

async fn fs_read(args: &Value) -> Result<String> {
    let path = args.get("path").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("fs.read: path missing"))?;
    let max_bytes = args.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(65536) as usize;
    let p = expand_tilde(path);
    let bytes = fs::read(&p).with_context(|| format!("read {}", p.display()))?;
    let truncated = bytes.len() > max_bytes;
    let slice = if truncated { &bytes[..max_bytes] } else { &bytes[..] };
    let text = String::from_utf8_lossy(slice).to_string();
    Ok(if truncated {
        format!("{text}\n\n[truncated at {max_bytes} bytes; total {}]", bytes.len())
    } else {
        text
    })
}

async fn fs_list(args: &Value) -> Result<String> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let p = expand_tilde(path);
    let mut out = String::new();
    let mut entries: Vec<_> = fs::read_dir(&p)
        .with_context(|| format!("read_dir {}", p.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let ft = entry.file_type()?;
        let kind = if ft.is_dir() { "d" } else if ft.is_symlink() { "l" } else { "f" };
        let name = entry.file_name();
        out.push_str(&format!("{kind} {}\n", name.to_string_lossy()));
    }
    Ok(out)
}

async fn sysinfo_get(_args: &Value) -> Result<String> {
    let mut out = String::new();
    if let Ok(uname) = std::process::Command::new("uname").arg("-a").output() {
        out.push_str(&format!("uname: {}", String::from_utf8_lossy(&uname.stdout)));
    }
    if let Ok(rel) = fs::read_to_string("/etc/os-release") {
        out.push_str("--- /etc/os-release ---\n");
        out.push_str(&rel);
    }
    if let Ok(host) = fs::read_to_string("/etc/hostname") {
        out.push_str("--- /etc/hostname ---\n");
        out.push_str(&host);
    }
    if out.is_empty() {
        out.push_str("(no system info available)\n");
    }
    Ok(out)
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(stripped) = s.strip_prefix("~/") {
        if let Some(home) = wilai_core::paths::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(s)
}
