use crate::chain::{recover_from_tail, sha256_hex, ChainHead, GENESIS_HASH};
use crate::entry::EntryPayload;
use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::sync::mpsc;
use wilai_core::Mode;

const SCHEMA_VERSION: u32 = 1;

pub struct WriteRequest {
    pub session: String,
    pub mode: Mode,
    pub payload: EntryPayload,
}

pub struct AuditWriter {
    dir: PathBuf,
    file: File,
    file_name: String,
    running_hash: String,
    seq: u64,
}

impl AuditWriter {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create audit dir {}", dir.display()))?;

        let today = today_filename()?;
        let file_path = dir.join(&today);
        let head_path = dir.join("chain.head");

        let mut writer = if file_path.exists() && file_path.metadata()?.len() > 0 {
            let (running_hash, last_seq) = recover_from_tail(&file_path)
                .with_context(|| format!("recover tail of {}", file_path.display()))?;
            let file = OpenOptions::new().append(true).open(&file_path)?;
            Self {
                dir: dir.to_path_buf(),
                file,
                file_name: today.clone(),
                running_hash,
                seq: last_seq,
            }
        } else {
            let head = ChainHead::load(&head_path)?;
            let running = head
                .as_ref()
                .map(|h| h.running_hash.clone())
                .unwrap_or_else(|| GENESIS_HASH.to_string());
            let file = OpenOptions::new()
                .append(true)
                .create(true)
                .open(&file_path)?;
            Self {
                dir: dir.to_path_buf(),
                file,
                file_name: today,
                running_hash: running,
                seq: 0,
            }
        };

        update_symlink(dir, &writer.file_name)?;

        if writer.seq == 0 && writer.running_hash == GENESIS_HASH {
            writer.write(WriteRequest {
                session: "-".to_string(),
                mode: Mode::Normal,
                payload: EntryPayload::SystemChainGenesis,
            })?;
        }

        Ok(writer)
    }

    pub fn write(&mut self, req: WriteRequest) -> Result<()> {
        self.seq += 1;
        let ts = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .context("format ts")?;
        let id = wilai_core::types::new_ulid();

        let mut obj = Map::new();
        obj.insert("v".into(), Value::from(SCHEMA_VERSION));
        obj.insert("seq".into(), Value::from(self.seq));
        obj.insert("ts".into(), Value::from(ts));
        obj.insert("id".into(), Value::from(id));
        obj.insert("prev_hash".into(), Value::from(self.running_hash.clone()));
        obj.insert("kind".into(), Value::from(req.payload.kind()));
        obj.insert("session".into(), Value::from(req.session));
        obj.insert("mode".into(), serde_json::to_value(req.mode)?);
        if let Value::Object(payload) = req.payload.to_value() {
            for (k, v) in payload {
                obj.insert(k, v);
            }
        }

        let bytes = serde_json::to_vec(&Value::Object(obj))?;
        self.file.write_all(&bytes)?;
        self.file.write_all(b"\n")?;
        self.file.sync_data()?;

        self.running_hash = sha256_hex(&bytes);

        let head = ChainHead {
            running_hash: self.running_hash.clone(),
            last_seq: self.seq,
            current_file: self.file_name.clone(),
        };
        head.save_atomic(&self.dir.join("chain.head"))?;
        Ok(())
    }

    pub fn running_hash(&self) -> &str {
        &self.running_hash
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn spawn_task(mut self) -> (mpsc::Sender<WriteRequest>, tokio::task::JoinHandle<()>) {
        let (tx, mut rx) = mpsc::channel::<WriteRequest>(256);
        let handle = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            while let Some(req) = rt.block_on(rx.recv()) {
                if let Err(e) = self.write(req) {
                    tracing::error!("audit write failed: {e:#}");
                }
            }
        });
        (tx, handle)
    }
}

fn today_filename() -> Result<String> {
    let now = OffsetDateTime::now_utc();
    Ok(format!(
        "{:04}-{:02}-{:02}.jsonl",
        now.year(),
        u8::from(now.month()),
        now.day()
    ))
}

fn update_symlink(dir: &Path, target: &str) -> Result<()> {
    let link = dir.join("current.jsonl");
    let tmp = dir.join("current.jsonl.tmp");
    if tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, &tmp)?;
        std::fs::rename(&tmp, &link)?;
    }
    #[cfg(not(unix))]
    {
        let _ = (target, &link);
    }
    Ok(())
}
