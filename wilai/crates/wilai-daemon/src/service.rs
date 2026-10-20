use crate::mode_mgr::ModeManager;
use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, RwLock};
use wilai_audit::writer::WriteRequest;
use wilai_audit::AuditWriter;
use wilai_core::{Config, Mode};
use wilai_mcp::McpSupervisor;
use wilai_providers::{AnthropicProvider, OllamaProvider, Provider};
use wilai_tools::Registry;

pub struct Service {
    pub cfg: Config,
    /// Shared tool registry. Wrapped in RwLock so the background MCP
    /// backfill task can insert tools that weren't ready at daemon
    /// startup (e.g. a server that took longer than init_mcp's 5s
    /// grace period to spawn).
    pub registry: Arc<RwLock<Registry>>,
    pub audit_tx: mpsc::Sender<WriteRequest>,
    pub mode: Arc<ModeManager>,
    pub default_provider: String,
    pub default_model: String,
    pub confirm_timeout_s: u32,
    /// Map from server name to its supervisor.
    pub mcp: HashMap<String, Arc<McpSupervisor>>,
}

impl Service {
    /// Build a provider, honoring the cloud kill-switch in pentest mode.
    pub async fn build_provider(&self, name: &str) -> Result<Box<dyn Provider>> {
        let pcfg = self
            .cfg
            .providers
            .get(name)
            .ok_or_else(|| anyhow!("provider {name} not in config"))?;
        let provider: Box<dyn Provider> = match pcfg.kind.as_str() {
            "ollama" => {
                let url = pcfg
                    .url
                    .clone()
                    .unwrap_or_else(|| "http://127.0.0.1:11434".to_string());
                Box::new(OllamaProvider::new(name, url)?)
            }
            "anthropic" => {
                let env_var = pcfg
                    .api_key_env
                    .clone()
                    .unwrap_or_else(|| "ANTHROPIC_API_KEY".to_string());
                let key = std::env::var(&env_var).map_err(|_| {
                    anyhow!("env var {env_var} unset; required for anthropic provider")
                })?;
                Box::new(AnthropicProvider::new(name, key, pcfg.url.clone())?)
            }
            other => return Err(anyhow!("provider type {other} not implemented yet")),
        };
        if self.mode.current().await == Mode::Pentest && !provider.is_local() {
            return Err(anyhow!(
                "provider `{name}` is non-local; cloud kill-switch is active in pentest mode"
            ));
        }
        Ok(provider)
    }
}

/// Spawn a supervisor per configured MCP server. Each supervisor keeps
/// its underlying client alive with backoff. We block briefly on the
/// initial tool listing so the registry is populated before the daemon
/// starts accepting client connections; if the wait times out we install
/// the supervisor anyway and the tools will appear after the next
/// successful restart cycle.
pub async fn init_mcp(
    cfg: &Config,
    registry: &Arc<RwLock<Registry>>,
) -> HashMap<String, Arc<McpSupervisor>> {
    use std::time::Duration;
    let mut out: HashMap<String, Arc<McpSupervisor>> = HashMap::new();
    for (name, server_cfg) in &cfg.mcp {
        if !server_cfg.enabled {
            tracing::info!(server = %name, "mcp server disabled in config; skipping");
            continue;
        }
        let sup = McpSupervisor::spawn(name, &server_cfg.command, &server_cfg.args, &server_cfg.env).await;

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let tools = sup.tools().await;
            if !tools.is_empty() {
                let mut count = 0;
                let mut reg = registry.write().await;
                for desc in &tools {
                    match wilai_mcp::convert::descriptor_to_spec_with_gate(
                        name,
                        desc,
                        server_cfg.gate_in_pentest,
                    ) {
                        Ok(spec) => {
                            reg.insert_dynamic(spec);
                            count += 1;
                        }
                        Err(e) => tracing::warn!(server = %name, tool = %desc.name, "skip mcp tool: {e:#}"),
                    }
                }
                tracing::info!(
                    server = %name,
                    tools = count,
                    gated = server_cfg.gate_in_pentest,
                    "mcp server ready"
                );
                break;
            }
            if std::time::Instant::now() >= deadline {
                tracing::warn!(server = %name, "mcp server not ready within 5s; backfill task will pick it up");
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        out.insert(name.clone(), sup);
    }
    out
}

/// Scan every supervisor's cached tools list and insert anything the
/// registry doesn't have yet. Returns the number of tools added. Called
/// at startup grace-period expiry, after a manual restart, and
/// periodically from a background task so tools that take long to
/// become available still land in the registry without a daemon
/// restart.
pub async fn backfill_mcp_tools(
    cfg: &Config,
    registry: &Arc<RwLock<Registry>>,
    mcp: &HashMap<String, Arc<McpSupervisor>>,
) -> usize {
    let mut added = 0;
    for (name, sup) in mcp {
        let tools = sup.tools().await;
        if tools.is_empty() {
            continue;
        }
        let gated = cfg
            .mcp
            .get(name)
            .map(|c| c.gate_in_pentest)
            .unwrap_or(false);
        let mut reg = registry.write().await;
        for desc in &tools {
            let tool_name = format!("mcp.{}.{}", name, desc.name);
            if reg.get(&tool_name).is_some() {
                continue;
            }
            match wilai_mcp::convert::descriptor_to_spec_with_gate(name, desc, gated) {
                Ok(spec) => {
                    reg.insert_dynamic(spec);
                    added += 1;
                }
                Err(e) => tracing::warn!(server = %name, tool = %desc.name, "skip mcp tool: {e:#}"),
            }
        }
    }
    if added > 0 {
        tracing::info!(added, "mcp backfill: new tools registered");
    }
    added
}

/// Background task that calls `backfill_mcp_tools` on a fixed interval.
/// Catches tools from supervisors that come up late (slow npm install,
/// transient network, backoff ladder) without requiring a daemon restart.
pub fn spawn_backfill_task(
    cfg: Config,
    registry: Arc<RwLock<Registry>>,
    mcp: HashMap<String, Arc<McpSupervisor>>,
    interval_s: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let dur = std::time::Duration::from_secs(interval_s);
        loop {
            tokio::time::sleep(dur).await;
            let _ = backfill_mcp_tools(&cfg, &registry, &mcp).await;
        }
    })
}

pub fn collect_tool_dirs(cfg: &Config) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    dirs.push(wilai_core::paths::shipped_tools_dir());
    if let Ok(user) = wilai_core::paths::user_tools_dir() {
        dirs.push(user);
    }
    for d in &cfg.tools.extra_dirs {
        dirs.push(d.clone());
    }
    dirs
}

pub struct SocketServer {
    listener: UnixListener,
    service: Arc<Service>,
}

impl SocketServer {
    pub async fn bind(path: &std::path::Path, service: Arc<Service>) -> Result<Self> {
        if path.exists() {
            std::fs::remove_file(path).with_context(|| format!("rm {}", path.display()))?;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let listener = UnixListener::bind(path)
            .with_context(|| format!("bind {}", path.display()))?;
        // Lock down permissions to user-only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, perms).ok();
        }
        Ok(Self { listener, service })
    }

    pub async fn run(self) -> Result<()> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            let service = self.service.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, service).await {
                    tracing::warn!("connection ended with error: {e:#}");
                }
            });
        }
    }
}

async fn handle_connection(stream: UnixStream, service: Arc<Service>) -> Result<()> {
    crate::session::run_session(stream, service).await
}

pub async fn open_audit(cfg: &Config) -> Result<(AuditWriter, PathBuf)> {
    let dir = cfg
        .audit
        .dir
        .clone()
        .map(Ok)
        .unwrap_or_else(wilai_core::paths::audit_dir)?;

    // Load Ed25519 audit-signing key if one is present at the default path.
    // Absence is normal (signing is opt-in); a parse error is a hard fail
    // because silently writing unsigned entries would defeat the operator's
    // intent.
    let signer = match wilai_audit::sign::default_priv_path() {
        Ok(p) if p.exists() => match wilai_audit::AuditSigner::load(&p) {
            Ok(s) => {
                tracing::info!(fp = %s.fingerprint(), "audit signing key loaded");
                Some(s)
            }
            Err(e) => return Err(e.context(format!("load signing key {}", p.display()))),
        },
        _ => {
            tracing::info!("no audit signing key; entries will be unsigned");
            None
        }
    };

    let writer = AuditWriter::open_with_signer(&dir, signer)
        .with_context(|| format!("open audit dir {}", dir.display()))?;
    Ok((writer, dir))
}

pub fn default_socket_path() -> Result<PathBuf> {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            let uid = unsafe { libc_getuid() };
            Some(PathBuf::from(format!("/run/user/{uid}")))
        })
        .ok_or_else(|| anyhow!("no XDG_RUNTIME_DIR"))?;
    Ok(runtime.join("wilai.sock"))
}

#[cfg(unix)]
unsafe fn libc_getuid() -> u32 {
    extern "C" {
        fn getuid() -> u32;
    }
    getuid()
}

#[cfg(not(unix))]
unsafe fn libc_getuid() -> u32 {
    0
}
