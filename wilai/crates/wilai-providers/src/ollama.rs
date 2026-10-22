use crate::provider::{
    AssistantMessage, ChatRequest, ChatResponse, Message, MessageRole, Provider, ToolCall,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub struct OllamaProvider {
    name: String,
    base_url: String,
    client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(name: impl Into<String>, base_url: impl Into<String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()?;
        Ok(Self {
            name: name.into(),
            base_url: base_url.into(),
            client,
        })
    }

    /// Tell Ollama to drop a model from RAM immediately. We POST to
    /// /api/generate with an empty prompt and keep_alive=0 — that's
    /// the documented Ollama idiom for forcing an unload without
    /// generating any tokens. Short timeout (5s): if Ollama is slow
    /// or already gone, we give up rather than blocking shutdown.
    pub async fn unload_model(&self, model: &str) -> Result<()> {
        let url = format!("{}/api/generate", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": model,
            "prompt": "",
            "stream": false,
            "keep_alive": 0,
        });
        let short = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        short
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        Ok(())
    }

    pub fn base_url(&self) -> &str { &self.base_url }

    /// Prewarm a model by loading it into Ollama's RAM with a long
    /// keep_alive, so the first real prompt skips the cold-start.
    /// POSTs an empty prompt — Ollama loads the model and returns
    /// immediately without generating tokens.
    pub async fn prewarm(&self, model: &str) -> Result<()> {
        let url = format!("{}/api/generate", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": model,
            "prompt": "",
            "stream": false,
            // Hold the model in RAM for 10 min; subsequent prompts refresh it.
            "keep_alive": "10m",
        });
        // Longer timeout than unload — the model might take 30s+ to load
        // on a cold Ollama with a chunky model.
        let slow = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;
        slow.post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        Ok(())
    }
}

#[derive(Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OllamaTool>,
    stream: bool,
    options: OllamaOptions,
}

#[derive(Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug)]
struct OllamaMessage {
    role: String,
    content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<OllamaToolCall>,
}

#[derive(Serialize, Deserialize, Debug)]
struct OllamaToolCall {
    function: OllamaFunctionCall,
}

#[derive(Serialize, Deserialize, Debug)]
struct OllamaFunctionCall {
    name: String,
    arguments: Value,
}

#[derive(Serialize)]
struct OllamaTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: OllamaFunctionSpec,
}

#[derive(Serialize)]
struct OllamaFunctionSpec {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Deserialize, Debug)]
struct OllamaChatResponse {
    message: OllamaMessage,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    #[serde(default)]
    eval_count: Option<u32>,
}

#[async_trait]
impl Provider for OllamaProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_local(&self) -> bool {
        true
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        let mut messages: Vec<OllamaMessage> = Vec::with_capacity(req.messages.len() + 1);
        if let Some(sys) = req.system {
            messages.push(OllamaMessage {
                role: "system".to_string(),
                content: sys,
                tool_calls: vec![],
            });
        }
        for m in req.messages {
            messages.push(to_ollama_message(m));
        }

        let tools: Vec<OllamaTool> = req
            .tools
            .into_iter()
            .map(|t| OllamaTool {
                kind: "function",
                function: OllamaFunctionSpec {
                    name: t.name,
                    description: t.description,
                    parameters: t.parameters,
                },
            })
            .collect();

        let body = OllamaChatRequest {
            model: &req.model,
            messages,
            tools,
            stream: false,
            options: OllamaOptions {
                num_predict: req.max_tokens,
            },
        };

        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let resp = self.client.post(&url).json(&body).send().await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("ollama HTTP {}: {}", status, text);
        }
        let parsed: OllamaChatResponse = serde_json::from_str(&text)
            .with_context(|| format!("parse ollama response: {text}"))?;

        let tool_calls: Vec<ToolCall> = parsed
            .message
            .tool_calls
            .into_iter()
            .enumerate()
            .map(|(i, tc)| ToolCall {
                id: format!("call_{i}"),
                name: tc.function.name,
                arguments: tc.function.arguments,
            })
            .collect();

        Ok(ChatResponse {
            message: AssistantMessage {
                content: parsed.message.content,
                tool_calls,
            },
            prompt_tokens: parsed.prompt_eval_count,
            output_tokens: parsed.eval_count,
        })
    }

    async fn chat_streaming(
        &self,
        req: ChatRequest,
        chunk_tx: tokio::sync::mpsc::Sender<String>,
    ) -> Result<ChatResponse> {
        // Build the request the same way as chat(), but with stream=true.
        let mut messages: Vec<OllamaMessage> = Vec::with_capacity(req.messages.len() + 1);
        if let Some(sys) = req.system {
            messages.push(OllamaMessage {
                role: "system".to_string(),
                content: sys,
                tool_calls: vec![],
            });
        }
        for m in req.messages {
            messages.push(to_ollama_message(m));
        }
        let tools: Vec<OllamaTool> = req
            .tools
            .into_iter()
            .map(|t| OllamaTool {
                kind: "function",
                function: OllamaFunctionSpec {
                    name: t.name,
                    description: t.description,
                    parameters: t.parameters,
                },
            })
            .collect();
        let body = OllamaChatRequest {
            model: &req.model,
            messages,
            tools,
            stream: true,
            options: OllamaOptions {
                num_predict: req.max_tokens,
            },
        };

        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let resp = self.client.post(&url).json(&body).send().await
            .with_context(|| format!("POST {url}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("ollama HTTP {}: {}", status, text);
        }

        // Ollama streams NDJSON: one JSON object per line, each with
        // {"message":{"content":"...","tool_calls":[...]},"done":bool,
        //  "prompt_eval_count":..,"eval_count":..}.
        // We accumulate content, forward each non-empty delta to
        // chunk_tx, and surface tool_calls from the final object.
        use tokio::io::AsyncBufReadExt;
        use tokio::io::BufReader;
        use futures_util::StreamExt;
        let stream = resp.bytes_stream().map(|r| r.map_err(std::io::Error::other));
        let reader = tokio_util::io::StreamReader::new(stream);
        let mut lines = BufReader::new(reader).lines();

        let mut full_content = String::new();
        let mut final_tool_calls: Vec<OllamaToolCall> = Vec::new();
        let mut prompt_tokens: Option<u32> = None;
        let mut output_tokens: Option<u32> = None;

        while let Some(line) = lines.next_line().await.context("read stream line")? {
            if line.trim().is_empty() { continue; }
            let obj: OllamaStreamChunk = serde_json::from_str(&line)
                .with_context(|| format!("parse stream chunk: {line}"))?;
            if !obj.message.content.is_empty() {
                full_content.push_str(&obj.message.content);
                // Non-blocking send; if the receiver is gone, we still
                // keep draining the stream for correctness.
                let _ = chunk_tx.send(obj.message.content).await;
            }
            if !obj.message.tool_calls.is_empty() {
                final_tool_calls = obj.message.tool_calls;
            }
            if obj.done {
                prompt_tokens = obj.prompt_eval_count;
                output_tokens = obj.eval_count;
                break;
            }
        }

        let tool_calls: Vec<ToolCall> = final_tool_calls
            .into_iter()
            .enumerate()
            .map(|(i, tc)| ToolCall {
                id: format!("call_{i}"),
                name: tc.function.name,
                arguments: tc.function.arguments,
            })
            .collect();

        Ok(ChatResponse {
            message: AssistantMessage {
                content: full_content,
                tool_calls,
            },
            prompt_tokens,
            output_tokens,
        })
    }
}

/// One NDJSON object yielded by Ollama's /api/chat?stream=true.
#[derive(Deserialize, Debug)]
struct OllamaStreamChunk {
    message: OllamaMessage,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    #[serde(default)]
    eval_count: Option<u32>,
}

fn to_ollama_message(m: Message) -> OllamaMessage {
    let role = match m.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
    .to_string();
    OllamaMessage {
        role,
        content: m.content,
        tool_calls: m
            .tool_calls
            .into_iter()
            .map(|tc| OllamaToolCall {
                function: OllamaFunctionCall {
                    name: tc.name,
                    arguments: tc.arguments,
                },
            })
            .collect(),
    }
}
