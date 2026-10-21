//! Voice I/O for wilai-shell. All engines run as subprocesses (no audio
//! crates linked here), so missing binaries degrade gracefully.
//!
//! Pipeline:
//!   - record: arecord → raw WAV (16kHz mono S16_LE)
//!   - stt:    whisper.cpp → plain text
//!   - tts:    piper → raw PCM → aplay
//!
//! Strictly local. The daemon's `security.local_only` lock covers the
//! LLM; this module covers the STT/TTS half of the same promise.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
use wilai_core::VoiceConfig;

/// Where piper and whisper model files live by default. Both the
/// absolute paths in config and bare filenames are accepted; bare
/// filenames resolve under this directory.
pub fn models_dir() -> PathBuf {
    if let Some(d) = dirs::data_dir() {
        return d.join("wilai").join("models");
    }
    PathBuf::from("/var/lib/wilai/models")
}

fn resolve_model(name: &str) -> PathBuf {
    let p = PathBuf::from(name);
    if p.is_absolute() {
        p
    } else {
        models_dir().join(name)
    }
}

#[derive(Debug, Clone)]
pub struct VoiceEngine {
    pub cfg: VoiceConfig,
}

impl VoiceEngine {
    pub fn new(cfg: VoiceConfig) -> Self {
        Self { cfg }
    }

    /// Which engines are present on PATH right now. Reported to the UI
    /// so the user sees why voice is partially usable.
    pub fn probe(&self) -> VoiceProbe {
        VoiceProbe {
            record: on_path(&self.cfg.record_bin),
            play: on_path(&self.cfg.play_bin),
            whisper: on_path(&self.cfg.whisper_bin),
            piper: on_path(&self.cfg.piper_bin),
            whisper_model_exists: resolve_model(&self.cfg.whisper_model).exists(),
            piper_model_exists: {
                let m = if self.cfg.piper_model.is_empty() {
                    String::new()
                } else {
                    self.cfg.piper_model.clone()
                };
                !m.is_empty() && resolve_model(&m).exists()
            },
        }
    }

    pub fn can_listen(&self) -> bool {
        let p = self.probe();
        p.record && p.whisper && p.whisper_model_exists
    }

    pub fn can_speak(&self) -> bool {
        let p = self.probe();
        p.piper && p.play && p.piper_model_exists
    }

    /// Record one segment and transcribe it. The output is trimmed; empty
    /// means whisper produced no words.
    pub async fn record_and_transcribe(&self) -> Result<String> {
        if !self.can_listen() {
            bail!("STT unavailable (missing record binary, whisper, or model)");
        }
        let wav = std::env::temp_dir()
            .join(format!("wilai-shell-{}.wav", std::process::id()));
        let secs = self.cfg.record_secs;
        let rate = self.cfg.sample_rate.to_string();
        let record_secs = secs.to_string();

        let rec = Command::new(&self.cfg.record_bin)
            .args([
                "-q", "-d", &record_secs,
                "-f", "S16_LE", "-r", &rate,
                "-c", "1",
            ])
            .arg(&wav)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let rec_fut = timeout(Duration::from_secs(secs as u64 + 5), rec);
        let status = rec_fut
            .await
            .map_err(|_| anyhow!("record timeout"))?
            .with_context(|| format!("spawn {}", self.cfg.record_bin))?;
        if !status.success() {
            let _ = std::fs::remove_file(&wav);
            bail!("{} failed: {status}", self.cfg.record_bin);
        }

        let model = resolve_model(&self.cfg.whisper_model);
        let out = Command::new(&self.cfg.whisper_bin)
            .arg("-m")
            .arg(&model)
            .arg("-f")
            .arg(&wav)
            .args(["-otxt", "-nt", "-l", "auto"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .with_context(|| format!("spawn {}", self.cfg.whisper_bin))?;
        let _ = std::fs::remove_file(&wav);
        if !out.status.success() {
            bail!(
                "whisper failed: {} stderr={}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(text)
    }

    /// Speak one phrase. Piper streams raw PCM on stdout; aplay consumes
    /// it directly. Returns when playback finishes.
    pub async fn speak(&self, text: &str) -> Result<()> {
        if text.trim().is_empty() {
            return Ok(());
        }
        if !self.can_speak() {
            bail!("TTS unavailable (missing piper, aplay, or voice model)");
        }
        let model_name = if self.cfg.piper_model.is_empty() {
            // Fall back to the persona voice if VoiceConfig.piper_model is unset.
            // The shell owner usually sets this via CLI --piper-model.
            bail!("piper_model is unset");
        } else {
            self.cfg.piper_model.clone()
        };
        let model = resolve_model(&model_name);

        // Chain: piper --output_raw | aplay -r 22050
        let mut piper = Command::new(&self.cfg.piper_bin)
            .arg("--model")
            .arg(&model)
            .arg("--output_raw")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn {}", self.cfg.piper_bin))?;

        if let Some(mut stdin) = piper.stdin.take() {
            stdin.write_all(text.as_bytes()).await.ok();
            stdin.flush().await.ok();
            drop(stdin);
        }
        let piper_out = piper
            .stdout
            .take()
            .ok_or_else(|| anyhow!("piper has no stdout"))?;
        let piper_out: std::process::Stdio = piper_out
            .try_into()
            .map_err(|_| anyhow!("piper stdout not pipe-convertible"))?;

        let mut play = Command::new(&self.cfg.play_bin)
            .args(["-q", "-f", "S16_LE", "-r", "22050", "-c", "1"])
            .stdin(piper_out)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn {}", self.cfg.play_bin))?;

        // Both must finish; piper exits once input EOF + rendering done,
        // aplay exits once the pipe closes.
        let _ = piper.wait().await;
        let _ = play.wait().await;
        Ok(())
    }

    /// Split a reply into speech-sized phrases. Keeps TTS latency low by
    /// speaking as sentences arrive.
    pub fn split_phrases(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        for c in text.chars() {
            cur.push(c);
            if matches!(c, '.' | '!' | '?' | '\n') && cur.trim().len() >= 2 {
                out.push(cur.trim().to_string());
                cur.clear();
            }
        }
        let last = cur.trim();
        if !last.is_empty() {
            out.push(last.to_string());
        }
        out
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VoiceProbe {
    pub record: bool,
    pub play: bool,
    pub whisper: bool,
    pub piper: bool,
    pub whisper_model_exists: bool,
    pub piper_model_exists: bool,
}

impl VoiceProbe {
    pub fn summary(&self) -> String {
        let listen = if self.record && self.whisper && self.whisper_model_exists {
            "listen=yes"
        } else {
            "listen=no"
        };
        let speak = if self.play && self.piper && self.piper_model_exists {
            "speak=yes"
        } else {
            "speak=no"
        };
        format!("{listen} {speak}")
    }
}

fn on_path(name: &str) -> bool {
    let p = Path::new(name);
    if p.is_absolute() || name.contains('/') {
        return p.exists();
    }
    let Ok(path) = std::env::var("PATH") else { return false };
    for dir in std::env::split_paths(&path) {
        if dir.join(name).exists() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_phrases_breaks_on_sentence_end() {
        let v = VoiceEngine::split_phrases("First. Second! Third?\nFourth");
        assert_eq!(v, vec!["First.", "Second!", "Third?", "Fourth"]);
    }

    #[test]
    fn split_phrases_handles_single_phrase() {
        let v = VoiceEngine::split_phrases("Hello Monsieur");
        assert_eq!(v, vec!["Hello Monsieur"]);
    }

    #[test]
    fn split_phrases_ignores_lone_punct() {
        // A single-char fragment is not emitted on its own.
        let v = VoiceEngine::split_phrases(".");
        assert_eq!(v, vec!["."]);
    }

    #[test]
    fn probe_reports_missing_bins() {
        let mut cfg = VoiceConfig::default();
        cfg.whisper_bin = "/nope/whisper".to_string();
        cfg.record_bin = "/nope/arecord".to_string();
        cfg.piper_bin = "/nope/piper".to_string();
        cfg.play_bin = "/nope/aplay".to_string();
        let p = VoiceEngine::new(cfg).probe();
        assert!(!p.record && !p.play && !p.whisper && !p.piper);
    }

    #[test]
    fn on_path_rejects_impossible_binary() {
        assert!(!on_path("this_binary_definitely_does_not_exist_xyz_1234"));
    }
}
