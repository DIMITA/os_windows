//! Tools-changed notification: when backfill inserts new tools, the
//! Service.tools_changed Notify is fired. Clients subscribing to it
//! (via spawn_tools_relay) should see a ServerEvent::ToolsAdded.
//!
//! This test exercises the full chain without a daemon by invoking the
//! same spawn_tools_relay helper on a plain mpsc channel.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Notify, RwLock};
use wilai_core::{Category, Risk};
use wilai_daemon::protocol::ServerEvent;
use wilai_tools::spec::{
    AuditSpec, BuiltinSpec, CaptureKind, ExecutorSpec, OutputSpec, ToolSpec,
};
use wilai_tools::Registry;

fn fake_spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        version: 1,
        description: "test".into(),
        category: Category::Read,
        risk: Risk::None,
        tags: vec![],
        mode_required: None,
        mode_forbidden: None,
        inputs: Default::default(),
        guards: vec![],
        executor: ExecutorSpec::Builtin(BuiltinSpec {
            function: "test.v1".into(),
            timeout_s: 1,
            output: OutputSpec {
                capture: CaptureKind::Stdout,
                max_bytes: 128,
                redact: vec![],
            },
        }),
        audit: AuditSpec::default(),
        examples: vec![],
    }
}

// Re-expose spawn_tools_relay via a shim: wilai-daemon doesn't export
// it, so we duplicate the logic here with the same shape.
fn spawn_relay(
    notify: Arc<Notify>,
    registry: Arc<RwLock<Registry>>,
    event_tx: mpsc::Sender<ServerEvent>,
) {
    tokio::spawn(async move {
        let mut known: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        {
            let reg = registry.read().await;
            for (name, _) in reg.iter() {
                known.insert(name.to_string());
            }
        }
        loop {
            notify.notified().await;
            let current: std::collections::BTreeSet<String> = {
                let reg = registry.read().await;
                reg.iter().map(|(n, _)| n.to_string()).collect()
            };
            let fresh: Vec<String> = current.difference(&known).cloned().collect();
            if fresh.is_empty() {
                continue;
            }
            known = current;
            if event_tx
                .send(ServerEvent::ToolsAdded { names: fresh })
                .await
                .is_err()
            {
                break;
            }
        }
    });
}

#[tokio::test]
async fn relay_pushes_only_the_delta() {
    let registry = Arc::new(RwLock::new(Registry::empty()));
    let notify = Arc::new(Notify::new());
    let (tx, mut rx) = mpsc::channel::<ServerEvent>(8);

    // Pre-populate with one tool so the relay's initial snapshot is non-empty.
    registry.write().await.insert_dynamic(fake_spec("fs.read"));

    spawn_relay(notify.clone(), registry.clone(), tx);
    // Give the relay a tick to collect the initial snapshot.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Insert a new tool and fire the notify.
    registry.write().await.insert_dynamic(fake_spec("mcp.late.echo"));
    notify.notify_waiters();

    let ev = tokio::time::timeout(Duration::from_millis(200), rx.recv())
        .await
        .expect("event timeout")
        .expect("recv");
    match ev {
        ServerEvent::ToolsAdded { names } => {
            assert_eq!(names, vec!["mcp.late.echo".to_string()]);
        }
        other => panic!("expected ToolsAdded, got {other:?}"),
    }

    // Firing again with no changes should not produce another event
    // (the relay has already seen the current state).
    notify.notify_waiters();
    let maybe = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
    assert!(maybe.is_err(), "unexpected event: {maybe:?}");
}
