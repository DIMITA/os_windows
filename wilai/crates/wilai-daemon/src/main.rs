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
    let mut registry = wilai_tools::Registry::load_from_dirs(&dirs, &cfg.tools.disabled)?;
    tracing::info!(tool_count = registry.len(), "tools loaded");

    // Bring up MCP servers and merge their tools into the registry under
    // the `mcp.<server>.<tool>` namespace.
    let mcp_clients = wilai_daemon::service::init_mcp(&cfg, &mut registry).await;
    if !mcp_clients.is_empty() {
        tracing::info!(
            mcp_servers = mcp_clients.len(),
            "mcp clients spawned (registry now has {} tools)",
            registry.len()
        );
    }

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

    let service = Arc::new(Service {
        cfg: cfg.clone(),
        registry,
        audit_tx,
        mode,
        default_provider: cfg.general.default_provider,
        default_model: cfg.general.default_model,
        confirm_timeout_s: cfg.general.confirm_timeout_s,
        mcp: mcp_clients,
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
