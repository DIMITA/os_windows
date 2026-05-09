use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const GENESIS_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainHead {
    pub running_hash: String,
    pub last_seq: u64,
    pub current_file: String,
}

impl ChainHead {
    pub fn genesis(file_name: &str) -> Self {
        Self {
            running_hash: GENESIS_HASH.to_string(),
            last_seq: 0,
            current_file: file_name.to_string(),
        }
    }

    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let head: ChainHead = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse {}", path.display()))?;
        Ok(Some(head))
    }

    pub fn save_atomic(&self, path: &Path) -> Result<()> {
        let tmp: PathBuf = {
            let mut p = path.to_path_buf();
            p.set_extension("tmp");
            p
        };
        let bytes = serde_json::to_vec(self)?;
        std::fs::write(&tmp, &bytes)
            .with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }
}

pub fn recover_from_tail(file: &Path) -> Result<(String, u64)> {
    let bytes = std::fs::read(file).with_context(|| format!("read {}", file.display()))?;
    if bytes.is_empty() {
        return Ok((GENESIS_HASH.to_string(), 0));
    }

    let trimmed_end = if bytes.last() == Some(&b'\n') {
        bytes.len() - 1
    } else {
        bytes.len()
    };

    let mut last_nl = None;
    for i in (0..trimmed_end).rev() {
        if bytes[i] == b'\n' {
            last_nl = Some(i);
            break;
        }
    }
    let line_start = last_nl.map(|i| i + 1).unwrap_or(0);
    let line = &bytes[line_start..trimmed_end];

    if line.is_empty() {
        return Ok((GENESIS_HASH.to_string(), 0));
    }

    #[derive(Deserialize)]
    struct Tail { seq: u64 }
    let tail: Tail = serde_json::from_slice(line)
        .context("parse tail line of audit file")?;
    Ok((sha256_hex(line), tail.seq))
}
