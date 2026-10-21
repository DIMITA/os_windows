use anyhow::{anyhow, Context, Result};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use wilai_daemon::protocol::{ClientOp, McpServerInfo, ServerEvent};

pub async fn list(socket: &Path, json: bool) -> Result<()> {
    let servers = query(socket, ClientOp::McpList).await?;
    render(&servers, json)
}

pub async fn restart(socket: &Path, server: &str) -> Result<()> {
    let servers = query(
        socket,
        ClientOp::McpRestart {
            server: server.to_string(),
        },
    )
    .await?;
    render(&servers, false)
}

pub async fn sync(socket: &Path) -> Result<()> {
    let servers = query(socket, ClientOp::McpResync).await?;
    render(&servers, false)
}

async fn query(socket: &Path, op: ClientOp) -> Result<Vec<McpServerInfo>> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connect {}", socket.display()))?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // Skip SessionStart.
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let mut bytes = serde_json::to_vec(&op)?;
    bytes.push(b'\n');
    write_half.write_all(&bytes).await?;
    write_half.flush().await?;

    let servers = loop {
        line.clear();
        reader.read_line(&mut line).await?;
        let ev: ServerEvent = serde_json::from_str(line.trim())
            .with_context(|| format!("parse: {line}"))?;
        match ev {
            ServerEvent::McpServers { servers } => break servers,
            // Nudges may arrive asynchronously; skip and keep reading.
            ServerEvent::Nudge { .. } => continue,
            ServerEvent::Error { message } => return Err(anyhow!("daemon: {message}")),
            other => return Err(anyhow!("unexpected event: {other:?}")),
        }
    };

    let quit = ClientOp::Quit;
    let mut b = serde_json::to_vec(&quit)?;
    b.push(b'\n');
    let _ = write_half.write_all(&b).await;

    Ok(servers)
}

fn render(servers: &[McpServerInfo], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(servers)?);
        return Ok(());
    }
    if servers.is_empty() {
        println!("(no mcp servers configured)");
        return Ok(());
    }
    println!(
        "{:<20} {:<6} {:>8} {:>8} {:>6} {:<7}",
        "NAME", "STATUS", "UPTIME", "RESTARTS", "TOOLS", "PENTEST"
    );
    for s in servers {
        let status = if s.fatal {
            "FATAL"
        } else if s.alive {
            "alive"
        } else {
            "down"
        };
        let uptime = match s.uptime_s {
            Some(0) => "0s".to_string(),
            Some(u) if u < 60 => format!("{u}s"),
            Some(u) if u < 3600 => format!("{}m", u / 60),
            Some(u) => format!("{}h", u / 3600),
            None => "-".to_string(),
        };
        let gate = if s.gated_in_pentest { "gated" } else { "open" };
        println!(
            "{:<20} {:<6} {:>8} {:>8} {:>6} {:<7}",
            s.name, status, uptime, s.restart_count, s.tool_count, gate,
        );
        if let Some(err) = &s.last_error {
            println!("  last_error: {err}");
        }
    }
    Ok(())
}
