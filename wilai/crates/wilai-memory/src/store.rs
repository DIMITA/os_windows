//! SQLite-backed memory store. One file per Wilai install under
//! $XDG_DATA_HOME/wilai/memory/wilai.sqlite. Schema is created on first
//! open; migrations are forward-only via `PRAGMA user_version`.

use crate::embedder::{cosine, Embedder};
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

const SCHEMA_VERSION: u32 = 1;

/// A free-text note with its embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub session: Option<String>,
    pub content: String,
    pub created_at: String,
    pub tags: Vec<String>,
}

/// A note returned from a search, with its similarity score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteHit {
    pub note: Note,
    pub score: f32,
}

/// A typed key-value fact. Overwrite semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub key: String,
    pub value: String,
    pub updated_at: String,
}

/// A session row for the agent to reason about "what did we do".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIndex {
    pub id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub summary: Option<String>,
}

pub struct MemoryStore {
    conn: Mutex<Connection>,
}

impl MemoryStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open sqlite {}", path.display()))?;
        let s = Self { conn: Mutex::new(conn) };
        s.migrate()?;
        Ok(s)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let s = Self { conn: Mutex::new(conn) };
        s.migrate()?;
        Ok(s)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap_or(0);
        if v < 1 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS notes (
                    id TEXT PRIMARY KEY,
                    session TEXT,
                    content TEXT NOT NULL,
                    tags TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL,
                    embedding BLOB,
                    embedder TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_notes_session ON notes(session);
                CREATE INDEX IF NOT EXISTS idx_notes_created ON notes(created_at);

                CREATE TABLE IF NOT EXISTS facts (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS sessions (
                    id TEXT PRIMARY KEY,
                    started_at TEXT NOT NULL,
                    ended_at TEXT,
                    summary TEXT
                );

                PRAGMA user_version = 1;",
            )?;
        }
        let _ = SCHEMA_VERSION;
        Ok(())
    }

    pub async fn note(
        &self,
        session: Option<&str>,
        content: &str,
        tags: &[String],
        embedder: &dyn Embedder,
    ) -> Result<String> {
        let embedding = embedder.embed(content).await?;
        let bytes = f32_to_le_bytes(&embedding);
        let id = wilai_core::types::new_ulid();
        let now = iso_now()?;
        let tags_str = tags.join(",");
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO notes (id, session, content, tags, created_at, embedding, embedder)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, session, content, tags_str, now, bytes, embedder.name()],
        )?;
        Ok(id)
    }

    pub async fn recall(
        &self,
        query: &str,
        k: usize,
        embedder: &dyn Embedder,
    ) -> Result<Vec<NoteHit>> {
        if k == 0 {
            return Ok(vec![]);
        }
        let q = embedder.embed(query).await?;
        let rows = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id, session, content, tags, created_at, embedding
                 FROM notes WHERE embedding IS NOT NULL",
            )?;
            let iter = stmt.query_map([], |row| {
                let id: String = row.get(0)?;
                let session: Option<String> = row.get(1)?;
                let content: String = row.get(2)?;
                let tags: String = row.get(3)?;
                let created_at: String = row.get(4)?;
                let embedding: Vec<u8> = row.get(5)?;
                Ok((id, session, content, tags, created_at, embedding))
            })?;
            iter.collect::<Result<Vec<_>, rusqlite::Error>>()?
        };
        let mut hits: Vec<NoteHit> = rows
            .into_iter()
            .map(|(id, session, content, tags, created_at, embedding)| {
                let vec = le_bytes_to_f32(&embedding);
                let score = cosine(&q, &vec);
                NoteHit {
                    note: Note {
                        id,
                        session,
                        content,
                        created_at,
                        tags: split_tags(&tags),
                    },
                    score,
                }
            })
            .collect();
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(k);
        Ok(hits)
    }

    pub fn fact_set(&self, key: &str, value: &str) -> Result<()> {
        let now = iso_now()?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO facts (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, now],
        )?;
        Ok(())
    }

    pub fn fact_get(&self, key: &str) -> Result<Option<Fact>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT key, value, updated_at FROM facts WHERE key = ?1")?;
        let mut rows = stmt.query([key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Fact {
                key: row.get(0)?,
                value: row.get(1)?,
                updated_at: row.get(2)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn fact_list(&self, prefix: Option<&str>) -> Result<Vec<Fact>> {
        let conn = self.conn.lock().unwrap();
        let (sql, args): (&str, Vec<String>) = match prefix {
            Some(p) => (
                "SELECT key, value, updated_at FROM facts WHERE key LIKE ?1 ORDER BY key",
                vec![format!("{p}%")],
            ),
            None => (
                "SELECT key, value, updated_at FROM facts ORDER BY key",
                vec![],
            ),
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok(Fact {
                key: r.get(0)?,
                value: r.get(1)?,
                updated_at: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn session_record(&self, id: &str, started_at: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO sessions (id, started_at) VALUES (?1, ?2)",
            params![id, started_at],
        )?;
        Ok(())
    }

    pub fn session_end(&self, id: &str, ended_at: &str, summary: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET ended_at = ?2, summary = COALESCE(?3, summary) WHERE id = ?1",
            params![id, ended_at, summary],
        )?;
        Ok(())
    }

    pub fn sessions_recent(&self, limit: usize) -> Result<Vec<SessionIndex>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, started_at, ended_at, summary FROM sessions
             ORDER BY started_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(SessionIndex {
                id: r.get(0)?,
                started_at: r.get(1)?,
                ended_at: r.get(2)?,
                summary: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn note_count(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    /// Total `notes` row size in bytes, excluding SQLite overhead.
    /// Used to decide whether to prune.
    pub fn notes_bytes(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let n: Option<i64> = conn
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(content) + COALESCE(LENGTH(embedding),0) \
                 + LENGTH(COALESCE(tags,'')) + LENGTH(id)), 0) FROM notes",
                [],
                |r| r.get(0),
            )?;
        Ok(n.unwrap_or(0) as u64)
    }

    /// Compaction policy applied once per periodic sweep. Three cuts
    /// in order:
    ///   1. drop notes older than `ttl_days` days (if ttl_days > 0)
    ///   2. if the table still exceeds `max_notes`, drop the oldest
    ///      excess rows
    ///   3. if the content byte count still exceeds `max_bytes`, drop
    ///      the oldest rows until we fit
    /// Then VACUUM to actually give the pages back to the filesystem.
    ///
    /// Returns a `CompactReport` so the caller can log it.
    pub fn compact(&self, policy: CompactPolicy) -> Result<CompactReport> {
        let mut report = CompactReport::default();
        let before_count = self.note_count()?;
        let before_bytes = self.notes_bytes()?;

        let conn = self.conn.lock().unwrap();

        // 1. TTL cutoff.
        if policy.ttl_days > 0 {
            let cutoff = {
                let now = time::OffsetDateTime::now_utc();
                let dur = time::Duration::days(policy.ttl_days as i64);
                let then = now - dur;
                then.format(&time::format_description::well_known::Rfc3339)?
            };
            let n = conn.execute(
                "DELETE FROM notes WHERE created_at < ?1",
                params![cutoff],
            )?;
            report.ttl_deleted = n as u64;
        }

        // 2. Max-count cutoff.
        if policy.max_notes > 0 {
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))?;
            if (n as u64) > policy.max_notes {
                let excess = n as u64 - policy.max_notes;
                let deleted = conn.execute(
                    "DELETE FROM notes WHERE id IN (
                         SELECT id FROM notes ORDER BY created_at ASC LIMIT ?1
                     )",
                    params![excess as i64],
                )?;
                report.count_deleted = deleted as u64;
            }
        }

        // 3. Max-bytes cutoff. Deletes the oldest rows in batches of
        //    100 until we fit; cap total deletions at 10k to avoid
        //    spending a long time inside the sqlite mutex.
        if policy.max_bytes > 0 {
            let mut budget = 10_000u64;
            loop {
                let n: Option<i64> = conn
                    .query_row(
                        "SELECT COALESCE(SUM(LENGTH(content) + COALESCE(LENGTH(embedding),0) \
                         + LENGTH(COALESCE(tags,'')) + LENGTH(id)), 0) FROM notes",
                        [],
                        |r| r.get(0),
                    )?;
                let bytes = n.unwrap_or(0) as u64;
                if bytes <= policy.max_bytes || budget == 0 {
                    break;
                }
                let batch = std::cmp::min(100, budget);
                let deleted = conn.execute(
                    "DELETE FROM notes WHERE id IN (
                         SELECT id FROM notes ORDER BY created_at ASC LIMIT ?1
                     )",
                    params![batch as i64],
                )?;
                if deleted == 0 {
                    break;
                }
                report.bytes_deleted += deleted as u64;
                budget = budget.saturating_sub(deleted as u64);
            }
        }

        // VACUUM reclaims disk. Only run it when we actually deleted
        // something — vacuum of a quiescent DB is just I/O for nothing.
        if report.total() > 0 {
            conn.execute_batch("VACUUM")?;
            report.vacuumed = true;
        }

        report.before_count = before_count;
        report.before_bytes = before_bytes;
        report.after_count = {
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))?;
            n as u64
        };
        Ok(report)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CompactPolicy {
    pub ttl_days: u32,
    pub max_notes: u64,
    pub max_bytes: u64,
}

impl Default for CompactPolicy {
    fn default() -> Self {
        Self { ttl_days: 180, max_notes: 10_000, max_bytes: 128 * 1024 * 1024 }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CompactReport {
    pub ttl_deleted: u64,
    pub count_deleted: u64,
    pub bytes_deleted: u64,
    pub before_count: u64,
    pub after_count: u64,
    pub before_bytes: u64,
    pub vacuumed: bool,
}

impl CompactReport {
    pub fn total(&self) -> u64 {
        self.ttl_deleted + self.count_deleted + self.bytes_deleted
    }
}

fn iso_now() -> Result<String> {
    let now = time::OffsetDateTime::now_utc();
    Ok(now.format(&time::format_description::well_known::Rfc3339)?)
}

fn f32_to_le_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn le_bytes_to_f32(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn split_tags(s: &str) -> Vec<String> {
    s.split(',').filter(|t| !t.is_empty()).map(|t| t.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::HashEmbedder;

    #[tokio::test]
    async fn note_recall_round_trip() {
        let store = MemoryStore::open_in_memory().unwrap();
        let emb = HashEmbedder::new(128);
        store.note(Some("s1"), "the cat sat on the mat", &[], &emb).await.unwrap();
        store.note(Some("s1"), "quantum chromodynamics is strong", &[], &emb).await.unwrap();
        store.note(Some("s2"), "my favourite cat is Lucien", &[], &emb).await.unwrap();

        // The HashEmbedder is a bag-of-tokens hash, not a semantic model;
        // pick a query that literally shares tokens with the target notes.
        let hits = store.recall("find my cat", 2, &emb).await.unwrap();
        assert_eq!(hits.len(), 2);
        // Both top hits should be cat-related, with non-zero scores,
        // and strictly rank above the quantum note.
        for h in &hits {
            assert!(h.note.content.to_lowercase().contains("cat"),
                "unexpected recall: {h:?}");
            assert!(h.score > 0.0, "zero score on {h:?}");
        }
        assert_eq!(store.note_count().unwrap(), 3);
    }

    #[test]
    fn facts_set_overwrite_get_list() {
        let store = MemoryStore::open_in_memory().unwrap();
        store.fact_set("user.name", "Diana").unwrap();
        store.fact_set("user.name", "Diane").unwrap();
        store.fact_set("user.city", "Lyon").unwrap();
        assert_eq!(store.fact_get("user.name").unwrap().unwrap().value, "Diane");
        let list = store.fact_list(Some("user.")).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|f| f.key == "user.name"));
    }

    #[tokio::test]
    async fn compact_applies_ttl_count_and_bytes() {
        let store = MemoryStore::open_in_memory().unwrap();
        let emb = HashEmbedder::new(32);
        // Seed 20 notes.
        for i in 0..20 {
            store
                .note(Some("s"), &format!("note {i}: content body here"), &[], &emb)
                .await
                .unwrap();
        }
        assert_eq!(store.note_count().unwrap(), 20);

        // Count cap: keep 5 newest.
        let r = store.compact(CompactPolicy {
            ttl_days: 0,
            max_notes: 5,
            max_bytes: 0,
        }).unwrap();
        assert_eq!(r.count_deleted, 15);
        assert_eq!(store.note_count().unwrap(), 5);
        assert!(r.vacuumed);

        // Second pass with no cuts to apply → no-op, no VACUUM.
        let r2 = store.compact(CompactPolicy {
            ttl_days: 0,
            max_notes: 5,
            max_bytes: 0,
        }).unwrap();
        assert_eq!(r2.total(), 0);
        assert!(!r2.vacuumed);
    }

    #[tokio::test]
    async fn compact_bytes_cap_prunes_oldest() {
        let store = MemoryStore::open_in_memory().unwrap();
        let emb = HashEmbedder::new(16);
        for i in 0..50 {
            store.note(Some("s"), &format!("content {i}"), &[], &emb).await.unwrap();
        }
        let total = store.notes_bytes().unwrap();
        assert!(total > 0);
        // Cap to a quarter: expect at least a quarter to disappear.
        let budget = total / 4;
        let r = store.compact(CompactPolicy {
            ttl_days: 0,
            max_notes: 0,
            max_bytes: budget,
        }).unwrap();
        assert!(r.bytes_deleted > 0, "expected some bytes-driven deletions: {r:?}");
        assert!(store.notes_bytes().unwrap() <= budget, "post-compact bytes exceed budget");
    }

    #[test]
    fn sessions_record_and_summarize() {
        let store = MemoryStore::open_in_memory().unwrap();
        store.session_record("S1", "2026-10-20T10:00:00Z").unwrap();
        store.session_record("S2", "2026-10-20T11:00:00Z").unwrap();
        store.session_end("S1", "2026-10-20T10:45:00Z", Some("reviewed PR #128")).unwrap();
        let recent = store.sessions_recent(5).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].id, "S2");
        let s1 = recent.iter().find(|s| s.id == "S1").unwrap();
        assert_eq!(s1.summary.as_deref(), Some("reviewed PR #128"));
    }

    #[tokio::test]
    async fn survives_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("wilai.sqlite");
        let emb = HashEmbedder::new(32);
        {
            let s = MemoryStore::open(&path).unwrap();
            s.note(None, "remember me across restarts", &["persist".into()], &emb).await.unwrap();
            s.fact_set("boot.count", "1").unwrap();
        }
        let s = MemoryStore::open(&path).unwrap();
        assert_eq!(s.note_count().unwrap(), 1);
        assert_eq!(s.fact_get("boot.count").unwrap().unwrap().value, "1");
        let hits = s.recall("restarts", 1, &emb).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].note.tags, vec!["persist".to_string()]);
    }
}
