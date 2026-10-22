use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

pub async fn dispatch(name: &str, args: &Value) -> Result<String> {
    match name {
        "fs.read.v1" => fs_read(args).await,
        "fs.list.v1" => fs_list(args).await,
        "fs.write.v1" => fs_write(args).await,
        "sysinfo.get.v1" => sysinfo_get(args).await,
        "script.run.v1" => script_run(args, false).await,
        "script.run.privileged.v1" => script_run(args, true).await,
        other => Err(anyhow!("unknown builtin: {other}")),
    }
}

/// LLM-authored script execution.
///
/// `privileged = false`: bubblewrap-sandboxed, read-only /, no
///   network, writable /tmp only, 30s default timeout, 256 KiB
///   output cap. Deny-pattern scan refuses obvious destructive
///   commands before dispatch.
///
/// `privileged = true`: sandbox relaxed (writable $HOME + /tmp,
///   network allowed, sudo callable). Still deny-pattern scanned.
///   The daemon layer wraps this with a mandatory confirm + the
///   `[security] allow_privileged_script` config gate — this
///   function trusts its caller enforced those.
async fn script_run(args: &Value, privileged: bool) -> Result<String> {
    let interpreter = args
        .get("interpreter")
        .and_then(|v| v.as_str())
        .unwrap_or("bash");
    let source = args
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("script.run: `source` missing"))?;
    let timeout_s = args
        .get("timeout_s")
        .and_then(|v| v.as_u64())
        .unwrap_or(30)
        .min(120)
        .max(1) as u64;

    // Interpreter whitelist. "bash" and "sh" always available; "python3"
    // only if present on the host.
    let (bin, args_pre) = match interpreter {
        "bash" => ("/bin/bash", vec!["-c".to_string()]),
        "sh" => ("/bin/sh", vec!["-c".to_string()]),
        "python3" => ("/usr/bin/python3", vec!["-I".to_string(), "-c".to_string()]),
        other => return Err(anyhow!("script.run: interpreter `{other}` not supported (bash|sh|python3)")),
    };
    if !std::path::Path::new(bin).exists() {
        return Err(anyhow!("script.run: interpreter {bin} not installed on host"));
    }

    // 1. Deny-pattern scan — belt + braces around the sandbox.
    crate::deny::scan(source)?;

    // 2. Build the sandbox spec. Sandboxed path = read-only / with
    //    nothing writable but a per-run tmpfs for /tmp. Privileged
    //    path = bind the user's $HOME writable + allow network.
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let sandbox = if privileged {
        crate::spec::SandboxSpec::Bwrap {
            bind_ro: vec!["/usr".into(), "/etc".into(), "/bin".into(), "/sbin".into(), "/lib".into(), "/lib64".into(), "/opt".into()],
            bind_rw: vec!["/tmp".into(), home.clone()],
            tmpfs: vec![],
            network: true,
            pass_env: vec!["HOME".into(), "USER".into(), "PATH".into(), "LANG".into()],
        }
    } else {
        crate::spec::SandboxSpec::Bwrap {
            bind_ro: vec!["/usr".into(), "/etc".into(), "/bin".into(), "/sbin".into(), "/lib".into(), "/lib64".into(), "/opt".into()],
            bind_rw: vec![],   // nothing writable outside the tmpfs below
            tmpfs: vec!["/tmp".into(), "/home".into(), "/root".into()],
            network: false,
            pass_env: vec!["PATH".into(), "LANG".into()],
        }
    };

    // 3. Render the argv under bwrap. render_argv prepends --unshare-pid,
    //    --unshare-ipc, --die-with-parent, etc.
    let mut argv_in: Vec<String> = args_pre;
    argv_in.push(source.to_string());
    let argv_full = crate::sandbox::render_argv(bin, &argv_in, Some(&sandbox));
    let bwrap_bin = argv_full[0].clone();
    let bwrap_args: Vec<String> = argv_full[1..].to_vec();

    // 4. Run with timeout + output cap.
    use tokio::process::Command;
    use tokio::time::{timeout, Duration};
    let output = timeout(
        Duration::from_secs(timeout_s),
        Command::new(&bwrap_bin)
            .args(&bwrap_args)
            .output(),
    )
    .await
    .map_err(|_| anyhow!("script.run: timed out after {timeout_s}s"))?
    .context("script.run: spawn failed")?;

    const MAX_OUT: usize = 256 * 1024;
    let mut out = String::new();
    out.push_str(&String::from_utf8_lossy(&output.stdout));
    if !output.stderr.is_empty() {
        if !out.is_empty() { out.push('\n'); }
        out.push_str("[stderr]\n");
        out.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    if out.len() > MAX_OUT {
        let kept = &out[..MAX_OUT];
        out = format!("{kept}\n...(+{} bytes truncated)", out.len() - MAX_OUT);
    }
    if !output.status.success() {
        let code = output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signalled".to_string());
        return Err(anyhow!("script exited with status {code}:\n{out}"));
    }
    Ok(out)
}

async fn fs_write(args: &Value) -> Result<String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("fs.write: path missing"))?;
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("fs.write: content missing"))?;
    let create_only = args
        .get("create_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let p = expand_tilde(path);

    if create_only && p.exists() {
        return Err(anyhow!("fs.write: {} already exists", p.display()));
    }

    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create_dir_all {}", parent.display()))?;
        }
    }

    fs::write(&p, content).with_context(|| format!("write {}", p.display()))?;
    Ok(format!("wrote {} bytes to {}\n", content.len(), p.display()))
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
