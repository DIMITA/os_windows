//! Proactive-nudge primitives for the daemon.
//!
//! A `Nudge` is a small, well-typed signal the daemon generates on its
//! own (not in response to a user prompt) because a watcher noticed
//! something: a failed systemd service, a disk filling up, a watched
//! process disappearing. Nudges are broadcast to every connected
//! session and show up to the user as a `ServerEvent::Nudge`.
//!
//! The quota keeps the firehose polite: a token bucket with capacity
//! `quota_per_hour`, refilled proportionally over 60 minutes.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warn,
    Alert,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Alert => "alert",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nudge {
    /// Random id (ulid). Lets clients de-duplicate.
    pub id: String,
    /// Dotted kind, e.g. "systemd.failed", "disk.low", "process.gone".
    pub kind: String,
    pub severity: Severity,
    /// Short, user-facing message in natural language.
    pub message: String,
    /// Optional structured context (watcher-specific).
    #[serde(default)]
    pub context: serde_json::Value,
}

impl Nudge {
    pub fn new(kind: impl Into<String>, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            id: wilai_core::types::new_ulid(),
            kind: kind.into(),
            severity,
            message: message.into(),
            context: serde_json::Value::Null,
        }
    }

    pub fn with_context(mut self, ctx: serde_json::Value) -> Self {
        self.context = ctx;
        self
    }
}

/// Rolling-hour token bucket. Not strictly a bucket — tracks timestamps
/// of recent nudges and refuses when the count in the last 3600s meets
/// the configured cap. Simple, exact.
pub struct NudgeQuota {
    capacity: u32,
    recent: Mutex<VecDeque<Instant>>,
}

impl NudgeQuota {
    pub fn new(capacity: u32) -> Self {
        Self { capacity, recent: Mutex::new(VecDeque::new()) }
    }

    /// Try to admit one nudge now; returns true on success.
    pub fn take(&self) -> bool {
        if self.capacity == 0 {
            return false;
        }
        let mut q = self.recent.lock().unwrap();
        let now = Instant::now();
        let hour_ago = now - Duration::from_secs(3600);
        while let Some(front) = q.front() {
            if *front < hour_ago {
                q.pop_front();
            } else {
                break;
            }
        }
        if q.len() as u32 >= self.capacity {
            return false;
        }
        q.push_back(now);
        true
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_zero_rejects_everything() {
        let q = NudgeQuota::new(0);
        assert!(!q.take());
        assert!(!q.take());
    }

    #[test]
    fn quota_admits_up_to_capacity() {
        let q = NudgeQuota::new(3);
        assert!(q.take());
        assert!(q.take());
        assert!(q.take());
        assert!(!q.take());
    }

    #[test]
    fn nudge_builder_sets_fields() {
        let n = Nudge::new("disk.low", Severity::Warn, "/ at 92%");
        assert_eq!(n.kind, "disk.low");
        assert_eq!(n.severity.as_str(), "warn");
        assert_eq!(n.message, "/ at 92%");
    }
}
