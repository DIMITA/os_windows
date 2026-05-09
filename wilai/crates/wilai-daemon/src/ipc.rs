use crate::protocol::{ConfirmReply, ServerEvent};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;
use wilai_tools::confirm::{ConfirmAnswer, ConfirmDefault, Confirmer};

#[derive(Default)]
pub struct PendingConfirms {
    map: Mutex<HashMap<String, oneshot::Sender<ConfirmReply>>>,
}

impl PendingConfirms {
    pub fn register(&self, id: String, tx: oneshot::Sender<ConfirmReply>) {
        self.map.lock().unwrap().insert(id, tx);
    }
    pub fn deliver(&self, id: &str, reply: ConfirmReply) -> bool {
        if let Some(tx) = self.map.lock().unwrap().remove(id) {
            let _ = tx.send(reply);
            true
        } else {
            false
        }
    }
}

pub struct IpcConfirmer {
    pub event_tx: mpsc::Sender<ServerEvent>,
    pub pending: Arc<PendingConfirms>,
}

#[async_trait]
impl Confirmer for IpcConfirmer {
    async fn ask(
        &self,
        prompt: &str,
        default: ConfirmDefault,
        timeout_s: u32,
    ) -> Result<ConfirmAnswer> {
        let id = ulid::Ulid::new().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.register(id.clone(), tx);
        let default_str = match default {
            ConfirmDefault::Yes => "yes",
            ConfirmDefault::No => "no",
            ConfirmDefault::Required => "required",
        }
        .to_string();
        self.event_tx
            .send(ServerEvent::ConfirmAsk {
                id,
                prompt: prompt.to_string(),
                default: default_str,
                timeout_s,
            })
            .await
            .map_err(|e| anyhow::anyhow!("send confirm ask: {e}"))?;

        match timeout(Duration::from_secs(timeout_s as u64), rx).await {
            Ok(Ok(ConfirmReply::Yes)) => Ok(ConfirmAnswer::Yes),
            Ok(Ok(ConfirmReply::No)) => Ok(ConfirmAnswer::No),
            Ok(Err(_)) => Ok(ConfirmAnswer::No),
            Err(_) => Ok(ConfirmAnswer::Timeout),
        }
    }
}
