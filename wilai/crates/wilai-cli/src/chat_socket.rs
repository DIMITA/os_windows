use anyhow::{anyhow, Context, Result};
use std::io::{BufRead as _, Write as _};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use wilai_daemon::protocol::{ClientOp, ConfirmReply, ServerEvent};

pub async fn run(
    socket: &Path,
    once: Option<String>,
    model: Option<String>,
    provider: Option<String>,
) -> Result<()> {
    let stream = match UnixStream::connect(socket).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
            || e.kind() == std::io::ErrorKind::ConnectionRefused =>
        {
            spawn_daemon(socket).await?;
            UnixStream::connect(socket)
                .await
                .with_context(|| format!("connect {} after spawn", socket.display()))?
        }
        Err(e) => return Err(e).with_context(|| format!("connect {}", socket.display())),
    };
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // Wait for SessionStart so the user knows we're ready.
    let session = wait_session(&mut reader).await?;
    eprintln!("connected to wilai-daemon, session={}", session);

    if let Some(prompt) = once {
        send_op(
            &mut write_half,
            &ClientOp::Prompt {
                text: prompt,
                model: model.clone(),
                provider: provider.clone(),
            },
        )
        .await?;
        drive_until_done(&mut reader, &mut write_half).await?;
        send_op(&mut write_half, &ClientOp::Quit).await?;
        return Ok(());
    }

    println!("Wilai - daemon mode. Ctrl-D or :quit to exit.");
    let stdin = std::io::stdin();
    loop {
        {
            let stdout = std::io::stdout();
            let mut h = stdout.lock();
            write!(h, "\n> ")?;
            h.flush()?;
        }
        let mut line = String::new();
        let n = stdin.lock().read_line(&mut line)?;
        if n == 0 {
            break;
        }
        let prompt = line.trim().to_string();
        if prompt.is_empty() {
            continue;
        }
        if prompt == ":quit" || prompt == ":exit" {
            break;
        }
        if let Err(e) = send_op(
            &mut write_half,
            &ClientOp::Prompt {
                text: prompt,
                model: model.clone(),
                provider: provider.clone(),
            },
        )
        .await
        {
            eprintln!("send failed: {e:#}");
            break;
        }
        if let Err(e) = drive_until_done(&mut reader, &mut write_half).await {
            eprintln!("error: {e:#}");
        }
    }
    let _ = send_op(&mut write_half, &ClientOp::Quit).await;
    Ok(())
}

async fn wait_session<R: AsyncBufReadExt + Unpin>(reader: &mut R) -> Result<String> {
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Err(anyhow!("daemon closed before session.start"));
    }
    let ev: ServerEvent = serde_json::from_str(line.trim())
        .with_context(|| format!("parse handshake: {line}"))?;
    match ev {
        ServerEvent::SessionStart { session } => Ok(session),
        ServerEvent::Error { message } => Err(anyhow!("daemon error: {message}")),
        other => Err(anyhow!("expected session.start, got {:?}", other)),
    }
}

async fn drive_until_done<R, W>(reader: &mut R, writer: &mut W) -> Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(anyhow!("daemon closed mid-turn"));
        }
        let ev: ServerEvent = serde_json::from_str(line.trim())
            .with_context(|| format!("parse event: {line}"))?;
        match ev {
            ServerEvent::Text { content } => {
                println!("{content}");
            }
            ServerEvent::ToolCall { name, args } => {
                eprintln!("[tool call] {name} {args}");
            }
            ServerEvent::ToolResult { name, ok, output } => {
                let mark = if ok { "ok" } else { "err" };
                let snippet = if output.len() > 200 {
                    format!("{}... (truncated)", &output[..200])
                } else {
                    output
                };
                eprintln!("[tool {mark}] {name}: {snippet}");
            }
            ServerEvent::ConfirmAsk { id, prompt, default, timeout_s: _ } => {
                let hint = match default.as_str() {
                    "yes" => "[Y/n]",
                    _ => "[y/N]",
                };
                eprint!("{prompt} {hint} ");
                use std::io::Write as _;
                let _ = std::io::stderr().flush();
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                let yes = matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes");
                let reply = if yes { ConfirmReply::Yes } else { ConfirmReply::No };
                send_op(writer, &ClientOp::ConfirmAnswer { id, answer: reply }).await?;
            }
            ServerEvent::TurnDone => return Ok(()),
            ServerEvent::Error { message } => {
                eprintln!("error: {message}");
                return Ok(());
            }
            ServerEvent::Bye => return Ok(()),
            ServerEvent::SessionStart { .. } => {}
            ServerEvent::Mode { current, pentest_in_flight } => {
                eprintln!("[mode] {current} (pentest in flight: {pentest_in_flight})");
            }
        }
    }
}

async fn spawn_daemon(socket: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    let daemon_bin = exe
        .parent()
        .map(|p| p.join("wilai-daemon"))
        .unwrap_or_else(|| std::path::PathBuf::from("wilai-daemon"));
    let bin = if daemon_bin.exists() {
        daemon_bin
    } else {
        std::path::PathBuf::from("wilai-daemon")
    };
    eprintln!("starting wilai-daemon...");
    std::process::Command::new(&bin)
        .arg("--socket")
        .arg(socket)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("spawn {}", bin.display()))?;
    // Wait for the socket to appear; bounded retry.
    for _ in 0..40 {
        if socket.exists() && UnixStream::connect(socket).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    Err(anyhow!(
        "daemon failed to bind {} within 2s",
        socket.display()
    ))
}

async fn send_op<W: AsyncWriteExt + Unpin>(writer: &mut W, op: &ClientOp) -> Result<()> {
    let bytes = serde_json::to_vec(op)?;
    writer.write_all(&bytes).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}
