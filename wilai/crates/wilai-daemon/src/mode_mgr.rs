//! Mode manager: shared, auditable Normal/Pentest state with guards on
//! exit (cannot leave pentest while a pentest-category tool is in flight).

use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use wilai_audit::entry::EntryPayload;
use wilai_audit::writer::WriteRequest;
use wilai_core::Mode;

#[derive(Debug, Clone)]
pub struct ModeChange {
    pub from: Mode,
    pub to: Mode,
    pub trigger: String,
}

pub struct ModeManager {
    current: RwLock<Mode>,
    pentest_in_flight: AtomicU32,
    audit_tx: mpsc::Sender<WriteRequest>,
    listeners: RwLock<Vec<mpsc::Sender<ModeChange>>>,
}

impl ModeManager {
    pub fn new(initial: Mode, audit_tx: mpsc::Sender<WriteRequest>) -> Arc<Self> {
        Arc::new(Self {
            current: RwLock::new(initial),
            pentest_in_flight: AtomicU32::new(0),
            audit_tx,
            listeners: RwLock::new(Vec::new()),
        })
    }

    pub async fn current(&self) -> Mode {
        *self.current.read().await
    }

    pub async fn set(&self, to: Mode, trigger: &str) -> Result<ModeChange> {
        let from = *self.current.read().await;
        if from == to {
            return Ok(ModeChange {
                from,
                to,
                trigger: trigger.to_string(),
            });
        }
        if from == Mode::Pentest && to == Mode::Normal {
            let n = self.pentest_in_flight.load(Ordering::SeqCst);
            if n > 0 {
                return Err(anyhow!(
                    "cannot leave pentest mode: {n} pentest tool(s) in flight"
                ));
            }
        }
        {
            let mut w = self.current.write().await;
            *w = to;
        }
        let _ = self
            .audit_tx
            .send(WriteRequest {
                session: "-".to_string(),
                mode: to,
                payload: EntryPayload::ModeChange {
                    from,
                    to,
                    trigger: trigger.to_string(),
                },
            })
            .await;
        let change = ModeChange {
            from,
            to,
            trigger: trigger.to_string(),
        };
        // Best-effort fanout to subscribers.
        let mut listeners = self.listeners.write().await;
        listeners.retain(|tx| tx.try_send(change.clone()).is_ok() || !tx.is_closed());
        Ok(change)
    }

    pub async fn subscribe(&self) -> mpsc::Receiver<ModeChange> {
        let (tx, rx) = mpsc::channel(8);
        self.listeners.write().await.push(tx);
        rx
    }

    pub fn pentest_started(&self) {
        self.pentest_in_flight.fetch_add(1, Ordering::SeqCst);
    }

    pub fn pentest_finished(&self) {
        self.pentest_in_flight.fetch_sub(1, Ordering::SeqCst);
    }

    pub fn pentest_in_flight(&self) -> u32 {
        self.pentest_in_flight.load(Ordering::SeqCst)
    }
}
