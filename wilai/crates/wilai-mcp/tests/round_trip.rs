//! Spawn the mock MCP server, complete the handshake, list its tools,
//! call one, and confirm the round-trip.

use std::collections::BTreeMap;
use std::path::PathBuf;
use wilai_mcp::McpClient;

fn mock_server_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests");
    p.push("fixtures");
    p.push("mock_server.py");
    p
}

#[tokio::test]
async fn lists_and_calls_mock_tool() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skip: python3 not available");
        return;
    }
    let env = BTreeMap::new();
    let client = McpClient::spawn(
        "mock",
        "python3",
        &[mock_server_path().to_string_lossy().to_string()],
        &env,
    )
    .await
    .expect("spawn mock mcp server");

    let tools = client.list_tools().await.expect("list_tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");

    let res = client
        .call_tool("echo", serde_json::json!({"message": "hi there"}))
        .await
        .expect("call_tool");
    assert!(!res.is_error);
    assert_eq!(res.content.len(), 1);
    let text = res.content[0].render();
    assert!(text.contains("hi there"), "echo output: {text}");

    // ToolSpec conversion plugs into the wilai registry.
    let spec = wilai_mcp::convert::descriptor_to_spec("mock", &tools[0]).unwrap();
    assert_eq!(spec.name, "mcp.mock.echo");
    assert!(spec.inputs.contains_key("message"));
    assert!(spec.inputs.get("message").unwrap().required);
}
