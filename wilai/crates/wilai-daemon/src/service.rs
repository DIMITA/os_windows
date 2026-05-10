use crate::mode_mgr::ModeManager;
use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use wilai_audit::writer::WriteRequest;
use wilai_audit::AuditWriter;
use wilai_core::{Config, Mode};
use wilai_mcp::McpClient;
use wilai_providers::{AnthropicProvider, OllamaProvider, Provider};
use wilai_tools::Registry;

pub struct Service {
    pub cfg: Config,
    pub registry: Registry,
    pub audit_tx: mpsc::Sender<WriteRequest>,
    pub mode: Arc<ModeManager>,
    pub default_provider: String,
    pub default_model: String,
    pub confirm_timeout_s: u32,
    /// Map from server name to live MCP client. Tools registered as
    /// `mcp.<server>.<tool>` route here.
    pub mcp: HashMap<String, Arc<McpClient>>,
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

/// Spawn each enabled MCP server, list its tools, and merge them into the
/// registry under the prefix `mcp.<server>.<tool>`. Returns a map of live
/// clients that the agent loop can route calls to. A failure to spawn one
/// server is logged but does not abort daemon startup; the rest still come up.
pub async fn init_mcp(
    cfg: &Config,
    registry: &mut Registry,
) -> HashMap<String, Arc<McpClient>> {
    let mut out: HashMap<String, Arc<McpClient>> = HashMap::new();
    for (name, server_cfg) in &cfg.mcp {
        if !server_cfg.enabled {
            tracing::info!(server = %name, "mcp server disabled in config; skipping");
            continue;
        }
        match McpClient::spawn(name, &server_cfg.command, &server_cfg.args, &server_cfg.env).await {
            Ok(client) => match client.list_tools().await {
                Ok(tools) => {
                    let mut count = 0;
                    for desc in &tools {
                        match wilai_mcp::convert::descriptor_to_spec_with_gate(
                            name,
                            desc,
                            server_cfg.gate_in_pentest,
                        ) {
                            Ok(spec) => {
                                registry.insert_dynamic(spec);
                                count += 1;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    server = %name,
                                    tool = %desc.name,
                                    "skip mcp tool: {e:#}"
                                );
                            }
                        }
                    }
                    tracing::info!(
                        server = %name,
                        tools = count,
                        gated = server_cfg.gate_in_pentest,
                        "mcp server ready"
                    );
                    out.insert(name.clone(), client);
                }
                Err(e) => {
                    tracing::error!(server = %name, "mcp tools/list failed: {e:#}");
                }
            },
            Err(e) => {
                tracing::error!(server = %name, "mcp spawn failed: {e:#}");
            }
        }
    }
    out
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
