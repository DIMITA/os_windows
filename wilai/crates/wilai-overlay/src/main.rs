//! wilai-overlay: a small desktop client that connects to wilai-daemon and
//! renders agent activity as libnotify notifications. Confirms are mediated
//! through zenity (or another configured prompter).
//!
//! Typical use: bind a Hyprland keybind to `wilai-overlay --ask` so a global
//! hotkey opens a "Wilai:" entry box, then the answer streams in as toasts.

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::Command;
use tracing_subscriber::EnvFilter;
use wilai_daemon::protocol::{ClientOp, ConfirmReply, ServerEvent};

#[derive(Parser, Debug)]
#[command(name = "wilai-overlay", version, about = "Wilai desktop overlay")]
struct Cli {
    /// Daemon socket. Defaults to $XDG_RUNTIME_DIR/wilai.sock.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Prompt to send. If unset, an entry box is opened (--entry-cmd).
    #[arg(long)]
    prompt: Option<String>,
    /// Open an interactive entry box before sending. Implied if --prompt is unset.
    #[arg(long)]
    ask: bool,
    /// Command used to ask the user for the prompt. Must print the entered
    /// text to stdout. Defaults to `zenity --entry --title "Wilai" --text "Ask Wilai:"`.
    #[arg(long)]
    entry_cmd: Option<String>,
    /// Command used to ask yes/no questions. The command must exit 0 for "yes",
    /// non-zero for "no". `{prompt}` is substituted with the question text.
    /// Defaults to `zenity --question --title "Wilai" --text "{prompt}"`.
    #[arg(long)]
    confirm_cmd: Option<String>,
    /// Command used to display notifications. `{title}` and `{body}` placeholders
    /// are substituted. Defaults to `notify-send -a Wilai "{title}" "{body}"`.
    #[arg(long)]
    notify_cmd: Option<String>,
    /// Notification urgency (low / normal / critical) for tool errors.
    #[arg(long, default_value = "normal")]
    urgency: String,
    /// Logging filter.
    #[arg(long, default_value = "warn")]
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

    let socket = match &cli.socket {
        Some(p) => p.clone(),
        None => wilai_daemon::service::default_socket_path()?,
    };

    let prompt = match cli.prompt.clone() {
        Some(p) => p,
        None => ask_for_prompt(&cli)
            .await
            .context("no prompt provided and entry box failed")?,
    };
    if prompt.trim().is_empty() {
        return Ok(());
    }

    drive_session(&cli, &socket, &prompt).await
}

async fn ask_for_prompt(cli: &Cli) -> Result<String> {
    let cmd = cli
        .entry_cmd
        .clone()
        .unwrap_or_else(|| "zenity --entry --title \"Wilai\" --text \"Ask Wilai:\"".to_string());
    let out = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .with_context(|| format!("entry cmd: {cmd}"))?;
    if !out.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn drive_session(cli: &Cli, socket: &Path, prompt: &str) -> Result<()> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connect {}", socket.display()))?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half).lines();

    let first = reader
        .next_line()
        .await?
        .ok_or_else(|| anyhow!("daemon closed before session.start"))?;
    let _: ServerEvent = serde_json::from_str(&first)?;

    notify(cli, "Wilai", &format!("> {prompt}"), &cli.urgency).await;

    let op = ClientOp::Prompt {
        text: prompt.to_string(),
        model: None,
        provider: None,
    };
    write_op(&mut write_half, &op).await?;

    while let Some(line) = reader.next_line().await? {
        let ev: ServerEvent = serde_json::from_str(&line)?;
        match ev {
            ServerEvent::SessionStart { .. } => {}
            ServerEvent::Text { content } => {
                notify(cli, "Wilai", &content, &cli.urgency).await;
            }
            ServerEvent::ToolCall { name, args } => {
                let body = format!("{name} {}", trim(&args.to_string(), 240));
                notify(cli, "tool call", &body, "low").await;
            }
            ServerEvent::ToolResult { name, ok, output } => {
                let title = if ok { "tool ok" } else { "tool err" };
                notify(cli, title, &format!("{name}: {}", trim(&output, 240)), "low").await;
            }
            ServerEvent::ConfirmAsk { id, prompt, default: _, timeout_s: _ } => {
                let yes = ask_confirm(cli, &prompt).await;
                let reply = if yes { ConfirmReply::Yes } else { ConfirmReply::No };
                write_op(&mut write_half, &ClientOp::ConfirmAnswer { id, answer: reply }).await?;
            }
            ServerEvent::TurnDone => break,
            ServerEvent::Error { message } => {
                notify(cli, "Wilai error", &message, "critical").await;
                break;
            }
            ServerEvent::Bye => break,
            ServerEvent::Mode { current, .. } => {
                let urgency = if current == "pentest" { "critical" } else { "low" };
                notify(cli, "Wilai mode", &current, urgency).await;
            }
        }
    }

    let _ = write_op(&mut write_half, &ClientOp::Quit).await;
    Ok(())
}

async fn ask_confirm(cli: &Cli, prompt: &str) -> bool {
    let template = cli
        .confirm_cmd
        .clone()
        .unwrap_or_else(|| {
            "zenity --question --title \"Wilai\" --text \"{prompt}\"".to_string()
        });
    let cmd = template.replace("{prompt}", &shell_escape(prompt));
    match Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
    {
        Ok(s) => s.success(),
        Err(e) => {
            tracing::warn!("confirm cmd failed: {e}; defaulting to no");
            false
        }
    }
}

async fn notify(cli: &Cli, title: &str, body: &str, urgency: &str) {
    let template = cli
        .notify_cmd
        .clone()
        .unwrap_or_else(|| {
            r#"notify-send -a Wilai -u {urgency} "{title}" "{body}""#.to_string()
        });
    let cmd = template
        .replace("{title}", &shell_escape(title))
        .replace("{body}", &shell_escape(body))
        .replace("{urgency}", urgency);
    let _ = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

fn shell_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('`', "\\`").replace('$', "\\$")
}

fn trim(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

async fn write_op<W: AsyncWriteExt + Unpin>(w: &mut W, op: &ClientOp) -> Result<()> {
    let mut bytes = serde_json::to_vec(op)?;
    bytes.push(b'\n');
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}
