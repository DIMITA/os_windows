use anyhow::{anyhow, Context, Result};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use wilai_daemon::protocol::{ClientOp, ServerEvent};

pub async fn show(socket: &Path) -> Result<()> {
    let mut stream = connect(socket).await?;
    write_op(&mut stream, &ClientOp::ModeGet).await?;
    let ev = next_event_skipping_session(&mut stream).await?;
    match ev {
        ServerEvent::Mode { current, pentest_in_flight } => {
            println!("mode: {current}");
            println!("pentest_in_flight: {pentest_in_flight}");
        }
        ServerEvent::Error { message } => return Err(anyhow!("daemon: {message}")),
        other => return Err(anyhow!("unexpected event: {:?}", other)),
    }
    write_op(&mut stream, &ClientOp::Quit).await?;
    Ok(())
}

pub async fn set(socket: &Path, mode: &str, trigger: Option<&str>) -> Result<()> {
    if mode != "normal" && mode != "pentest" {
        return Err(anyhow!("mode must be `normal` or `pentest`"));
    }
    let mut stream = connect(socket).await?;
    write_op(
        &mut stream,
        &ClientOp::ModeSet {
            to: mode.to_string(),
            trigger: trigger.map(|s| s.to_string()),
        },
    )
    .await?;
    let ev = next_event_skipping_session(&mut stream).await?;
    match ev {
        ServerEvent::Mode { current, .. } => {
            println!("mode: {current}");
        }
        ServerEvent::Error { message } => return Err(anyhow!("daemon refused: {message}")),
        other => return Err(anyhow!("unexpected event: {:?}", other)),
    }
    write_op(&mut stream, &ClientOp::Quit).await?;
    Ok(())
}

struct Stream {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

async fn connect(socket: &Path) -> Result<Stream> {
    let s = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connect {}", socket.display()))?;
    let (r, w) = s.into_split();
    Ok(Stream {
        reader: BufReader::new(r),
        writer: w,
    })
}

async fn next_event_skipping_session(s: &mut Stream) -> Result<ServerEvent> {
    loop {
        let mut line = String::new();
        let n = s.reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(anyhow!("daemon closed"));
        }
        let ev: ServerEvent = serde_json::from_str(line.trim())
            .with_context(|| format!("parse: {line}"))?;
        match ev {
            ServerEvent::SessionStart { .. } => continue,
            other => return Ok(other),
        }
    }
}

async fn write_op(s: &mut Stream, op: &ClientOp) -> Result<()> {
    let mut bytes = serde_json::to_vec(op)?;
    bytes.push(b'\n');
    s.writer.write_all(&bytes).await?;
    s.writer.flush().await?;
    Ok(())
}
