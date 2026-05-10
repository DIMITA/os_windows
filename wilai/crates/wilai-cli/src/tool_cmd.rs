use anyhow::{anyhow, Context, Result};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use wilai_daemon::protocol::{ClientOp, ServerEvent};

pub async fn list_via_daemon(socket: &Path) -> Result<()> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connect {}", socket.display()))?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // Skip the SessionStart handshake.
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let op = ClientOp::ToolsList;
    let mut bytes = serde_json::to_vec(&op)?;
    bytes.push(b'\n');
    write_half.write_all(&bytes).await?;
    write_half.flush().await?;

    line.clear();
    reader.read_line(&mut line).await?;
    let ev: ServerEvent = serde_json::from_str(line.trim())
        .with_context(|| format!("parse: {line}"))?;
    let tools = match ev {
        ServerEvent::Tools { tools } => tools,
        ServerEvent::Error { message } => return Err(anyhow!("daemon: {message}")),
        other => return Err(anyhow!("unexpected event: {other:?}")),
    };

    if tools.is_empty() {
        println!("(no tools loaded)");
    } else {
        for t in tools {
            println!(
                "{:35}  v{}  {:>11}  {:>8}  [{}]  {}",
                t.name, t.version, t.category, t.risk, t.origin, t.description
            );
        }
    }

    let quit = ClientOp::Quit;
    let mut b = serde_json::to_vec(&quit)?;
    b.push(b'\n');
    let _ = write_half.write_all(&b).await;
    Ok(())
}
