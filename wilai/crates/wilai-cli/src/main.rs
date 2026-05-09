mod audit_cmd;
mod chat;
mod schema;

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
    /// Verify the hash chain.
    Verify,
}

#[derive(Subcommand)]
enum ToolCmd {
    /// List loaded tools.
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cli.log)))
        .with_target(false)
        .init();

    match cli.cmd {
        Cmd::Chat { once, model, provider } => chat::run(once, model, provider).await,
        Cmd::Audit { sub } => match sub {
            AuditCmd::Tail { n } => audit_cmd::tail(n),
            AuditCmd::Show { session } => audit_cmd::show(&session),
            AuditCmd::Verify => audit_cmd::verify(),
        },
        Cmd::Tool { sub } => match sub {
            ToolCmd::List => list_tools(),
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
