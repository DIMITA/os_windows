//! Ed25519 signing of audit entries (v1, software key).
//!
//! Each signed entry carries a `sig` field appended at the end of the JSON
//! object as the last key, hex-encoded. The signature covers the entry's
//! bytes WITHOUT the `,"sig":"<hex>"` suffix - that is, the canonical
//! unsigned form ending in the closing `}`. This lets a verifier slice the
//! suffix off, restore the closing `}`, and verify the Ed25519 signature
//! over the resulting bytes without any JSON canonicalization headache.
//!
//! Entries written before signing was enabled remain valid: verify treats a
//! missing `sig` as "skip, but report" so old logs stay readable when a
//! key is added later.

use anyhow::{anyhow, Context, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey, SECRET_KEY_LENGTH};
use std::path::{Path, PathBuf};

pub const PUB_KEY_LEN: usize = 32;

pub struct AuditSigner {
    key: SigningKey,
    public_hex: String,
}

impl AuditSigner {
    pub fn from_secret_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != SECRET_KEY_LENGTH {
            return Err(anyhow!(
                "expected {SECRET_KEY_LENGTH}-byte ed25519 secret, got {}",
                bytes.len()
            ));
        }
        let mut buf = [0u8; SECRET_KEY_LENGTH];
        buf.copy_from_slice(bytes);
        let key = SigningKey::from_bytes(&buf);
        let public_hex = hex::encode(key.verifying_key().to_bytes());
        Ok(Self { key, public_hex })
    }

    pub fn load(priv_path: &Path) -> Result<Self> {
        let bytes = std::fs::read(priv_path)
            .with_context(|| format!("read {}", priv_path.display()))?;
        Self::from_secret_bytes(&bytes)
    }

    pub fn sign_hex(&self, message: &[u8]) -> String {
        let sig: Signature = self.key.sign(message);
        hex::encode(sig.to_bytes())
    }

    pub fn public_key_hex(&self) -> &str {
        &self.public_hex
    }

    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.key.verifying_key().to_bytes());
        let digest = h.finalize();
        hex::encode(&digest[..8])
    }
}

pub struct AuditVerifier {
    key: VerifyingKey,
}

impl AuditVerifier {
    pub fn from_public_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != PUB_KEY_LEN {
            return Err(anyhow!(
                "expected {PUB_KEY_LEN}-byte ed25519 public key, got {}",
                bytes.len()
            ));
        }
        let mut buf = [0u8; PUB_KEY_LEN];
        buf.copy_from_slice(bytes);
        let key = VerifyingKey::from_bytes(&buf)
            .with_context(|| "ed25519 public key parse")?;
        Ok(Self { key })
    }

    pub fn load(pub_path: &Path) -> Result<Self> {
        let bytes = std::fs::read(pub_path)
            .with_context(|| format!("read {}", pub_path.display()))?;
        Self::from_public_bytes(&bytes)
    }

    pub fn verify_hex(&self, message: &[u8], sig_hex: &str) -> Result<()> {
        let raw = hex::decode(sig_hex).with_context(|| "decode sig hex")?;
        if raw.len() != 64 {
            return Err(anyhow!("sig is {} bytes, expected 64", raw.len()));
        }
        let mut buf = [0u8; 64];
        buf.copy_from_slice(&raw);
        let sig = Signature::from_bytes(&buf);
        use ed25519_dalek::Verifier;
        self.key
            .verify(message, &sig)
            .with_context(|| "ed25519 verify")
    }
}

/// Generate a fresh keypair, write the secret to `priv_path` (mode 0600) and
/// the public to `priv_path` with `.pub` appended (mode 0644). Parent dir
/// is created at mode 0700.
pub fn generate_to(priv_path: &Path) -> Result<AuditSigner> {
    use rand_core::OsRng;
    if let Some(parent) = priv_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let key = SigningKey::generate(&mut OsRng);
    std::fs::write(priv_path, key.to_bytes())
        .with_context(|| format!("write {}", priv_path.display()))?;
    let pub_path: PathBuf = {
        let mut p = priv_path.to_path_buf();
        let mut name = priv_path
            .file_name()
            .map(|s| s.to_os_string())
            .unwrap_or_default();
        name.push(".pub");
        p.set_file_name(name);
        p
    };
    std::fs::write(&pub_path, key.verifying_key().to_bytes())
        .with_context(|| format!("write {}", pub_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(priv_path, std::fs::Permissions::from_mode(0o600));
        let _ = std::fs::set_permissions(&pub_path, std::fs::Permissions::from_mode(0o644));
    }

    let public_hex = hex::encode(key.verifying_key().to_bytes());
    Ok(AuditSigner { key, public_hex })
}

/// Default location for the daemon's signing key.
pub fn default_priv_path() -> Result<PathBuf> {
    Ok(wilai_core::paths::data_dir()?
        .join("keys")
        .join("audit.ed25519"))
}

pub fn default_pub_path() -> Result<PathBuf> {
    Ok(wilai_core::paths::data_dir()?
        .join("keys")
        .join("audit.ed25519.pub"))
}

/// Splits a signed entry's line into (unsigned_bytes, sig_hex).
/// Returns Ok(None) if the entry has no signature.
pub fn split_signed_line(line: &[u8]) -> Result<Option<(Vec<u8>, String)>> {
    let needle: &[u8] = b",\"sig\":\"";
    let pos = match memmem(line, needle) {
        Some(p) => p,
        None => return Ok(None),
    };
    let sig_start = pos + needle.len();
    let sig_end = match line[sig_start..].iter().position(|&b| b == b'"') {
        Some(i) => sig_start + i,
        None => return Err(anyhow!("malformed sig field: no closing quote")),
    };
    if line.last() != Some(&b'}') {
        return Err(anyhow!("entry does not end with }}"));
    }
    let sig_hex = std::str::from_utf8(&line[sig_start..sig_end])
        .with_context(|| "sig not utf8")?
        .to_string();
    let mut unsigned = Vec::with_capacity(pos + 1);
    unsigned.extend_from_slice(&line[..pos]);
    unsigned.push(b'}');
    Ok(Some((unsigned, sig_hex)))
}

fn memmem(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_sign_and_verify() {
        let tmp = tempfile::tempdir().unwrap();
        let priv_path = tmp.path().join("audit.ed25519");
        let signer = generate_to(&priv_path).unwrap();
        let msg = br#"{"v":1,"seq":0}"#;
        let sig = signer.sign_hex(msg);

        let pub_path = tmp.path().join("audit.ed25519.pub");
        let verifier = AuditVerifier::load(&pub_path).unwrap();
        verifier.verify_hex(msg, &sig).unwrap();

        // Tamper with the message.
        let bad = br#"{"v":1,"seq":1}"#;
        assert!(verifier.verify_hex(bad, &sig).is_err());
    }

    #[test]
    fn split_finds_sig_at_end() {
        let line = br#"{"v":1,"seq":0,"sig":"abcdef"}"#;
        let (unsigned, sig) = split_signed_line(line).unwrap().unwrap();
        assert_eq!(sig, "abcdef");
        assert_eq!(unsigned, br#"{"v":1,"seq":0}"#);
    }

    #[test]
    fn split_returns_none_when_unsigned() {
        let line = br#"{"v":1,"seq":0}"#;
        assert!(split_signed_line(line).unwrap().is_none());
    }
}
