//! McpClient: spawn a JSON-RPC 2.0 MCP server, talk to it over stdio,
//! initialize, list tools, route call_tool requests by id.
//!
//! Each server is a long-running subprocess. We assume one outstanding
//! request at a time per server (sequential), which keeps the implementation
//! tiny and matches how the agent loop calls tools anyway.

use crate::wire::{
    CallToolParams, CallToolResult, ClientInfo, InitializeParams, InitializeResult,
    ListToolsResult, McpToolDescriptor, Notification, PROTOCOL_VERSION, Request, Response,
};
use anyhow::{anyhow, Context, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;

const RPC_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct McpToolHandle {
    pub server: String,
    pub descriptor: McpToolDescriptor,
}

pub struct McpClient {
    pub server_name: String,
    next_id: Mutex<u64>,
    stdin: Mutex<ChildStdin>,
    pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Value>>>>,
    _child: Mutex<Child>,
    _reader_task: tokio::task::JoinHandle<()>,
}

impl McpClient {
    /// Spawn a server and complete the initialize handshake.
    pub async fn spawn(
        name: &str,
        command: &str,
        args: &[String],
        env: &std::collections::BTreeMap<String, String>,
    ) -> Result<Arc<Self>> {
        let mut cmd = Command::new(command);
        cmd.args(args);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn mcp server `{name}`: {command}"))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let stderr = child.stderr.take();

        let pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_for_reader = pending.clone();
        let server_name_for_reader = name.to_string();
        let reader_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let v: Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(server = %server_name_for_reader, "mcp parse: {e}: {trimmed}");
                        continue;
                    }
                };
                if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                    let mut map = pending_for_reader.lock().await;
                    if let Some(tx) = map.remove(&id) {
                        let _ = tx.send(v);
                    } else {
                        tracing::debug!(server = %server_name_for_reader, "no pending for id {id}");
                    }
                } else {
                    // notification or malformed; ignore for now.
                    tracing::debug!(server = %server_name_for_reader, "mcp notification: {trimmed}");
                }
            }
            tracing::info!(server = %server_name_for_reader, "mcp reader closed");
        });

        if let Some(stderr) = stderr {
            let server_name = name.to_string();
            tokio::spawn(async move {
                let mut r = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = r.next_line().await {
                    tracing::debug!(server = %server_name, "mcp stderr: {}", line);
                }
            });
        }

        let client = Arc::new(Self {
            server_name: name.to_string(),
            next_id: Mutex::new(1),
            stdin: Mutex::new(stdin),
            pending,
            _child: Mutex::new(child),
            _reader_task: reader_task,
        });

        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&self) -> Result<()> {
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION,
            capabilities: serde_json::json!({}),
            client_info: ClientInfo {
                name: "wilai",
                version: env!("CARGO_PKG_VERSION"),
            },
        };
        let _: InitializeResult = self.request("initialize", params).await?;

        let note = Notification::new("notifications/initialized", serde_json::json!({}));
        self.send_raw(&serde_json::to_value(&note)?).await?;
        Ok(())
    }

    pub async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>> {
        let res: ListToolsResult = self.request("tools/list", serde_json::json!({})).await?;
        Ok(res.tools)
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<CallToolResult> {
        let params = CallToolParams { name, arguments };
        self.request("tools/call", params).await
    }

    async fn request<P: serde::Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R> {
        let id = {
            let mut lk = self.next_id.lock().await;
            let id = *lk;
            *lk += 1;
            id
        };
        let req = Request::new(id, method, params);
        let req_value = serde_json::to_value(&req)?;
        let (tx, rx) = tokio::sync::oneshot::channel::<Value>();
        self.pending.lock().await.insert(id, tx);
        self.send_raw(&req_value).await?;

        let raw = match timeout(RPC_TIMEOUT, rx).await {
            Ok(Ok(v)) => v,
            Ok(Err(_)) => return Err(anyhow!("mcp connection closed mid-request")),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(anyhow!("mcp request `{method}` timed out"));
            }
        };
        let parsed: Response<R> = serde_json::from_value(raw)
            .with_context(|| format!("parse mcp response for `{method}`"))?;
        if let Some(err) = parsed.error {
            return Err(anyhow!("mcp error {}: {}", err.code, err.message));
        }
        parsed
            .result
            .ok_or_else(|| anyhow!("mcp `{method}`: no result and no error"))
    }

    async fn send_raw(&self, v: &Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(v)?;
        bytes.push(b'\n');
        let mut lk = self.stdin.lock().await;
        lk.write_all(&bytes).await?;
        lk.flush().await?;
        Ok(())
    }
}
