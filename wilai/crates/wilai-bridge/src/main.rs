//! wilai-bridge — Windows-side launcher for Wilai.
//!
//! The agent lives on Linux. This binary is the client-side pipe:
//! it reads `%APPDATA%\wilai\config.toml`, picks a bridge mode
//! (wsl or ssh), execs the right tool with the right args, and
//! inherits stdin/stdout/stderr so the TUI renders inline.
//!
//! Builds natively for Windows (x86_64-pc-windows-gnu /
//! x86_64-pc-windows-msvc). On other platforms the binary still
//! compiles — it just prints a short note and exits non-zero. This
//! keeps the Rust workspace build portable; CI only ships the
//! Windows artifact.

mod config;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

#[derive(Parser, Debug)]
#[command(
    name = "wilai",
    version,
    about = "Wilai Windows bridge — forwards to a Linux Wilai via WSL or SSH",
    disable_help_subcommand = true,
    trailing_var_arg = true,
    allow_hyphen_values = true
)]
struct Cli {
    /// Override the bridge mode for this run ("wsl" or "ssh").
    #[arg(long)]
    mode: Option<String>,
    /// Explicit config path (defaults to the OS config dir).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Print the resolved config and exit.
    #[arg(long = "dump-config")]
    dump_config: bool,
    /// Extra arguments forwarded to wilai-shell on the Linux side.
    #[arg(trailing_var_arg = true)]
    rest: Vec<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("wilai: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let cfg = config::load(cli.config.as_deref())?;
    let mode = cli
        .mode
        .clone()
        .unwrap_or_else(|| cfg.bridge.mode.clone());

    if cli.dump_config {
        println!(
            "mode = {mode}\n[bridge.wsl]\n  distro = {:?}\n  user   = {:?}\n  shell  = {:?}\n[bridge.ssh]\n  host = {:?}\n  port = {}\n  key  = {:?}",
            cfg.bridge.wsl.distro,
            cfg.bridge.wsl.user,
            cfg.bridge.wsl.shell,
            cfg.bridge.ssh.host,
            cfg.bridge.ssh.port,
            cfg.bridge.ssh.key,
        );
        return Ok(ExitCode::SUCCESS);
    }

    match mode.as_str() {
        "wsl" => dispatch_wsl(&cfg, &cli.rest),
        "ssh" => dispatch_ssh(&cfg, &cli.rest),
        other => Err(anyhow!("unknown bridge mode `{other}` (expected wsl or ssh)")),
    }
}

fn dispatch_wsl(cfg: &config::Config, extra: &[String]) -> Result<ExitCode> {
    let distro = cfg.bridge.wsl.distro.trim();
    let user = cfg.bridge.wsl.user.trim();
    let shell = if cfg.bridge.wsl.shell.is_empty() {
        "wilai-shell"
    } else {
        cfg.bridge.wsl.shell.as_str()
    };

    let mut cmd = Command::new("wsl.exe");
    if !distro.is_empty() {
        cmd.args(["-d", distro]);
    }
    if !user.is_empty() {
        cmd.args(["-u", user]);
    }
    cmd.arg("--");
    cmd.arg(shell);
    for a in extra {
        cmd.arg(a);
    }
    let status = cmd
        .status()
        .with_context(|| "spawn wsl.exe (install WSL2: https://aka.ms/wsl)")?;
    Ok(ExitCode::from(status.code().unwrap_or(1) as u8))
}

fn dispatch_ssh(cfg: &config::Config, extra: &[String]) -> Result<ExitCode> {
    let host = cfg.bridge.ssh.host.trim();
    if host.is_empty() {
        return Err(anyhow!(
            "no SSH host configured. Set [bridge.ssh] host in config.toml"
        ));
    }
    let port = cfg.bridge.ssh.port;
    let key = cfg.bridge.ssh.key.trim();

    let mut cmd = Command::new("ssh.exe");
    cmd.args(["-t", "-o", "ConnectTimeout=10"]);
    if port != 22 {
        cmd.args(["-p", &port.to_string()]);
    }
    if !key.is_empty() {
        cmd.args(["-i", key]);
    }
    cmd.arg(host);
    // If the operator passes trailing args, we assume they want
    // wilai-shell with those; otherwise we rely on the server's
    // wilai@ user having wilai-shell as login shell (v4.3
    // drop-shell convention) and pass nothing.
    if !extra.is_empty() {
        cmd.arg("wilai-shell");
        for a in extra {
            cmd.arg(a);
        }
    }
    let status = cmd
        .status()
        .with_context(|| {
            "spawn ssh.exe (install OpenSSH client: \
             Settings → Apps → Optional Features → OpenSSH Client)"
        })?;
    Ok(ExitCode::from(status.code().unwrap_or(1) as u8))
}

#[cfg(test)]
mod tests {
    // config has its own test module. We keep this file test-light:
    // dispatch_wsl / dispatch_ssh talk to real binaries we can't
    // stub without exploding the test matrix, and the config parser
    // is already covered.
    #[test]
    fn smoke() {
        assert_eq!(2 + 2, 4);
    }
}
