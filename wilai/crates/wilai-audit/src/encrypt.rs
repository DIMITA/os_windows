//! Symmetric encryption for rotated audit files.
//!
//! The hot audit file stays plaintext — the hash chain writer appends
//! JSONL and syncs on every entry, so inline encryption would wreck
//! integrity semantics (and make tamper-detection much harder). On
//! day rollover, after the previous file has been fsync'd and
//! `chattr +a`'d, we encrypt it with ChaCha20-Poly1305 to
//! `<name>.enc`, verify, then delete the plaintext.
//!
//! File format:
//!   magic:   b"WILAIAEN"           (8 bytes)
//!   version: 1                     (1 byte)
//!   nonce:   12 bytes (random)
//!   len:     u64 LE                (plaintext size)
//!   payload: ChaCha20-Poly1305(plaintext || "")
//!   tag:     16 bytes (included in payload per RustCrypto convention)
//!
//! The key is 32 bytes. The caller is responsible for keeping it on a
//! secure-enough filesystem path (we recommend
//! /var/lib/wilai/audit-key with mode 0400 wilai:wilai).

use anyhow::{anyhow, bail, Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

pub const MAGIC: &[u8] = b"WILAIAEN";
pub const VERSION: u8 = 1;
pub const KEY_SIZE: usize = 32;
pub const NONCE_SIZE: usize = 12;

pub fn encrypt_file(src: &Path, dst: &Path, key: &[u8; KEY_SIZE]) -> Result<()> {
    let mut plaintext = Vec::new();
    OpenOptions::new()
        .read(true)
        .open(src)
        .with_context(|| format!("open {}", src.display()))?
        .read_to_end(&mut plaintext)
        .with_context(|| format!("read {}", src.display()))?;

    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let mut nonce_bytes = [0u8; NONCE_SIZE];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_ref())
        .map_err(|e| anyhow!("chacha20poly1305 encrypt: {e}"))?;

    let mut out = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dst)
        .with_context(|| format!("open {}", dst.display()))?;
    out.write_all(MAGIC)?;
    out.write_all(&[VERSION])?;
    out.write_all(&nonce_bytes)?;
    out.write_all(&(plaintext.len() as u64).to_le_bytes())?;
    out.write_all(&ciphertext)?;
    out.sync_all()?;
    Ok(())
}

pub fn decrypt_file(src: &Path, dst: &Path, key: &[u8; KEY_SIZE]) -> Result<()> {
    let mut raw = Vec::new();
    OpenOptions::new()
        .read(true)
        .open(src)
        .with_context(|| format!("open {}", src.display()))?
        .read_to_end(&mut raw)?;
    if raw.len() < MAGIC.len() + 1 + NONCE_SIZE + 8 {
        bail!("file too short to be a wilai audit archive");
    }
    let (header, rest) = raw.split_at(MAGIC.len() + 1 + NONCE_SIZE + 8);
    if &header[..MAGIC.len()] != MAGIC {
        bail!("bad magic");
    }
    if header[MAGIC.len()] != VERSION {
        bail!("unsupported archive version {}", header[MAGIC.len()]);
    }
    let nonce_bytes = &header[MAGIC.len() + 1..MAGIC.len() + 1 + NONCE_SIZE];
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let plaintext = cipher
        .decrypt(nonce, rest)
        .map_err(|e| anyhow!("chacha20poly1305 decrypt: {e}"))?;
    fs::write(dst, &plaintext)
        .with_context(|| format!("write {}", dst.display()))?;
    Ok(())
}

/// Load (or generate) the 32-byte audit encryption key from disk. The
/// file is created with 0600 permissions. Hex-encoded for easy copy.
pub fn load_or_create_key(path: &Path) -> Result<[u8; KEY_SIZE]> {
    if path.exists() {
        let text = fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        let bytes = hex::decode(text.trim()).context("hex decode audit key")?;
        if bytes.len() != KEY_SIZE {
            bail!("audit key must be {} bytes, got {}", KEY_SIZE, bytes.len());
        }
        let mut out = [0u8; KEY_SIZE];
        out.copy_from_slice(&bytes);
        return Ok(out);
    }
    let mut key = [0u8; KEY_SIZE];
    rand::thread_rng().fill_bytes(&mut key);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let mut f = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    f.write_all(hex::encode(key).as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn round_trip_small_file() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("plain.jsonl");
        let enc = tmp.path().join("plain.jsonl.enc");
        let out = tmp.path().join("plain.jsonl.dec");
        fs::write(&src, b"{\"seq\":0}\n{\"seq\":1,\"kind\":\"tool.exec\"}\n").unwrap();

        let key = [7u8; KEY_SIZE];
        encrypt_file(&src, &enc, &key).unwrap();
        decrypt_file(&enc, &out, &key).unwrap();

        let a = fs::read(&src).unwrap();
        let b = fs::read(&out).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn tamper_fails_auth() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("plain.jsonl");
        let enc = tmp.path().join("plain.jsonl.enc");
        let out = tmp.path().join("plain.jsonl.dec");
        fs::write(&src, b"important").unwrap();
        let key = [7u8; KEY_SIZE];
        encrypt_file(&src, &enc, &key).unwrap();
        // Flip one byte in the ciphertext region.
        let mut raw = fs::read(&enc).unwrap();
        let i = raw.len() - 5;
        raw[i] ^= 0x01;
        fs::write(&enc, &raw).unwrap();
        let err = decrypt_file(&enc, &out, &key).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("decrypt"));
    }

    #[test]
    fn key_round_trip() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("audit-key");
        let a = load_or_create_key(&p).unwrap();
        let b = load_or_create_key(&p).unwrap();
        assert_eq!(a, b);
    }
}
