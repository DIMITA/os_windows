//! Scripted test provider: hand it a sequence of `ChatResponse` and it
//! returns them in order. Any extra calls get an error. Public (not
//! #[cfg(test)]) so integration tests outside the crate can use it and
//! so operators can wire it in as a `type = "mock"` provider from
//! wilai.toml when debugging without touching a real LLM.

use crate::provider::{AssistantMessage, ChatRequest, ChatResponse, Provider};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

pub struct MockProvider {
    name: String,
    responses: Mutex<Vec<ChatResponse>>,
    call_count: AtomicUsize,
    captured_requests: Mutex<Vec<ChatRequest>>,
}

impl MockProvider {
    /// Create a mock scripted with a sequence of responses. The provider
    /// hands them out in order on each `chat()` call.
    pub fn with_script(name: impl Into<String>, responses: Vec<ChatResponse>) -> Self {
        Self {
            name: name.into(),
            responses: Mutex::new(responses),
            call_count: AtomicUsize::new(0),
            captured_requests: Mutex::new(Vec::new()),
        }
    }

    /// Convenience: a single turn that just produces a plain text reply.
    pub fn with_text(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self::with_script(
            name,
            vec![ChatResponse {
                message: AssistantMessage {
                    content: text.into(),
                    tool_calls: vec![],
                },
                prompt_tokens: Some(1),
                output_tokens: Some(1),
            }],
        )
    }

    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    pub fn captured(&self) -> Vec<ChatRequest> {
        self.captured_requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_local(&self) -> bool {
        true
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        self.captured_requests.lock().unwrap().push(req);
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let mut scripted = self.responses.lock().unwrap();
        if scripted.is_empty() {
            return Err(anyhow!("MockProvider ran out of scripted responses"));
        }
        Ok(scripted.remove(0))
    }
}
