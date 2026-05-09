//! Pentest mode auto-detection. Polls a small set of cheap, side-effect-free
//! signals every interval, sums a score, and asks ModeManager to switch when
//! the score crosses a threshold. Exit from pentest is never automatic.

use crate::mode_mgr::ModeManager;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use wilai_core::Mode;

const PENTEST_BINARIES: &[&str] = &[
    "msfconsole",
    "msfvenom",
    "burpsuite",
    "burpsuite-pro",
    "wireshark",
    "tshark",
    "nmap",
    "nuclei",
    "sqlmap",
    "hydra",
    "responder",
    "bloodhound",
    "ffuf",
    "gobuster",
    "wfuzz",
    "metasploit",
];

const WORKSPACE_PATTERNS: &[&str] = &["pentest", "redteam", "offsec"];

#[derive(Debug, Default, Clone)]
pub struct Signal {
    pub name: &'static str,
    pub score: u32,
    pub note: String,
}

pub struct DetectConfig {
    pub interval: Duration,
    pub threshold: u32,
    pub workspace_score: u32,
    pub binary_score: u32,
    pub vpn_score: u32,
}

impl Default for DetectConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            threshold: 100,
            workspace_score: 60,
            binary_score: 40,
            vpn_score: 40,
        }
    }
}

/// Spawn the auto-detection task. The task polls forever; abort the
/// JoinHandle to stop it.
pub fn spawn(mode: Arc<ModeManager>, cfg: DetectConfig) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if mode.current().await == Mode::Normal {
                let signals = collect_signals(&cfg).await;
                let score: u32 = signals.iter().map(|s| s.score).sum();
                if score >= cfg.threshold {
                    let names: Vec<String> = signals
                        .iter()
                        .map(|s| format!("{}:{}", s.name, s.score))
                        .collect();
                    let trigger = format!("autodetect[{}]@{}", names.join(","), score);
                    match mode.set(Mode::Pentest, &trigger).await {
                        Ok(change) => {
                            tracing::info!(?change, "auto-switched to pentest mode");
                        }
                        Err(e) => {
                            tracing::warn!("auto-switch refused: {e}");
                        }
                    }
                }
            }
            tokio::time::sleep(cfg.interval).await;
        }
    })
}

pub async fn collect_signals(cfg: &DetectConfig) -> Vec<Signal> {
    let mut out = Vec::new();
    if let Some(name) = active_workspace_name().await {
        if WORKSPACE_PATTERNS
            .iter()
            .any(|p| name.to_ascii_lowercase().contains(p))
        {
            out.push(Signal {
                name: "workspace",
                score: cfg.workspace_score,
                note: name,
            });
        }
    }
    if let Some(found) = active_pentest_binary().await {
        out.push(Signal {
            name: "binary",
            score: cfg.binary_score,
            note: found,
        });
    }
    if vpn_active().await {
        out.push(Signal {
            name: "vpn",
            score: cfg.vpn_score,
            note: "active vpn".to_string(),
        });
    }
    out
}

async fn active_workspace_name() -> Option<String> {
    let out = Command::new("hyprctl")
        .args(["activeworkspace", "-j"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    v.get("name").and_then(|n| n.as_str()).map(|s| s.to_string())
}

async fn active_pentest_binary() -> Option<String> {
    let proc = Path::new("/proc");
    let entries = std::fs::read_dir(proc).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        let comm = p.join("comm");
        if let Ok(name) = std::fs::read_to_string(&comm) {
            let trimmed = name.trim();
            if PENTEST_BINARIES.iter().any(|b| *b == trimmed) {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

async fn vpn_active() -> bool {
    if let Ok(out) = Command::new("nmcli")
        .args(["-t", "-f", "TYPE", "connection", "show", "--active"])
        .output()
        .await
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if s.lines().any(|l| l.contains("vpn") || l.contains("wireguard")) {
                return true;
            }
        }
    }
    if let Ok(routes) = std::fs::read_to_string("/proc/net/route") {
        if routes.lines().skip(1).any(|l| {
            let cols: Vec<&str> = l.split_whitespace().collect();
            cols.first().map(|name| name.starts_with("tun") || name.starts_with("wg")).unwrap_or(false)
        }) {
            return true;
        }
    }
    false
}
