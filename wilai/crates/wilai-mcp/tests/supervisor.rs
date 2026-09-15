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
