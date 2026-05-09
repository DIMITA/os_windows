use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::timeout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmDefault {
    Yes,
    No,
    /// User must explicitly type "yes" or "no"; no default.
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAnswer {
    Yes,
    No,
    Timeout,
}

#[async_trait]
pub trait Confirmer: Send + Sync {
    async fn ask(
        &self,
        prompt: &str,
        default: ConfirmDefault,
        timeout_s: u32,
    ) -> Result<ConfirmAnswer>;
}

/// Reads y/n on the controlling terminal. The current process must own a tty.
pub struct TtyConfirmer;

#[async_trait]
impl Confirmer for TtyConfirmer {
    async fn ask(
        &self,
        prompt: &str,
        default: ConfirmDefault,
        timeout_s: u32,
    ) -> Result<ConfirmAnswer> {
        let hint = match default {
            ConfirmDefault::Yes => "[Y/n]",
            ConfirmDefault::No => "[y/N]",
            ConfirmDefault::Required => "[y/n]",
        };

        let mut stdout = tokio::io::stdout();
        stdout
            .write_all(format!("\n{prompt} {hint} ").as_bytes())
            .await?;
        stdout.flush().await?;

        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();
        let read = timeout(Duration::from_secs(timeout_s as u64), reader.read_line(&mut line)).await;

        let answer = match read {
            Err(_) => ConfirmAnswer::Timeout,
            Ok(Ok(0)) => ConfirmAnswer::No,
            Ok(Ok(_)) => parse_answer(line.trim(), default),
            Ok(Err(e)) => return Err(e.into()),
        };
        Ok(answer)
    }
}

fn parse_answer(s: &str, default: ConfirmDefault) -> ConfirmAnswer {
    let s = s.to_ascii_lowercase();
    match s.as_str() {
        "y" | "yes" => ConfirmAnswer::Yes,
        "n" | "no" => ConfirmAnswer::No,
        "" => match default {
            ConfirmDefault::Yes => ConfirmAnswer::Yes,
            ConfirmDefault::No => ConfirmAnswer::No,
            ConfirmDefault::Required => ConfirmAnswer::No,
        },
        _ => ConfirmAnswer::No,
    }
}

/// A confirmer that always answers a fixed value. For tests and non-interactive use.
pub struct FixedConfirmer(pub ConfirmAnswer);

#[async_trait]
impl Confirmer for FixedConfirmer {
    async fn ask(
        &self,
        _prompt: &str,
        _default: ConfirmDefault,
        _timeout_s: u32,
    ) -> Result<ConfirmAnswer> {
        Ok(self.0)
    }
}
