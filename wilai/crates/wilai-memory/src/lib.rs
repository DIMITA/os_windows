//! Wilai long-term memory. SQLite file under $XDG_DATA_HOME/wilai/memory/
//! plus a local Ollama-hosted embedding model for semantic recall.
//!
//! Three stores:
//! - `notes`:    free text with embeddings, searchable by meaning.
//! - `facts`:    typed KV ("user.name" = "Diana"), overwrites, no vectors.
//! - `sessions`: index of past chat sessions with optional summaries,
//!               so the agent can resume by name or date.
//!
//! Nothing in this crate ever leaves the host. Embeddings go to a
//! configured Ollama endpoint (default 127.0.0.1:11434) with a
//! documented local model (`nomic-embed-text` by default).

pub mod embedder;
pub mod store;

pub use embedder::{Embedder, OllamaEmbedder};
pub use store::{CompactPolicy, CompactReport, Fact, MemoryStore, Note, NoteHit, SessionIndex};
