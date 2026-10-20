//! Supervisor restart-on-crash smoke: spawn the mock, kill it, and
//! confirm the supervisor brings a new client up before the backoff cap.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;
use wilai_mcp::McpSupervisor;

fn mock_server_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests");
    p.push("fixtures");
    p.push("mock_server.py");
    p
}

#[tokio::test]
async fn supervisor_restarts_after_kill() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skip: python3 not available");
        return;
    }
    let env = BTreeMap::new();
    let sup = McpSupervisor::spawn(
        "mock",
        "python3",
        &[mock_server_path().to_string_lossy().to_string()],
        &env,
    )
    .await;

    // Wait for the first client to be alive (tools populated).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let status = sup.status().await;
        if status.alive && status.tool_count > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let before = sup.status().await;
    assert!(before.alive, "server never became alive");
    assert!(before.tool_count > 0, "no tools listed");
    assert_eq!(before.restart_count, 0);

    // Kill the current client (triggers wait_closed).
    sup.restart_now();

    // Backoff sequence starts at 2s. Wait up to 10s for the supervisor
    // to bring a new client up.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut restarted = false;
    while std::time::Instant::now() < deadline {
        let status = sup.status().await;
        if status.alive && status.restart_count >= 1 {
            restarted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(restarted, "supervisor did not restart within 10s");

    // The new client answers requests.
    let res = sup
        .call_tool("echo", serde_json::json!({"message": "post-restart"}))
        .await
        .expect("call_tool after restart");
    assert!(res.content.iter().any(|c| c.render().contains("post-restart")));
}

#[tokio::test]
async fn supervisor_marks_fatal_when_binary_is_missing() {
    // The whole point: no infinite tight-loop on a typo in the config.
    // Backoff steps are 2/4/8s for the first few failures; after 5
    // consecutive we expect `fatal = true`. Allow up to 25s.
    let env = BTreeMap::new();
    let sup = McpSupervisor::spawn(
        "ghost",
        "/this/binary/does/not/exist",
        &[],
        &env,
    )
    .await;

    // Backoff ladder (1/2/4s) means the 4th failure happens around
    // t = 7s. Allow 15s margin.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut saw_fatal = false;
    while std::time::Instant::now() < deadline {
        let s = sup.status().await;
        if s.fatal {
            saw_fatal = true;
            assert!(s.last_error.is_some(), "expected a last_error with fatal");
            assert!(!s.alive);
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        saw_fatal,
        "supervisor should have given up after 5 spawn failures"
    );

    // A tool call returns a clean error, not a hang.
    let err = sup
        .call_tool("anything", serde_json::json!({}))
        .await
        .err()
        .expect("call_tool on fatal server should fail fast");
    assert!(format!("{err:#}").contains("not alive"));
}
