//! wilai-voice: bridge between local STT/TTS engines and a running wilai-daemon.
//!
//! The binary spawns subprocesses to record audio, transcribe it, send the text
//! to the daemon, and synthesize the daemon's text replies back through the
//! speakers. Engines are not embedded - operators bring their own
//! (whisper.cpp + piper + arecord/aplay are the defaults). Missing engines
//! degrade gracefully: a warning is logged and the relevant pipeline step is
//! skipped.

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::Command;
use tracing_subscriber::EnvFilter;
use wilai_daemon::protocol::{ClientOp, ServerEvent};

#[derive(Parser, Debug)]
#[command(name = "wilai-voice", version, about = "Wilai voice client")]
struct Cli {
    /// Daemon socket. Defaults to $XDG_RUNTIME_DIR/wilai.sock.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Recording duration in seconds for one push-to-talk segment.
    #[arg(long, default_value_t = 5)]
    record_secs: u32,
    /// Path to the whisper.cpp binary (e.g. "main" or "whisper-cli").
    #[arg(long, default_value = "whisper-cli")]
    whisper_bin: String,
    /// Whisper model file (.gguf or .bin). Required if STT is desired.
    #[arg(long)]
    whisper_model: Option<PathBuf>,
    /// Path to the piper binary.
    #[arg(long, default_value = "piper")]
    piper_bin: String,
    /// Piper voice model (.onnx).
    #[arg(long)]
    piper_model: Option<PathBuf>,
    /// External wake-word command. Each line on its stdout triggers a recording.
    /// If unset, the binary runs in push-to-talk mode (press Enter to record).
    #[arg(long)]
    wake_cmd: Option<String>,
    /// Audio recorder binary. Receives args: "-d <secs> -f S16_LE -r 16000 -c 1 <wav>".
    #[arg(long, default_value = "arecord")]
    record_bin: String,
    /// Audio playback binary. Receives a WAV path on stdin.
    #[arg(long, default_value = "aplay")]
    play_bin: String,
    /// Mute - skip TTS even if piper is available.
    #[arg(long)]
    mute: bool,
    /// Logging filter.
    #[arg(long, default_value = "info")]
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

    if cli.whisper_model.is_none() {
        tracing::warn!("--whisper-model unset; STT will fail until provided");
    }
    if cli.piper_model.is_none() && !cli.mute {
        tracing::warn!("--piper-model unset; TTS will be skipped");
    }

    if let Some(cmd) = cli.wake_cmd.clone() {
        run_wake_loop(&cli, &socket, &cmd).await
    } else {
        run_ptt_loop(&cli, &socket).await
    }
}

async fn run_ptt_loop(cli: &Cli, socket: &Path) -> Result<()> {
    println!("wilai-voice push-to-talk. Press Enter to record {}s, type :quit to exit.", cli.record_secs);
    let stdin = std::io::stdin();
    loop {
        let mut line = String::new();
        let n = stdin.lock().read_line(&mut line)?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed == ":quit" || trimmed == ":exit" {
            break;
        }
        if let Err(e) = one_round(cli, socket).await {
            eprintln!("error: {e:#}");
        }
    }
    Ok(())
}

async fn run_wake_loop(cli: &Cli, socket: &Path, wake_cmd: &str) -> Result<()> {
    tracing::info!("starting wake-word command: {wake_cmd}");
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(wake_cmd)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn wake cmd: {wake_cmd}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("wake-cmd has no stdout"))?;
    let mut reader = BufReader::new(stdout).lines();
    while let Some(line) = reader.next_line().await? {
        tracing::info!(trigger = %line, "wake-word triggered");
        if let Err(e) = one_round(cli, socket).await {
            eprintln!("voice round failed: {e:#}");
        }
    }
    let _ = child.wait().await;
    Ok(())
}

async fn one_round(cli: &Cli, socket: &Path) -> Result<()> {
    let wav = std::env::temp_dir().join(format!("wilai-voice-{}.wav", std::process::id()));
    record_audio(cli, &wav).await?;

    let text = match cli.whisper_model.as_deref() {
        Some(m) => transcribe(cli, &wav, m).await?,
        None => {
            tracing::warn!("no whisper model; skipping STT");
            String::new()
        }
    };
    let _ = std::fs::remove_file(&wav);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        eprintln!("(empty transcription)");
        return Ok(());
    }
    eprintln!("you: {trimmed}");

    let replies = run_daemon_turn(socket, trimmed).await?;
    for reply in &replies {
        println!("wilai: {reply}");
        if !cli.mute {
            if let Some(model) = &cli.piper_model {
                if let Err(e) = speak(cli, reply, model).await {
                    tracing::warn!("tts failed: {e:#}");
                }
            }
        }
    }
    Ok(())
}

async fn record_audio(cli: &Cli, out: &Path) -> Result<()> {
    let status = Command::new(&cli.record_bin)
        .args([
            "-q",
            "-d",
            &cli.record_secs.to_string(),
            "-f",
            "S16_LE",
            "-r",
            "16000",
            "-c",
            "1",
        ])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .await
        .with_context(|| format!("spawn {}", cli.record_bin))?;
    if !status.success() {
        anyhow::bail!("{} failed: {status}", cli.record_bin);
    }
    Ok(())
}

async fn transcribe(cli: &Cli, wav: &Path, model: &Path) -> Result<String> {
    let out = Command::new(&cli.whisper_bin)
        .args(["-m"])
        .arg(model)
        .args(["-f"])
        .arg(wav)
        .args(["-otxt", "-nt"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("spawn {}", cli.whisper_bin))?;
    if !out.status.success() {
        anyhow::bail!(
            "whisper failed: {} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

async fn speak(cli: &Cli, text: &str, model: &Path) -> Result<()> {
    use std::process::Stdio as Stdio2;
    let mut piper = std::process::Command::new(&cli.piper_bin)
        .args(["--model"])
        .arg(model)
        .args(["--output_raw"])
        .stdin(Stdio2::piped())
        .stdout(Stdio2::piped())
        .stderr(Stdio2::null())
        .spawn()
        .with_context(|| format!("spawn {}", cli.piper_bin))?;
    use std::io::Write;
    if let Some(mut stdin) = piper.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    let piper_out = piper
        .stdout
        .take()
        .ok_or_else(|| anyhow!("piper has no stdout"))?;
    let aplay = std::process::Command::new(&cli.play_bin)
        .args(["-q", "-f", "S16_LE", "-r", "22050", "-c", "1"])
        .stdin(piper_out)
        .stdout(Stdio2::null())
        .stderr(Stdio2::null())
        .spawn()
        .with_context(|| format!("spawn {}", cli.play_bin))?;
    let _ = piper.wait();
    let _ = aplay.wait_with_output();
    Ok(())
}

async fn run_daemon_turn(socket: &Path, prompt: &str) -> Result<Vec<String>> {
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

    let op = ClientOp::Prompt {
        text: prompt.to_string(),
        model: None,
        provider: None,
    };
    let mut bytes = serde_json::to_vec(&op)?;
    bytes.push(b'\n');
    write_half.write_all(&bytes).await?;
    write_half.flush().await?;

    let mut texts = Vec::new();
    while let Some(line) = reader.next_line().await? {
        let ev: ServerEvent = serde_json::from_str(&line)?;
        match ev {
            ServerEvent::Text { content } => texts.push(content),
            ServerEvent::ToolCall { name, args: _ } => {
                eprintln!("[voice] tool call: {name}");
            }
            ServerEvent::ToolResult { name, ok, .. } => {
                let m = if ok { "ok" } else { "err" };
                eprintln!("[voice] tool {m}: {name}");
            }
            ServerEvent::ConfirmAsk { id, prompt: _, .. } => {
                // Voice client cannot mediate confirms safely; refuse by default.
                eprintln!("[voice] confirm refused (voice client does not gate)");
                let reply = ClientOp::ConfirmAnswer {
                    id,
                    answer: wilai_daemon::protocol::ConfirmReply::No,
                };
                let mut b = serde_json::to_vec(&reply)?;
                b.push(b'\n');
                write_half.write_all(&b).await?;
                write_half.flush().await?;
            }
            ServerEvent::TurnDone => break,
            ServerEvent::Error { message } => {
                eprintln!("[voice] daemon error: {message}");
                break;
            }
            ServerEvent::Bye => break,
            ServerEvent::SessionStart { .. } => {}
            ServerEvent::Mode { current, .. } => {
                eprintln!("[voice] mode is now {current}");
            }
            ServerEvent::Tools { .. } => {}
        }
    }

    let quit = ClientOp::Quit;
    let mut b = serde_json::to_vec(&quit)?;
    b.push(b'\n');
    let _ = write_half.write_all(&b).await;
    Ok(texts)
}
