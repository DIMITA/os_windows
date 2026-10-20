//! Subprocess sandboxing via bubblewrap (bwrap). Converts a declared
//! SandboxSpec into the argv prefix that bwrap understands, falls back
//! gracefully when bwrap is absent on the host (logs once, returns the
//! original argv), and is a no-op outside Linux.

use crate::spec::SandboxSpec;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Cached detection result for `bwrap`. Checked once per daemon lifetime.
static BWRAP: OnceLock<Option<PathBuf>> = OnceLock::new();

fn bwrap_path() -> Option<PathBuf> {
    // Operator / test override. Checked FIRST so an integration test can
    // point at a stub without racing the OnceLock cache; an operator can
    // also redirect to a vendored bwrap for consistency across machines.
    if let Ok(override_path) = std::env::var("WILAI_BWRAP") {
        if !override_path.is_empty() {
            let p = PathBuf::from(&override_path);
            if p.exists() {
                return Some(p);
            }
        }
    }
    BWRAP
        .get_or_init(|| {
            if let Ok(path) = std::env::var("PATH") {
                for dir in path.split(':') {
                    let candidate = PathBuf::from(dir).join("bwrap");
                    if candidate.exists() {
                        return Some(candidate);
                    }
                }
            }
            tracing::warn!(
                "bwrap (bubblewrap) not on PATH; sandboxed tools will run unsandboxed. \
                 Install bubblewrap to enable per-tool isolation."
            );
            None
        })
        .clone()
}

/// Transform `(binary, args)` into a sandboxed invocation when the
/// SandboxSpec and host allow it. Returns the original argv unchanged
/// if the sandbox is not applicable (no bwrap, non-Linux, etc.).
pub fn render_argv(binary: &str, args: &[String], sandbox: Option<&SandboxSpec>) -> Vec<String> {
    let Some(spec) = sandbox else {
        let mut v = Vec::with_capacity(1 + args.len());
        v.push(binary.to_string());
        v.extend_from_slice(args);
        return v;
    };
    let bwrap = match bwrap_path() {
        Some(p) => p,
        None => {
            let mut v = Vec::with_capacity(1 + args.len());
            v.push(binary.to_string());
            v.extend_from_slice(args);
            return v;
        }
    };

    match spec {
        SandboxSpec::Bwrap {
            bind_ro,
            bind_rw,
            tmpfs,
            network,
            pass_env,
        } => {
            let mut out: Vec<String> = Vec::with_capacity(1 + 2 * (bind_ro.len() + bind_rw.len() + tmpfs.len()) + 16);
            out.push(bwrap.to_string_lossy().into_owned());
            // Core safety: new PID + mount + IPC namespaces, no uid
            // change (preserve the user).
            out.push("--unshare-pid".into());
            out.push("--unshare-ipc".into());
            out.push("--unshare-uts".into());
            out.push("--unshare-cgroup-try".into());
            if !network {
                out.push("--unshare-net".into());
            }
            out.push("--die-with-parent".into());
            out.push("--new-session".into());
            // Minimal /proc so programs can find themselves.
            out.push("--proc".into());
            out.push("/proc".into());
            // tmpfs for /tmp by default when caller didn't ask for anything.
            out.push("--tmpfs".into());
            out.push("/tmp".into());
            // Dev stub.
            out.push("--dev".into());
            out.push("/dev".into());

            for p in bind_ro {
                out.push("--ro-bind-try".into());
                out.push(p.clone());
                out.push(p.clone());
            }
            for p in bind_rw {
                out.push("--bind-try".into());
                out.push(p.clone());
                out.push(p.clone());
            }
            for p in tmpfs {
                out.push("--tmpfs".into());
                out.push(p.clone());
            }
            if !pass_env.is_empty() {
                // The executor already env-cleared; bwrap re-populates
                // from the parent env of the bwrap process itself,
                // so we rely on the parent's env (which we curated).
                // No-op for now; left as documented knob.
                let _ = pass_env;
            }
            out.push("--".into());
            out.push(binary.to_string());
            out.extend_from_slice(args);
            out
        }
    }
}

/// Is bwrap available on this host? Public so tests and status commands
/// can report sandbox availability.
pub fn is_available() -> bool {
    bwrap_path().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_no_sandbox() {
        let v = render_argv("nmap", &["-sS".into(), "10.0.0.1".into()], None);
        assert_eq!(v, vec!["nmap", "-sS", "10.0.0.1"]);
    }

    #[test]
    fn bwrap_prefix_is_rendered_when_available() {
        // Only run the assertion when bwrap is actually on PATH;
        // otherwise the test confirms graceful fallback.
        let spec = SandboxSpec::Bwrap {
            bind_ro: vec!["/usr".to_string(), "/etc".to_string()],
            bind_rw: vec![],
            tmpfs: vec![],
            network: false,
            pass_env: vec![],
        };
        let v = render_argv("cat", &["/etc/hostname".into()], Some(&spec));
        if is_available() {
            assert!(v[0].ends_with("bwrap"), "expected bwrap prefix, got {v:?}");
            assert!(v.contains(&"--unshare-net".to_string()));
            assert!(v.contains(&"--ro-bind-try".to_string()));
            // Separator `--` precedes the real command.
            let dash = v.iter().position(|s| s == "--").unwrap();
            assert_eq!(v[dash + 1], "cat");
            assert_eq!(v[dash + 2], "/etc/hostname");
        } else {
            assert_eq!(v, vec!["cat", "/etc/hostname"]);
        }
    }

    #[test]
    fn network_true_omits_unshare_net() {
        let spec = SandboxSpec::Bwrap {
            bind_ro: vec![],
            bind_rw: vec![],
            tmpfs: vec![],
            network: true,
            pass_env: vec![],
        };
        let v = render_argv("curl", &["https://x".into()], Some(&spec));
        if is_available() {
            assert!(!v.contains(&"--unshare-net".to_string()));
        }
    }

    #[test]
    fn override_env_var_pins_bwrap_binary() {
        // Build a self-executable stub so bwrap_path's `.exists()` passes.
        let tmp = tempfile::tempdir().unwrap();
        let stub = tmp.path().join("bwrap-stub");
        std::fs::write(&stub, "#!/bin/sh\necho stub").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755));
        }
        // The override is checked FIRST and bypasses the OnceLock cache,
        // so we can exercise the bwrap render path on hosts without bwrap.
        std::env::set_var("WILAI_BWRAP", &stub);
        let spec = SandboxSpec::Bwrap {
            bind_ro: vec!["/usr".into(), "/etc".into()],
            bind_rw: vec!["/home/u/sandbox".into()],
            tmpfs: vec!["/var/tmp".into()],
            network: false,
            pass_env: vec![],
        };
        let v = render_argv("echo", &["hi".into()], Some(&spec));
        std::env::remove_var("WILAI_BWRAP");

        assert!(v[0].ends_with("bwrap-stub"));
        assert!(v.contains(&"--unshare-net".into()));
        assert!(v.contains(&"--die-with-parent".into()));
        // ro bind pair for /usr: `--ro-bind-try /usr /usr`
        let i = v.iter().position(|s| s == "/usr").unwrap();
        assert_eq!(v[i - 1], "--ro-bind-try");
        assert_eq!(v[i + 1], "/usr");
        // rw bind for /home/u/sandbox: `--bind-try /home/u/sandbox /home/u/sandbox`
        let j = v.iter().position(|s| s == "/home/u/sandbox").unwrap();
        assert_eq!(v[j - 1], "--bind-try");
        assert_eq!(v[j + 1], "/home/u/sandbox");
        // tmpfs for /var/tmp: `--tmpfs /var/tmp`
        let k = v.iter().position(|s| s == "/var/tmp").unwrap();
        assert_eq!(v[k - 1], "--tmpfs");
        // The dash separator precedes the user's command.
        let dash = v.iter().position(|s| s == "--").unwrap();
        assert_eq!(v[dash + 1], "echo");
        assert_eq!(v[dash + 2], "hi");
    }
}
