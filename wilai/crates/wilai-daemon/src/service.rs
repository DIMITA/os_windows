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
use wilai_memory::{Embedder, MemoryStore, OllamaEmbedder};
use wilai_providers::{AnthropicProvider, OllamaProvider, Provider};
use wilai_tools::Registry;

pub struct Service {
    pub cfg: Config,
    pub registry: Arc<RwLock<Registry>>,
    pub audit_tx: mpsc::Sender<WriteRequest>,
    pub mode: Arc<ModeManager>,
    pub default_provider: String,
    pub default_model: String,
    pub confirm_timeout_s: u32,
    pub mcp: HashMap<String, Arc<McpSupervisor>>,
    /// Notified every time the backfill task or an on-demand resync
    /// inserts new tools into the registry. Sessions subscribe to this
    /// and push `ServerEvent::ToolsAdded` to the client when it fires.
    pub tools_changed: Arc<tokio::sync::Notify>,
    /// Local memory store (SQLite). None when memory is disabled; the
    /// memory.* tools then return a clean error rather than hanging.
    pub memory: Option<Arc<MemoryStore>>,
    /// Embedder for memory.note / memory.recall. Implementations:
    /// OllamaEmbedder in production, HashEmbedder in tests.
    pub embedder: Option<Arc<dyn Embedder>>,
    /// Broadcast channel for proactive nudges. Each session loop
    /// subscribes a receiver and forwards every nudge to its client as
    /// a `ServerEvent::Nudge`.
    pub nudges: tokio::sync::broadcast::Sender<crate::nudge::Nudge>,
    /// Active pentest project (if any). pt.project.start sets it, pt.project.stop clears it.
    pub project: wilai_pentest::ProjectHandle,
    /// Project-local SQLite store (findings, targets). Opened by
    /// pt.project.start alongside the project.
    pub project_store: Arc<tokio::sync::RwLock<Option<Arc<wilai_pentest::ProjectStore>>>>,
}

impl Service {
    /// Build a provider, honoring both the security.local_only lock
    /// (refuse non-local providers unconditionally) and the pentest-mode
    /// cloud kill-switch (refuse non-local when the mode is Pentest).
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
        if !provider.is_local() {
            if self.cfg.security.local_only {
                return Err(anyhow!(
                    "provider `{name}` is non-local; security.local_only is set, refusing"
                ));
            }
            if self.mode.current().await == Mode::Pentest {
                return Err(anyhow!(
                    "provider `{name}` is non-local; cloud kill-switch is active in pentest mode"
                ));
            }
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
/// registry doesn't have yet. Returns the list of newly-inserted tool
/// names (full `mcp.<server>.<tool>` form). Called at startup grace-period
/// expiry, after a manual restart, and periodically from a background
/// task so tools that take long to become available still land in the
/// registry without a daemon restart.
pub async fn backfill_mcp_tools(
    cfg: &Config,
    registry: &Arc<RwLock<Registry>>,
    mcp: &HashMap<String, Arc<McpSupervisor>>,
) -> Vec<String> {
    let mut added: Vec<String> = Vec::new();
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
                    added.push(tool_name);
                }
                Err(e) => tracing::warn!(server = %name, tool = %desc.name, "skip mcp tool: {e:#}"),
            }
        }
    }
    if !added.is_empty() {
        tracing::info!(count = added.len(), "mcp backfill: new tools registered");
    }
    added
}

/// Background task that calls `backfill_mcp_tools` on a fixed interval,
/// notifying the provided tools_changed when it adds anything.
pub fn spawn_backfill_task(
    cfg: Config,
    registry: Arc<RwLock<Registry>>,
    mcp: HashMap<String, Arc<McpSupervisor>>,
    tools_changed: Arc<tokio::sync::Notify>,
    interval_s: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let dur = std::time::Duration::from_secs(interval_s);
        loop {
            tokio::time::sleep(dur).await;
            let added = backfill_mcp_tools(&cfg, &registry, &mcp).await;
            if !added.is_empty() {
                tools_changed.notify_waiters();
            }
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
        .unwrap_or_else(wilai_core::paths::audit_dir)?;    // Load Ed25519 audit-signing key if one is present at the default path.
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

    // Optional at-rest encryption key for rotated audit files. The
    // hot file stays plain; rotated ones get ChaCha20-Poly1305'd.
    let enc_key = if cfg.security.audit_encrypt_rotated {
        let path = if cfg.security.audit_key_file.is_empty() {
            dir.join("audit-key")
        } else {
            PathBuf::from(&cfg.security.audit_key_file)
        };
        match wilai_audit::encrypt::load_or_create_key(&path) {
            Ok(k) => {
                tracing::info!(key_file = %path.display(), "audit rotation encryption on");
                Some(k)
            }
            Err(e) => {
                tracing::warn!(
                    "audit rotation encryption requested but key load failed: {e:#}; staying plain"
                );
                None
            }
        }
    } else {
        None
    };

    let writer = AuditWriter::open_full(&dir, signer, enc_key)
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

/// Open (or create) the local memory store under
/// $XDG_DATA_HOME/wilai/memory/wilai.sqlite and build its embedder
/// against the configured Ollama endpoint. Returns None if the embedder
/// cannot be constructed (e.g. no provider configured); the daemon
/// stays up without memory in that case rather than refusing to boot.
pub async fn open_memory(
    cfg: &Config,
) -> Option<(Arc<MemoryStore>, Arc<dyn Embedder>)> {
    let data_dir = wilai_core::paths::data_dir().ok()?;
    let path = data_dir.join("memory").join("wilai.sqlite");
    let store = match MemoryStore::open(&path) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            tracing::warn!("memory store disabled: {e:#}");
            return None;
        }
    };

    // Embedder: use whichever provider has `local: true` first. Default
    // to ollama at 127.0.0.1:11434 if no match.
    let (url, model) = cfg
        .providers
        .iter()
        .find(|(_, p)| p.kind == "ollama" && p.local)
        .map(|(_, p)| (
            p.url.clone().unwrap_or_else(|| "http://127.0.0.1:11434".to_string()),
            "nomic-embed-text".to_string(),
        ))
        .unwrap_or_else(|| (
            "http://127.0.0.1:11434".to_string(),
            "nomic-embed-text".to_string(),
        ));
    match OllamaEmbedder::new(url, model) {
        Ok(e) => {
            tracing::info!(embedder = e.name(), "memory ready");
            Some((store, Arc::new(e) as Arc<dyn Embedder>))
        }
        Err(e) => {
            tracing::warn!("embedder init failed: {e:#}; memory unavailable");
            None
        }
    }
}

/// Daily task that applies the configured compaction policy to the
/// memory store. Logs a one-line report after each sweep so operators
/// can see what Wilai pruned. The runtime periodicity + policy knobs
/// come from [memory] in config.
pub fn spawn_memory_compact(
    cfg: wilai_core::MemoryConfig,
    store: Arc<MemoryStore>,
) -> tokio::task::JoinHandle<()> {
    use std::time::Duration;
    tokio::spawn(async move {
        let period = Duration::from_secs(cfg.compact_every_h.max(1) as u64 * 3600);
        // Wait one period before the first pass so a freshly started
        // daemon doesn't hit the DB immediately. Then loop.
        tokio::time::sleep(period).await;
        loop {
            let policy = wilai_memory::CompactPolicy {
                ttl_days: cfg.ttl_days,
                max_notes: cfg.max_notes,
                max_bytes: cfg.max_bytes,
            };
            match store.compact(policy) {
                Ok(r) if r.total() > 0 => {
                    tracing::info!(
                        ttl = r.ttl_deleted,
                        over_count = r.count_deleted,
                        over_bytes = r.bytes_deleted,
                        before = r.before_count,
                        after = r.after_count,
                        vacuumed = r.vacuumed,
                        "memory compacted"
                    );
                }
                Ok(_) => tracing::debug!("memory compact: nothing to prune"),
                Err(e) => tracing::warn!("memory compact failed: {e:#}"),
            }
            tokio::time::sleep(period).await;
        }
    })
}
