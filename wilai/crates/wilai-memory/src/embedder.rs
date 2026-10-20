//! Local embedding client. Calls Ollama's /api/embeddings endpoint
//! with a configured model and returns an f32 vector. No cloud paths.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[async_trait]
pub trait Embedder: Send + Sync {
    /// Compute an embedding for `text`. Returns a vector of f32 whose
    /// length is the model's embedding dimension (768 for
    /// nomic-embed-text, 1024 for mxbai-embed-large, etc.).
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// A human-readable name for logging / audit.
    fn name(&self) -> &str;
}

/// Deterministic embedder for tests: hashes the input into a small
/// fixed-size vector so recall is reproducible without a running Ollama.
pub struct HashEmbedder {
    pub dim: usize,
    name: String,
}

impl HashEmbedder {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            name: format!("hash-{dim}"),
        }
    }
}

#[async_trait]
impl Embedder for HashEmbedder {
    fn name(&self) -> &str { &self.name }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        // Not for security, just reproducible pseudo-vectors keyed on
        // the content. Each unique token contributes to a bucket
        // position-independently (so "cat" matches "cat" regardless of
        // where it sits in the sentence); final vector is L2-normalized
        // so cosine similarity is meaningful.
        let mut v = vec![0f32; self.dim];
        for token in text.split_whitespace() {
            let lower = token.to_ascii_lowercase();
            let mut h: u64 = 1469598103934665603;
            for b in lower.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(1099511628211);
            }
            let idx = (h as usize) % self.dim;
            v[idx] += 1.0;
            v[(idx + 7) % self.dim] += 0.5;
        }
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        for x in &mut v {
            *x /= norm;
        }
        Ok(v)
    }
}

pub struct OllamaEmbedder {
    url: String,
    model: String,
    client: reqwest::Client,
    name: String,
}

impl OllamaEmbedder {
    pub fn new(url: impl Into<String>, model: impl Into<String>) -> Result<Self> {
        let model = model.into();
        let name = format!("ollama/{model}");
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            url: url.into(),
            model,
            client,
            name,
        })
    }
}

#[derive(Serialize)]
struct EmbedReq<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Deserialize)]
struct EmbedResp {
    #[serde(default)]
    embedding: Vec<f32>,
    #[serde(default)]
    error: Option<String>,
}

#[async_trait]
impl Embedder for OllamaEmbedder {
    fn name(&self) -> &str { &self.name }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!("{}/api/embeddings", self.url.trim_end_matches('/'));
        let resp = self
            .client
            .post(&url)
            .json(&EmbedReq { model: &self.model, prompt: text })
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("ollama embeddings HTTP {status}: {text}");
        }
        let parsed: EmbedResp = serde_json::from_str(&text)
            .with_context(|| format!("parse ollama embed response: {text}"))?;
        if let Some(e) = parsed.error {
            anyhow::bail!("ollama embeddings error: {e}");
        }
        if parsed.embedding.is_empty() {
            anyhow::bail!("ollama embeddings returned empty vector");
        }
        Ok(parsed.embedding)
    }
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0f32;
    let mut na = 0f32;
    let mut nb = 0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom < 1e-9 { 0.0 } else { dot / denom }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_embedder_is_deterministic_and_sensitive() {
        let e = HashEmbedder::new(64);
        let a = e.embed("the cat sat on the mat").await.unwrap();
        let a2 = e.embed("the cat sat on the mat").await.unwrap();
        let b = e.embed("quantum chromodynamics").await.unwrap();
        assert_eq!(a, a2);
        let same = cosine(&a, &a2);
        let diff = cosine(&a, &b);
        assert!(same > 0.99, "same text should give ~1.0 cosine, got {same}");
        assert!(diff < same, "different text should cosine-differ: {diff} vs {same}");
    }

    #[test]
    fn cosine_edge_cases() {
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0); // length mismatch
        let v = vec![1.0, 0.0, 0.0];
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
    }
}
