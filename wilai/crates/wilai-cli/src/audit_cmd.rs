use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

fn audit_dir() -> Result<PathBuf> {
    wilai_core::paths::audit_dir()
}

fn list_files(dir: &std::path::Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .collect();
    files.sort();
    Ok(files)
}

pub fn tail(n: usize) -> Result<()> {
    let dir = audit_dir()?;
    if !dir.exists() {
        eprintln!("no audit dir at {}", dir.display());
        return Ok(());
    }
    let files = list_files(&dir)?;
    let mut ring: VecDeque<String> = VecDeque::with_capacity(n);
    for f in files {
        let text = fs::read_to_string(&f)
            .with_context(|| format!("read {}", f.display()))?;
        for line in text.lines() {
            if ring.len() == n {
                ring.pop_front();
            }
            ring.push_back(line.to_string());
        }
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in &ring {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

pub fn show(session: &str) -> Result<()> {
    let dir = audit_dir()?;
    if !dir.exists() {
        return Ok(());
    }
    let files = list_files(&dir)?;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for f in files {
        let text = fs::read_to_string(&f)?;
        for line in text.lines() {
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("session").and_then(|s| s.as_str()) == Some(session) {
                writeln!(out, "{line}")?;
            }
        }
    }
    Ok(())
}

pub fn verify() -> Result<()> {
    use sha2::{Digest, Sha256};
    let dir = audit_dir()?;
    if !dir.exists() {
        eprintln!("no audit dir");
        return Ok(());
    }
    let files = list_files(&dir)?;
    let mut prev_hash = String::from(
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    let mut total = 0u64;
    let mut errors = 0u64;

    for f in &files {
        let bytes = fs::read(f)?;
        let mut last_seq: Option<u64> = None;
        let mut start = 0usize;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                let line = &bytes[start..i];
                start = i + 1;
                if line.is_empty() {
                    continue;
                }
                total += 1;
                let v: Value = match serde_json::from_slice(line) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("{}: line parse error: {e}", f.display());
                        errors += 1;
                        continue;
                    }
                };
                let entry_prev = v.get("prev_hash").and_then(|s| s.as_str()).unwrap_or("");
                if entry_prev != prev_hash {
                    eprintln!(
                        "{}: chain break at seq={:?}: expected prev_hash {}, got {}",
                        f.display(),
                        v.get("seq"),
                        prev_hash,
                        entry_prev,
                    );
                    errors += 1;
                }
                if let Some(seq) = v.get("seq").and_then(|s| s.as_u64()) {
                    if let Some(prev) = last_seq {
                        if seq != prev + 1 {
                            eprintln!(
                                "{}: seq jump from {} to {}",
                                f.display(),
                                prev, seq
                            );
                            errors += 1;
                        }
                    }
                    last_seq = Some(seq);
                }
                let mut h = Sha256::new();
                h.update(line);
                prev_hash = hex::encode(h.finalize());
            }
        }
    }
    println!("verified {} entries across {} files; {} errors", total, files.len(), errors);
    if errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}
