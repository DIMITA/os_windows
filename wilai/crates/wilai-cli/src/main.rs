mod audit_cmd;
mod chat;
mod chat_socket;
mod mode_cmd;
mod schema;
mod tool_authoring;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "wilai", version, about = "Wilai - WilOS system agent")]
struct Cli {
    #[arg(long, global = true, default_value = "warn")]
    log: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a chat session.
    Chat {
        /// Single-shot prompt; if omitted, an interactive prompt is started.
        #[arg(long, value_name = "TEXT")]
        once: Option<String>,
        /// Override the configured model.
        #[arg(long)]
        model: Option<String>,
        /// Override the configured provider.
        #[arg(long)]
        provider: Option<String>,
        /// Use the running wilai-daemon over its Unix socket. Auto-detects
        /// the default path; pass --socket to override.
        #[arg(long)]
        daemon: bool,
        /// Path to the daemon socket. Implies --daemon.
        #[arg(long)]
        socket: Option<std::path::PathBuf>,
    },
    /// Audit log inspection.
    Audit {
        #[command(subcommand)]
        sub: AuditCmd,
    },
    /// Tool registry inspection.
    Tool {
        #[command(subcommand)]
        sub: ToolCmd,
    },
    /// Mode inspection and switching (talks to wilai-daemon).
    Mode {
        #[command(subcommand)]
        sub: ModeCmd,
    },
}

#[derive(Subcommand)]
enum ModeCmd {
    /// Print the daemon's current mode.
    Show {
        #[arg(long)]
        socket: Option<std::path::PathBuf>,
        /// Emit a single line of JSON instead of a human-readable form.
        /// Suitable for poll-based status bars.
        #[arg(long)]
        json: bool,
    },
    /// Switch the daemon to <mode> (normal | pentest).
    Set {
        mode: String,
        #[arg(long)]
        socket: Option<std::path::PathBuf>,
        #[arg(long)]
        trigger: Option<String>,
    },
}

#[derive(Subcommand)]
enum AuditCmd {
    /// Print the last N lines of the audit log.
    Tail {
        #[arg(short = 'n', default_value_t = 20)]
        n: usize,
    },
    /// Show entries for a session id.
    Show { session: String },
    /// Verify the hash chain (and signatures, if a public key is present).
    Verify {
        /// Treat missing signatures as failures (default: warn-only).
        #[arg(long)]
        require_sig: bool,
    },
    /// Generate a fresh Ed25519 audit-signing keypair under
    /// ~/.local/share/wilai/keys/. Refuses to overwrite an existing key.
    Keygen {
        /// Path to write the private key to.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
enum ToolCmd {
    /// List loaded tools.
    List,
    /// Scaffold a new tool YAML at the given path.
    New {
        /// Dotted name of the tool (e.g. "fs.snapshot").
        name: String,
        /// Output path. Defaults to ~/.config/wilai/tools/<name>.yaml.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Category to use in the scaffold.
        #[arg(long, default_value = "read")]
        category: String,
        /// Risk level to use in the scaffold.
        #[arg(long, default_value = "none")]
        risk: String,
    },
    /// Validate a tool YAML against the meta-schema.
    Validate {
        path: std::path::PathBuf,
    },
    /// Render the executor's argv (subprocess) or builtin call (builtin)
    /// without executing the tool. Args come from --args (JSON object).
    DryRun {
        path: std::path::PathBuf,
        /// JSON object of arguments. Default: {}.
        #[arg(long, default_value = "{}")]
        args: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cli.log)))
        .with_target(false)
        .init();

    match cli.cmd {
        Cmd::Chat { once, model, provider, daemon, socket } => {
            let use_socket = daemon || socket.is_some();
            if use_socket {
                let path = match socket {
                    Some(p) => p,
                    None => wilai_daemon::service::default_socket_path()?,
                };
                chat_socket::run(&path, once, model, provider).await
            } else {
                chat::run(once, model, provider).await
            }
        }
        Cmd::Audit { sub } => match sub {
            AuditCmd::Tail { n } => audit_cmd::tail(n),
            AuditCmd::Show { session } => audit_cmd::show(&session),
            AuditCmd::Verify { require_sig } => audit_cmd::verify(require_sig),
            AuditCmd::Keygen { out } => audit_cmd::keygen(out.as_deref()),
        },
        Cmd::Tool { sub } => match sub {
            ToolCmd::List => list_tools(),
            ToolCmd::New { name, out, category, risk } => {
                tool_authoring::new(&name, out.as_deref(), &category, &risk)
            }
            ToolCmd::Validate { path } => tool_authoring::validate(&path),
            ToolCmd::DryRun { path, args } => tool_authoring::dry_run(&path, &args),
        },
        Cmd::Mode { sub } => match sub {
            ModeCmd::Show { socket, json } => {
                let path = match socket {
                    Some(p) => p,
                    None => wilai_daemon::service::default_socket_path()?,
                };
                mode_cmd::show(&path, json).await
            }
            ModeCmd::Set { mode, socket, trigger } => {
                let path = match socket {
                    Some(p) => p,
                    None => wilai_daemon::service::default_socket_path()?,
                };
                mode_cmd::set(&path, &mode, trigger.as_deref()).await
            }
        },
    }
}

fn list_tools() -> Result<()> {
    let cfg = wilai_core::Config::load()?;
    let dirs = collect_tool_dirs(&cfg);
    let reg = wilai_tools::Registry::load_from_dirs(&dirs, &cfg.tools.disabled)?;
    if reg.is_empty() {
        println!("(no tools loaded)");
        return Ok(());
    }
    for (name, spec) in reg.iter() {
        println!(
            "{:30}  v{}  {:>11}  {:>8}  {}",
            name, spec.version, spec.category, spec.risk, spec.description
        );
    }
    Ok(())
}

pub(crate) fn collect_tool_dirs(cfg: &wilai_core::Config) -> Vec<std::path::PathBuf> {
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
