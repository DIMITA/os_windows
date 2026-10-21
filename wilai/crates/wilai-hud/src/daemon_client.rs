//! Daemon event subscriber. Runs on a tokio thread, posts face-state
//! changes to a shared `AtomicU8` the render loop reads on each frame.

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use wilai_daemon::protocol::{ClientOp, ServerEvent};

use crate::face::FaceState;

pub struct SharedFaceState {
    inner: AtomicU8,
}

impl SharedFaceState {
    pub fn new(initial: FaceState) -> Self {
        Self { inner: AtomicU8::new(initial as u8) }
    }

    pub fn set(&self, s: FaceState) {
        self.inner.store(s as u8, Ordering::Relaxed);
    }

    pub fn get(&self) -> FaceState {
        match self.inner.load(Ordering::Relaxed) {
            0 => FaceState::Idle,
            1 => FaceState::Listening,
            2 => FaceState::Thinking,
            3 => FaceState::Speaking,
            4 => FaceState::Acting,
            5 => FaceState::Alert,
            _ => FaceState::Offline,
        }
    }
}

pub async fn run_subscriber(socket: &Path, state: Arc<SharedFaceState>) -> Result<()> {
    loop {
        match UnixStream::connect(socket).await {
            Ok(stream) => {
                state.set(FaceState::Idle);
                if let Err(e) = drive(stream, &state).await {
                    tracing::warn!("daemon subscriber error: {e:#}; reconnecting");
                }
            }
            Err(e) => {
                tracing::debug!("daemon unreachable: {e}; retrying");
            }
        }
        state.set(FaceState::Offline);
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

async fn drive(stream: UnixStream, state: &Arc<SharedFaceState>) -> Result<()> {
    let (r, mut w) = stream.into_split();
    let mut reader = BufReader::new(r).lines();

    // Handshake.
    let _ = reader.next_line().await?.context("daemon closed before handshake")?;

    // Subscribe passively: we never send a Prompt. We just stay
    // connected and watch events the user generates via the shell.
    // Even in passive mode, SessionStart has been sent so we can read
    // ongoing Mode / Tools events.

    while let Some(line) = reader.next_line().await? {
        let ev: ServerEvent = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        match ev {
            ServerEvent::Text { .. } => state.set(FaceState::Speaking),
            ServerEvent::ToolCall { .. } => state.set(FaceState::Acting),
            ServerEvent::ToolResult { ok: false, .. } => state.set(FaceState::Alert),
            ServerEvent::ToolResult { ok: true, .. } => state.set(FaceState::Thinking),
            ServerEvent::ConfirmAsk { .. } => state.set(FaceState::Alert),
            ServerEvent::TurnDone => state.set(FaceState::Idle),
            ServerEvent::Error { .. } => state.set(FaceState::Alert),
            ServerEvent::Mode { current, .. } if current == "pentest" => {
                state.set(FaceState::Alert);
            }
            ServerEvent::Bye => {
                state.set(FaceState::Offline);
                break;
            }
            _ => {}
        }
    }

    // Clean quit.
    let bye = ClientOp::Quit;
    let mut bytes = serde_json::to_vec(&bye).unwrap_or_default();
    bytes.push(b'\n');
    let _ = w.write_all(&bytes).await;
    Ok(())
}
