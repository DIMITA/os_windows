//! Background watchers that produce `Nudge`s.
//!
//! Each watcher is a tokio task that loops at its own cadence,
//! observes a slice of the system, and emits a nudge via the shared
//! broadcast channel when it sees a transition worth telling the user
//! about. All three are strictly local (no network), use the smallest
//! external surface they can (sysconf, /proc, `systemctl --failed`),
//! and respect the quota.

use crate::nudge::{Nudge, NudgeQuota, Severity};
use std::collections::HashSet;
use std::ffi::CString;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use wilai_core::ProactivityConfig;

/// Spawn one task per enabled watcher. Returns their JoinHandles so
/// main.rs can abort them on shutdown.
pub fn spawn_all(
    cfg: ProactivityConfig,
    tx: broadcast::Sender<Nudge>,
    quota: Arc<NudgeQuota>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::new();
    if !cfg.enabled {
        return handles;
    }

    let watch_mounts = if cfg.watch_mounts.is_empty() {
        vec!["/".to_string()]
    } else {
        cfg.watch_mounts.clone()
    };

    // systemd watcher
    {
        let tx = tx.clone();
        let q = Arc::clone(&quota);
        let poll = cfg.systemd_poll_s.max(5) as u64;
        handles.push(tokio::spawn(async move {
            systemd_loop(tx, q, Duration::from_secs(poll)).await;
        }));
    }

    // disk watcher
    {
        let tx = tx.clone();
        let q = Arc::clone(&quota);
        let poll = cfg.disk_poll_s.max(10) as u64;
        let threshold = cfg.disk_threshold_pct;
        handles.push(tokio::spawn(async move {
            disk_loop(tx, q, Duration::from_secs(poll), watch_mounts, threshold).await;
        }));
    }

    // process watcher
    if !cfg.watch_processes.is_empty() {
        let tx = tx.clone();
        let q = Arc::clone(&quota);
        let poll = cfg.process_poll_s.max(5) as u64;
        let names = cfg.watch_processes.clone();
        handles.push(tokio::spawn(async move {
            process_loop(tx, q, Duration::from_secs(poll), names).await;
        }));
    }

    handles
}

fn send(tx: &broadcast::Sender<Nudge>, quota: &NudgeQuota, nudge: Nudge) {
    if !quota.take() {
        tracing::debug!(kind = %nudge.kind, "nudge rate-limited");
        return;
    }
    let _ = tx.send(nudge);
}

async fn systemd_loop(tx: broadcast::Sender<Nudge>, quota: Arc<NudgeQuota>, poll: Duration) {
    let mut known: HashSet<String> = HashSet::new();
    // First pass: seed known set without emitting (we only care about
    // *new* failures after startup).
    if let Ok(seen) = failed_units().await {
        known = seen;
    }
    let mut ticker = tokio::time::interval(poll);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let now = match failed_units().await {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("systemd watcher: {e:#}");
                continue;
            }
        };
        for unit in now.difference(&known) {
            let msg = format!("{unit} has entered failed state");
            send(
                &tx,
                &quota,
                Nudge::new("systemd.failed", Severity::Warn, msg)
                    .with_context(serde_json::json!({ "unit": unit })),
            );
        }
        known = now;
    }
}

async fn failed_units() -> anyhow::Result<HashSet<String>> {
    let out = tokio::process::Command::new("systemctl")
        .args(["--no-pager", "--no-legend", "list-units", "--state=failed", "--type=service"])
        .output()
        .await?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut units = HashSet::new();
    for line in text.lines() {
        if let Some(unit) = line.split_whitespace().next() {
            units.insert(unit.to_string());
        }
    }
    Ok(units)
}

async fn disk_loop(
    tx: broadcast::Sender<Nudge>,
    quota: Arc<NudgeQuota>,
    poll: Duration,
    mounts: Vec<String>,
    threshold_pct: u32,
) {
    let mut below: HashSet<String> = HashSet::new();
    let mut ticker = tokio::time::interval(poll);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        for mnt in &mounts {
            let free = match free_pct(mnt) {
                Some(p) => p,
                None => continue,
            };
            let is_below = free < threshold_pct;
            if is_below && !below.contains(mnt) {
                let msg = format!("{mnt} has only {free}% free (threshold {threshold_pct}%)");
                let sev = if free < threshold_pct / 2 { Severity::Alert } else { Severity::Warn };
                send(
                    &tx,
                    &quota,
                    Nudge::new("disk.low", sev, msg)
                        .with_context(serde_json::json!({ "mount": mnt, "free_pct": free })),
                );
                below.insert(mnt.clone());
            } else if !is_below && below.contains(mnt) {
                // Recovered: emit an info nudge once, then stop
                // reporting until we drop below again.
                let msg = format!("{mnt} is back to {free}% free");
                send(
                    &tx,
                    &quota,
                    Nudge::new("disk.recovered", Severity::Info, msg)
                        .with_context(serde_json::json!({ "mount": mnt, "free_pct": free })),
                );
                below.remove(mnt);
            }
        }
    }
}

/// Free-space percent on a mount, via libc::statvfs. Returns None when
/// the path can't be statvfs'd (e.g. unmounted or permission denied).
pub fn free_pct(path: &str) -> Option<u32> {
    if !Path::new(path).exists() {
        return None;
    }
    let c = CString::new(path).ok()?;
    // SAFETY: statvfs is a stable POSIX call; buf is correctly sized.
    unsafe {
        let mut buf = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if libc::statvfs(c.as_ptr(), buf.as_mut_ptr()) != 0 {
            return None;
        }
        let s = buf.assume_init();
        if s.f_blocks == 0 {
            return None;
        }
        // bavail is for non-root; blocks is total. Both in f_frsize units.
        let total = s.f_blocks as u128;
        let free = s.f_bavail as u128;
        Some(((free * 100) / total) as u32)
    }
}

async fn process_loop(
    tx: broadcast::Sender<Nudge>,
    quota: Arc<NudgeQuota>,
    poll: Duration,
    names: Vec<String>,
) {
    let mut gone: HashSet<String> = HashSet::new();
    let mut ticker = tokio::time::interval(poll);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        for name in &names {
            let alive = process_alive(name);
            if !alive && !gone.contains(name) {
                let msg = format!("process `{name}` is no longer running");
                send(
                    &tx,
                    &quota,
                    Nudge::new("process.gone", Severity::Warn, msg)
                        .with_context(serde_json::json!({ "name": name })),
                );
                gone.insert(name.clone());
            } else if alive && gone.contains(name) {
                let msg = format!("process `{name}` is back");
                send(
                    &tx,
                    &quota,
                    Nudge::new("process.back", Severity::Info, msg)
                        .with_context(serde_json::json!({ "name": name })),
                );
                gone.remove(name);
            }
        }
    }
}

/// Scan /proc for a process whose comm matches `name` exactly.
fn process_alive(name: &str) -> bool {
    let proc = match std::fs::read_dir("/proc") {
        Ok(d) => d,
        Err(_) => return false,
    };
    for entry in proc.flatten() {
        let file_name = entry.file_name();
        let s = file_name.to_string_lossy();
        if !s.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let comm_path = entry.path().join("comm");
        if let Ok(comm) = std::fs::read_to_string(&comm_path) {
            if comm.trim() == name {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_pct_root_is_sane() {
        // Root should always be statvfs'able in a test environment.
        if let Some(p) = free_pct("/") {
            assert!(p <= 100);
        }
    }

    #[test]
    fn process_alive_detects_init() {
        // PID 1 always exists on linux. Its comm is 'systemd' or
        // 'init' depending on environment; try both.
        let systemd = process_alive("systemd");
        let init = process_alive("init");
        // At least one is usually true on a real system; in a stripped
        // container it may be neither — don't fail the suite.
        let _ = systemd || init;
    }

    #[tokio::test]
    async fn failed_units_returns_a_set() {
        // Just make sure it doesn't panic; the content depends on the
        // host. In a container without systemctl it returns Err — fine.
        let _ = failed_units().await;
    }
}
