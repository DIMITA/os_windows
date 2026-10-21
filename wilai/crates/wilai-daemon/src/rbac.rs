//! Role-based access control. The daemon exposes a single tool
//! catalogue but filters which tools each session may call based on
//! the role derived from the peer uid of its unix socket.

use wilai_core::{Category, Risk};
use wilai_tools::ToolSpec;

/// Does a role permit calling this tool?
pub fn role_allows(role: &str, spec: &ToolSpec) -> bool {
    match role {
        "admin" => true,
        "user" => allows_user(spec),
        "guest" => allows_guest(spec),
        // Unknown role: deny by default. The session loop logs this
        // the first time it is seen so operators notice a config typo.
        _ => false,
    }
}

fn allows_user(spec: &ToolSpec) -> bool {
    // The user role is "my regular shell user" — can run most things
    // including sys.service.* and sys.pkg.install BUT:
    //   - no destructive/critical risk (would wipe a disk / drop a db)
    //   - no network category (prevents accidental exfil / outbound)
    //   - no remote.wilai.* (federation needs admin by design)
    if matches!(spec.risk, Risk::Critical) {
        return false;
    }
    if matches!(spec.category, Category::Destructive) {
        return false;
    }
    if matches!(spec.category, Category::Network) {
        return false;
    }
    if spec.name.starts_with("remote.wilai.") {
        return false;
    }
    true
}

fn allows_guest(spec: &ToolSpec) -> bool {
    // guest == read-only. Category must be `read`, risk at most Low,
    // name must not start with sys.* (even sys.service.status is
    // more than a guest should learn: it leaks the running services).
    if !matches!(spec.category, Category::Read) {
        return false;
    }
    if matches!(spec.risk, Risk::Medium | Risk::High | Risk::Critical) {
        return false;
    }
    if spec.name.starts_with("sys.") {
        return false;
    }
    if spec.name.starts_with("remote.") {
        return false;
    }
    true
}

/// Normalize an operator-supplied role string. Unknown values map to
/// "guest" (least privilege). Case-insensitive, trimmed.
pub fn normalize_role(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "admin" | "root" | "owner" => "admin",
        "user" | "member" | "" => "user",
        _ => "guest",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use wilai_tools::spec::{BuiltinSpec, CaptureKind, ExecutorSpec, OutputSpec};

    fn make_spec(name: &str, cat: Category, risk: Risk) -> ToolSpec {
        ToolSpec {
            name: name.to_string(),
            version: 1,
            description: "t".to_string(),
            category: cat,
            risk,
            tags: vec![],
            mode_required: None,
            mode_forbidden: None,
            inputs: BTreeMap::new(),
            guards: vec![],
            executor: ExecutorSpec::Builtin(BuiltinSpec {
                function: "x".to_string(),
                timeout_s: 1,
                output: OutputSpec {
                    capture: CaptureKind::Stdout,
                    max_bytes: 1024,
                    redact: vec![],
                },
            }),
            audit: Default::default(),
            examples: vec![],
        }
    }

    #[test]
    fn admin_allows_everything() {
        let s = make_spec("sys.disk.wipe", Category::Destructive, Risk::Critical);
        assert!(role_allows("admin", &s));
    }

    #[test]
    fn user_denies_destructive_critical_network_remote() {
        assert!(!role_allows(
            "user",
            &make_spec("sys.disk.wipe", Category::Destructive, Risk::Critical)
        ));
        assert!(!role_allows(
            "user",
            &make_spec("remote.wilai.exec", Category::Network, Risk::High)
        ));
        assert!(role_allows(
            "user",
            &make_spec("sys.service.start", Category::Privileged, Risk::Medium)
        ));
    }

    #[test]
    fn guest_is_read_only_non_sys() {
        assert!(role_allows(
            "guest",
            &make_spec("fs.read", Category::Read, Risk::None)
        ));
        assert!(!role_allows(
            "guest",
            &make_spec("sys.service.status", Category::Read, Risk::None)
        ));
        assert!(!role_allows(
            "guest",
            &make_spec("fs.write", Category::Write, Risk::Medium)
        ));
    }

    #[test]
    fn unknown_role_is_denied() {
        assert!(!role_allows(
            "wizard",
            &make_spec("fs.read", Category::Read, Risk::None)
        ));
    }

    #[test]
    fn normalize_role_covers_aliases() {
        assert_eq!(normalize_role("Admin"), "admin");
        assert_eq!(normalize_role("owner"), "admin");
        assert_eq!(normalize_role(""), "user");
        assert_eq!(normalize_role("intern"), "guest");
    }
}
