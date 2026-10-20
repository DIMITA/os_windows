//! McpSupervisor: keeps an MCP server alive with exponential backoff on
//! crash. Callers hold an Arc<McpSupervisor> and invoke `call_tool()`
//! without worrying about the underlying process lifecycle.

use crate::client::McpClient;
use crate::wire::{CallToolResult, McpToolDescriptor};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify, RwLock};

const BACKOFF_STEPS: &[u64] = &[1, 2, 4, 8, 16, 30, 60];
/// After this many consecutive spawn failures with the same error
/// (typical case: binary not on PATH), the supervisor stops retrying
/// and sits idle until an operator calls `restart_now()`.
const FATAL_AFTER_CONSECUTIVE_FAILURES: u32 = 4;

#[derive(Debug, Clone)]
pub struct SupervisorStatus {
    pub name: String,
    pub alive: bool,
    pub uptime_s: Option<u64>,
    pub restart_count: u32,
    pub tool_count: usize,
    pub last_error: Option<String>,
    /// True when the supervisor has given up retrying after too many
    /// consecutive spawn failures. Cleared by `restart_now()`.
    pub fatal: bool,
}

pub struct McpSupervisor {
    pub name: String,
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    client: RwLock<Option<Arc<McpClient>>>,
    tools: RwLock<Vec<McpToolDescriptor>>,
    restart_count: AtomicU32,
    consecutive_spawn_failures: AtomicU32,
    fatal: AtomicBool,
    last_error: Mutex<Option<String>>,
    started_at: Mutex<Option<Instant>>,
    kick: Notify,
    _task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl McpSupervisor {
    /// Spawn a supervisor. Returns immediately; the initial client comes up
    /// asynchronously and may take a beat.
    pub async fn spawn(
        name: &str,
        command: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
    ) -> Arc<Self> {
        let sup = Arc::new(Self {
            name: name.to_string(),
            command: command.to_string(),
            args: args.to_vec(),
            env: env.clone(),
            client: RwLock::new(None),
            tools: RwLock::new(Vec::new()),
            restart_count: AtomicU32::new(0),
            consecutive_spawn_failures: AtomicU32::new(0),
            fatal: AtomicBool::new(false),
            last_error: Mutex::new(None),
            started_at: Mutex::new(None),
            kick: Notify::new(),
            _task: Mutex::new(None),
        });
        let sup_for_task = sup.clone();
        let handle = tokio::spawn(async move {
            sup_for_task.run().await;
        });
        *sup._task.lock().await = Some(handle);
        sup
    }

    async fn run(self: Arc<Self>) {
        let mut backoff_idx = 0usize;
        loop {
            // Fatal state: wait for an explicit kick before trying again.
            if self.fatal.load(Ordering::SeqCst) {
                self.kick.notified().await;
                self.fatal.store(false, Ordering::SeqCst);
                self.consecutive_spawn_failures.store(0, Ordering::SeqCst);
                backoff_idx = 0;
            }

            match McpClient::spawn(&self.name, &self.command, &self.args, &self.env).await {
                Ok(client) => {
                    backoff_idx = 0;
                    self.consecutive_spawn_failures.store(0, Ordering::SeqCst);
                    *self.started_at.lock().await = Some(Instant::now());
                    *self.last_error.lock().await = None;
                    match client.list_tools().await {
                        Ok(t) => {
                            let n = t.len();
                            *self.tools.write().await = t;
                            tracing::info!(server = %self.name, tools = n, "mcp supervisor: server live");
                        }
                        Err(e) => {
                            tracing::warn!(server = %self.name, "mcp supervisor: list_tools failed: {e:#}");
                        }
                    }
                    *self.client.write().await = Some(client.clone());

                    tokio::select! {
                        _ = client.wait_closed() => {
                            tracing::warn!(server = %self.name, "mcp supervisor: server exited");
                        }
                        _ = self.kick.notified() => {
                            tracing::info!(server = %self.name, "mcp supervisor: manual restart");
                            client.kill().await;
                            let _ = client.wait_closed().await;
                        }
                    }

                    *self.client.write().await = None;
                    *self.started_at.lock().await = None;
                    self.restart_count.fetch_add(1, Ordering::SeqCst);
                }
                Err(e) => {
                    *self.last_error.lock().await = Some(format!("{e:#}"));
                    self.restart_count.fetch_add(1, Ordering::SeqCst);
                    let n = self.consecutive_spawn_failures.fetch_add(1, Ordering::SeqCst) + 1;
                    tracing::warn!(
                        server = %self.name,
                        attempt = n,
                        "mcp supervisor: spawn failed: {e:#}"
                    );
                    if n >= FATAL_AFTER_CONSECUTIVE_FAILURES {
                        self.fatal.store(true, Ordering::SeqCst);
                        tracing::error!(
                            server = %self.name,
                            attempts = n,
                            "mcp supervisor: too many spawn failures; marking fatal (wilai mcp restart <name> to retry)"
                        );
                        continue; // top of loop handles the wait-for-kick
                    }
                }
            }

            let delay = BACKOFF_STEPS[backoff_idx.min(BACKOFF_STEPS.len() - 1)];
            backoff_idx = (backoff_idx + 1).min(BACKOFF_STEPS.len() - 1);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(delay)) => {}
                _ = self.kick.notified() => {
                    tracing::info!(server = %self.name, "mcp supervisor: kick during backoff");
                    backoff_idx = 0;
                }
            }
        }
    }

    /// Route a tool call through the current live client. Fails with a
    /// clean error (not a hang) when the server is down.
    pub async fn call_tool(&self, tool: &str, args: Value) -> Result<CallToolResult> {
        let client = self
            .client
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow!("mcp server `{}` is not alive", self.name))?;
        client.call_tool(tool, args).await
    }

    /// Trigger an immediate restart. If the server is alive it's killed;
    /// the supervisor loop will respawn without waiting for the current
    /// backoff step.
    pub fn restart_now(&self) {
        self.kick.notify_one();
    }

    pub async fn tools(&self) -> Vec<McpToolDescriptor> {
        self.tools.read().await.clone()
    }

    pub async fn status(&self) -> SupervisorStatus {
        let alive = self.client.read().await.is_some();
        let started = self.started_at.lock().await;
        let uptime_s = started.map(|t| t.elapsed().as_secs());
        let restart_count = self.restart_count.load(Ordering::SeqCst);
        let last_error = self.last_error.lock().await.clone();
        let tool_count = self.tools.read().await.len();
        SupervisorStatus {
            name: self.name.clone(),
            alive,
            uptime_s,
            restart_count,
            tool_count,
            last_error,
            fatal: self.fatal.load(Ordering::SeqCst),
        }
    }
}
