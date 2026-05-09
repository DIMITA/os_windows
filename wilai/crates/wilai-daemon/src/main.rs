use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;
use wilai_daemon::service::{collect_tool_dirs, default_socket_path, open_audit, Service, SocketServer};

#[derive(Parser)]
#[command(name = "wilai-daemon", version, about = "Wilai system agent daemon")]
struct Cli {
    /// Override the socket path. Defaults to $XDG_RUNTIME_DIR/wilai.sock.
    #[arg(long)]
    socket: Option<std::path::PathBuf>,
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

    let (audit, audit_dir) = open_audit(&cfg).await?;
    tracing::info!(dir = %audit_dir.display(), "audit ready");
    let (audit_tx, audit_handle) = audit.spawn_task();

    let service = Arc::new(Service {
        cfg: cfg.clone(),
        registry,
        audit_tx,
        default_provider: cfg.general.default_provider,
        default_model: cfg.general.default_model,
        confirm_timeout_s: cfg.general.confirm_timeout_s,
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

    let _ = std::fs::remove_file(&socket_path);
    drop(audit_handle);
    Ok(())
}
