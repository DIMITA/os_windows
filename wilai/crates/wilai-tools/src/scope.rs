//! Scope-check indirection. wilai-pentest implements ScopeChecker and
//! installs an Arc of itself when `pt.project.start` runs. The guard
//! DSL below queries the global checker, so the existing network
//! tools (nmap.scan, nuclei.run, whatweb.scan) can bear
//!   `if: "scope_denies(target)"` / `if: "scope_requires_ok(target)"`
//! guards without wilai-tools needing a hard dep on wilai-pentest.
//!
//! When no project is active, both predicates return false — the
//! existing behavior is preserved for anyone not using the pentest
//! workspace.

use std::sync::{Arc, OnceLock, RwLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeDecision {
    Allow,
    RequireOk { note: String },
    Deny { reason: String },
    NotConfigured,
}

pub trait ScopeChecker: Send + Sync {
    fn check(&self, target: &str) -> ScopeDecision;
}

fn slot() -> &'static RwLock<Option<Arc<dyn ScopeChecker>>> {
    static CELL: OnceLock<RwLock<Option<Arc<dyn ScopeChecker>>>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(None))
}

pub fn set_checker(c: Arc<dyn ScopeChecker>) {
    *slot().write().unwrap() = Some(c);
}

pub fn clear_checker() {
    *slot().write().unwrap() = None;
}

pub fn check(target: &str) -> ScopeDecision {
    match slot().read().unwrap().as_ref() {
        Some(c) => c.check(target),
        None => ScopeDecision::NotConfigured,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Deny;
    impl ScopeChecker for Deny {
        fn check(&self, t: &str) -> ScopeDecision {
            ScopeDecision::Deny { reason: format!("deny {t}") }
        }
    }

    #[test]
    fn no_checker_returns_not_configured() {
        clear_checker();
        assert_eq!(check("1.2.3.4"), ScopeDecision::NotConfigured);
    }

    #[test]
    fn set_then_check() {
        set_checker(Arc::new(Deny));
        match check("1.2.3.4") {
            ScopeDecision::Deny { reason } => assert!(reason.contains("1.2.3.4")),
            other => panic!("expected deny, got {other:?}"),
        }
        clear_checker();
    }
}
