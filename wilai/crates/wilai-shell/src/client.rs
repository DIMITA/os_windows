//! Daemon client half: owns the UnixStream, parses ServerEvents into
//! commands the UI loop consumes, and sends ClientOps on request.

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use wilai_daemon::protocol::{ClientOp, ConfirmReply, ServerEvent};

pub struct DaemonClient {
    pub reader: BufReader<OwnedReadHalf>,
    pub writer: OwnedWriteHalf,
}

impl DaemonClient {
    pub async fn connect(socket: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket)
            .await
            .with_context(|| format!("connect {}", socket.display()))?;
        let (r, w) = stream.into_split();
        Ok(Self { reader: BufReader::new(r), writer: w })
    }

    pub async fn next_event(&mut self) -> Result<ServerEvent> {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .await
            .context("read event")?;
        if n == 0 {
            return Err(anyhow!("daemon closed"));
        }
        serde_json::from_str(line.trim()).with_context(|| format!("parse event: {line}"))
    }

    pub async fn send(&mut self, op: &ClientOp) -> Result<()> {
        let bytes = serde_json::to_vec(op)?;
        self.writer.write_all(&bytes).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }

    pub async fn send_prompt(&mut self, text: String, model: Option<String>, provider: Option<String>) -> Result<()> {
        self.send(&ClientOp::Prompt { text, model, provider }).await
    }

    pub async fn send_confirm(&mut self, id: String, yes: bool) -> Result<()> {
        let answer = if yes { ConfirmReply::Yes } else { ConfirmReply::No };
        self.send(&ClientOp::ConfirmAnswer { id, answer }).await
    }

    pub async fn send_quit(&mut self) -> Result<()> {
        self.send(&ClientOp::Quit).await
    }
}
