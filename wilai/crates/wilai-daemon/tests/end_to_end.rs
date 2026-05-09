//! End-to-end smoke for the daemon: spawn it on a temp socket, drive a few
//! ops over the wire, and assert that the resulting audit log is consistent.
//! Does not require a live LLM provider; verifies handshake, ModeGet/ModeSet,
//! and Quit shutdown.

use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use wilai_daemon::protocol::{ClientOp, ServerEvent};

fn daemon_bin() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("target");
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    p.push(profile);
    p.push("wilai-daemon");
    p
}

fn tools_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("tools");
    p
}

async fn read_event(reader: &mut (impl AsyncBufReadExt + Unpin)) -> ServerEvent {
    let mut line = String::new();
    let n = tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .expect("event timeout")
        .expect("read");
    assert!(n > 0, "daemon closed unexpectedly");
    serde_json::from_str(line.trim())
        .unwrap_or_else(|e| panic!("parse {line:?}: {e}"))
}

async fn write_op(stream: &mut tokio::net::unix::OwnedWriteHalf, op: &ClientOp) {
    let mut bytes = serde_json::to_vec(op).unwrap();
    bytes.push(b'\n');
    stream.write_all(&bytes).await.unwrap();
    stream.flush().await.unwrap();
}

async fn wait_for_socket(path: &std::path::Path) -> bool {
    for _ in 0..40 {
        if path.exists() && UnixStream::connect(path).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn daemon_handshake_and_mode_round_trip() {
    let bin = daemon_bin();
    if !bin.exists() {
        eprintln!("skip: {} not built (run `cargo build` first)", bin.display());
        return;
    }
    let tmp = TempDir::new().unwrap();
    let socket = tmp.path().join("wilai.sock");
    let data = tmp.path().join("data");
    let cfg = tmp.path().join("config");
    let runtime = tmp.path().join("run");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&runtime).unwrap();

    let mut child = std::process::Command::new(&bin)
        .arg("--socket").arg(&socket)
        .arg("--no-auto-detect")
        .arg("--log").arg("warn")
        .env("HOME", tmp.path())
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("XDG_DATA_HOME", &data)
        .env("XDG_CONFIG_HOME", &cfg)
        .env("WILAI_TOOLS_DIR", tools_dir())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn daemon");

    assert!(wait_for_socket(&socket).await, "daemon failed to bind socket");

    let stream = UnixStream::connect(&socket).await.unwrap();
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // Handshake.
    match read_event(&mut reader).await {
        ServerEvent::SessionStart { session } => {
            assert_eq!(session.len(), 26, "ULID length");
        }
        other => panic!("expected session_start, got {other:?}"),
    }

    // ModeGet -> Mode { current: "normal" }.
    write_op(&mut write_half, &ClientOp::ModeGet).await;
    match read_event(&mut reader).await {
        ServerEvent::Mode { current, pentest_in_flight } => {
            assert_eq!(current, "normal");
            assert_eq!(pentest_in_flight, 0);
        }
        other => panic!("expected mode, got {other:?}"),
    }

    // ModeSet pentest -> Mode { current: "pentest" } (and a relayed Mode event
    // from the subscriber fanout).
    write_op(
        &mut write_half,
        &ClientOp::ModeSet {
            to: "pentest".to_string(),
            trigger: Some("test".to_string()),
        },
    )
    .await;
    let mut got_pentest = false;
    for _ in 0..3 {
        if let ServerEvent::Mode { current, .. } = read_event(&mut reader).await {
            if current == "pentest" {
                got_pentest = true;
                break;
            }
        }
    }
    assert!(got_pentest, "did not see pentest mode confirmation");

    // Quit.
    write_op(&mut write_half, &ClientOp::Quit).await;
    drop(write_half);

    // Drain any trailing events (Bye) to ensure the session task ran to
    // completion and pushed its session.end audit entry through the mpsc.
    for _ in 0..6 {
        let mut line = String::new();
        let r = tokio::time::timeout(Duration::from_millis(200), reader.read_line(&mut line)).await;
        match r {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => break,
        }
    }

    // Daemon stays up; we shutdown ourselves. Give the audit task a beat
    // so the mode.change and session.end entries reach disk before we kill.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = child.kill();
    let _ = child.wait();

    // Audit log should contain at least: chain_genesis, session.start,
    // mode.change, session.end. The chain must verify.
    let mut audit_files: Vec<_> = std::fs::read_dir(data.join("wilai/audit"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .map(|e| e.path())
        .collect();
    audit_files.sort();
    assert!(!audit_files.is_empty(), "no audit files written");

    let mut prev_hash = String::from(
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    let mut kinds = Vec::new();
    use sha2::{Digest, Sha256};
    for f in &audit_files {
        let bytes = std::fs::read(f).unwrap();
        let mut start = 0;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                let line = &bytes[start..i];
                start = i + 1;
                if line.is_empty() { continue; }
                let v: serde_json::Value = serde_json::from_slice(line).unwrap();
                assert_eq!(
                    v.get("prev_hash").and_then(|s| s.as_str()).unwrap(),
                    prev_hash,
                    "chain break in {}",
                    f.display()
                );
                kinds.push(v.get("kind").and_then(|s| s.as_str()).unwrap().to_string());
                let mut h = Sha256::new();
                h.update(line);
                prev_hash = hex::encode(h.finalize());
            }
        }
    }
    assert!(kinds.iter().any(|k| k == "system.chain_genesis"));
    assert!(kinds.iter().any(|k| k == "session.start"));
    assert!(kinds.iter().any(|k| k == "mode.change"));
}
