//! Backfill test: a Service with an empty MCP tool slice in the
//! registry, a live supervisor whose server has published its tools,
//! and a call to `backfill_mcp_tools` should land the tools in the
//! registry.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::RwLock;
use wilai_audit::AuditWriter;
use wilai_core::{Config, McpServerConfig, Mode};
use wilai_daemon::mode_mgr::ModeManager;
use wilai_daemon::service::{backfill_mcp_tools, Service};
use wilai_mcp::McpSupervisor;
use wilai_tools::Registry;

fn mock_server_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.push("wilai-mcp");
    p.push("tests");
    p.push("fixtures");
    p.push("mock_server.py");
    p
}

#[tokio::test]
async fn backfill_registers_tools_after_supervisor_warms_up() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skip: python3 not available");
        return;
    }

    let tmp = TempDir::new().unwrap();
    let audit_dir = tmp.path().join("audit");
    std::fs::create_dir_all(&audit_dir).unwrap();
    let writer = AuditWriter::open(&audit_dir).unwrap();
    let (audit_tx, _h) = writer.spawn_task();
    let mode = ModeManager::new(Mode::Normal, audit_tx.clone());

    // Start an empty registry — no YAML tools, no MCP tools yet.
    let registry = Arc::new(RwLock::new(Registry::empty()));

    // Spawn a supervisor independently of the service (as if init_mcp
    // had timed out before this server was ready).
    let env = BTreeMap::new();
    let sup = McpSupervisor::spawn(
        "late",
        "python3",
        &[mock_server_path().to_string_lossy().to_string()],
        &env,
    )
    .await;

    // Wait for the supervisor to publish its tools list.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if !sup.tools().await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!sup.tools().await.is_empty(), "supervisor never got tools");

    // Build a cfg that declares the server (so backfill knows gate_in_pentest).
    let mut cfg = Config::default();
    cfg.mcp.insert(
        "late".to_string(),
        McpServerConfig {
            command: "python3".to_string(),
            args: vec![mock_server_path().to_string_lossy().to_string()],
            env: BTreeMap::new(),
            gate_in_pentest: false,
            enabled: true,
        },
    );

    let mut mcp: HashMap<String, Arc<McpSupervisor>> = HashMap::new();
    mcp.insert("late".to_string(), sup.clone());

    // Pre-condition: registry is empty.
    assert_eq!(registry.read().await.len(), 0);

    // Backfill.
    let added = backfill_mcp_tools(&cfg, &registry, &mcp).await;
    assert_eq!(added.len(), 1, "expected 1 tool added, got {added:?}");
    assert_eq!(added[0], "mcp.late.echo");

    // Post-condition: registry has `mcp.late.echo`.
    {
        let reg = registry.read().await;
        assert!(reg.get("mcp.late.echo").is_some(), "mcp.late.echo not in registry");
    }

    // Second call is a no-op: tool already there.
    let added_again = backfill_mcp_tools(&cfg, &registry, &mcp).await;
    assert!(added_again.is_empty());

    // Sanity: a Service built with this registry sees the tool through
    // the same lock pathway run_turn uses.
    let _service = Arc::new(Service {
        cfg,
        registry: registry.clone(),
        audit_tx,
        mode,
        default_provider: "mock".to_string(),
        default_model: "mock-1".to_string(),
        confirm_timeout_s: 5,
        mcp,
        tools_changed: Arc::new(tokio::sync::Notify::new()),
        memory: None,
        embedder: None,
        nudges: tokio::sync::broadcast::channel(16).0,
        project: wilai_pentest::project::empty_handle(),
        project_store: Arc::new(tokio::sync::RwLock::new(None)),
    });
}
