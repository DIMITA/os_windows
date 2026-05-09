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
