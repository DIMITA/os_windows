use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;
use wilai_daemon::detect::{spawn as spawn_detect, DetectConfig};
use wilai_daemon::mode_mgr::ModeManager;
use wilai_daemon::service::{collect_tool_dirs, default_socket_path, open_audit, Service, SocketServer};

#[derive(Parser)]
#[command(name = "wilai-daemon", version, about = "Wilai system agent daemon")]
struct Cli {
    /// Override the socket path. Defaults to $XDG_RUNTIME_DIR/wilai.sock.
    #[arg(long)]
    socket: Option<std::path::PathBuf>,
    /// Disable pentest auto-detection regardless of config.
    #[arg(long)]
    no_auto_detect: bool,
    /// Logging filter (e.g. info, debug).
    #[arg(long, default_value = "info")]
    log: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cli.log)),
        )
        .with_target(false)
        .init();

    let cfg = wilai_core::Config::load()?;
    let dirs = collect_tool_dirs(&cfg);
    let registry = wilai_tools::Registry::load_from_dirs(&dirs, &cfg.tools.disabled)?;
    tracing::info!(tool_count = registry.len(), "tools loaded");
    let registry = std::sync::Arc::new(tokio::sync::RwLock::new(registry));

    // Bring up MCP servers and merge their tools into the registry under
    // the `mcp.<server>.<tool>` namespace. Servers that come up late
    // are picked up by the backfill task below.
    let mcp_clients = wilai_daemon::service::init_mcp(&cfg, &registry).await;
    if !mcp_clients.is_empty() {
        tracing::info!(
            mcp_servers = mcp_clients.len(),
            "mcp clients spawned (registry now has {} tools)",
            registry.read().await.len()
        );
    }

    // Periodic backfill: catches tools from supervisors that bring their
    // servers up after the init_mcp grace period expired.
    let tools_changed = std::sync::Arc::new(tokio::sync::Notify::new());
    let _backfill_handle = wilai_daemon::service::spawn_backfill_task(
        cfg.clone(),
        registry.clone(),
        mcp_clients.clone(),
        tools_changed.clone(),
        30,
    );

    let (audit, audit_dir) = open_audit(&cfg).await?;
    tracing::info!(dir = %audit_dir.display(), "audit ready");
    let (audit_tx, audit_handle) = audit.spawn_task();

    let mode = ModeManager::new(wilai_core::Mode::Normal, audit_tx.clone());

    let auto_detect_handle = if cli.no_auto_detect || !cfg.mode.auto_detect {
        tracing::info!("auto-detect disabled");
        None
    } else {
        let detect_cfg = DetectConfig {
            threshold: cfg.mode.threshold,
            ..DetectConfig::default()
        };
        tracing::info!(threshold = detect_cfg.threshold, "starting auto-detect");
        Some(spawn_detect(mode.clone(), detect_cfg))
    };

    // Open local memory store + embedder (none when Ollama unreachable).
    let (memory, embedder) = match wilai_daemon::service::open_memory(&cfg).await {
        Some((m, e)) => (Some(m), Some(e)),
        None => (None, None),
    };

    // Daily memory compaction. Keeps SQLite from drifting into
    // Windows-sized state. Skipped when no memory store is open.
    let _memory_compact_handle = if let Some(m) = &memory {
        if cfg.memory.compact_enabled {
            Some(wilai_daemon::service::spawn_memory_compact(cfg.memory.clone(), m.clone()))
        } else { None }
    } else { None };

    // Proactive-nudge broadcast + quota, consumed by background watchers.
    let (nudge_tx, _nudge_rx) = tokio::sync::broadcast::channel::<wilai_daemon::nudge::Nudge>(64);
    let nudge_quota = std::sync::Arc::new(wilai_daemon::nudge::NudgeQuota::new(
        cfg.proactivity.quota_per_hour,
    ));
    let _watcher_handles = wilai_daemon::watchers::spawn_all(
        cfg.proactivity.clone(),
        nudge_tx.clone(),
        nudge_quota.clone(),
    );
    if cfg.proactivity.enabled {
        tracing::info!(
            quota = cfg.proactivity.quota_per_hour,
            "proactivity enabled; {} watcher(s) running",
            _watcher_handles.len()
        );
    }

    let service = Arc::new(Service {
        cfg: cfg.clone(),
        registry,
        audit_tx,
        mode,
        default_provider: cfg.general.default_provider,
        default_model: cfg.general.default_model,
        confirm_timeout_s: cfg.general.confirm_timeout_s,
        mcp: mcp_clients,
        tools_changed,
        memory,
        embedder,
        nudges: nudge_tx,
    });

    let socket_path = cli.socket.unwrap_or(default_socket_path()?);
    tracing::info!(socket = %socket_path.display(), "binding socket");
    let server = SocketServer::bind(&socket_path, service).await?;

    tokio::select! {
        r = server.run() => {
            if let Err(e) = r {
                tracing::error!("server: {e:#}");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("ctrl-c, shutting down");
        }
    }

    if let Some(h) = auto_detect_handle {
        h.abort();
    }
    let _ = std::fs::remove_file(&socket_path);
    drop(audit_handle);
    Ok(())
}
