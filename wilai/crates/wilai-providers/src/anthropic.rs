use crate::provider::{
    AssistantMessage, ChatRequest, ChatResponse, Message, MessageRole, Provider, ToolCall,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const ANTHROPIC_API: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    name: String,
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(name: impl Into<String>, api_key: String, base_url: Option<String>) -> Result<Self> {
        if api_key.is_empty() {
            anyhow::bail!("anthropic api key is empty");
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()?;
        Ok(Self {
            name: name.into(),
            api_key,
            base_url: base_url.unwrap_or_else(|| ANTHROPIC_API.to_string()),
            client,
        })
    }
}

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool>,
}

#[derive(Serialize, Debug)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicBlock>,
}

#[derive(Serialize, Debug)]
#[serde(untagged)]
enum AnthropicBlock {
    Text { #[serde(rename = "type")] kind: &'static str, text: String },
    ToolUse {
        #[serde(rename = "type")] kind: &'static str,
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        #[serde(rename = "type")] kind: &'static str,
        tool_use_id: String,
        content: String,
    },
}

#[derive(Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Deserialize, Debug)]
struct AnthropicResponse {
    content: Vec<RespBlock>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
struct Usage {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RespBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: Value },
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_local(&self) -> bool {
        false
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse> {
        let messages = build_messages(&req.messages);
        let tools: Vec<AnthropicTool> = req
            .tools
            .into_iter()
            .map(|t| AnthropicTool {
                name: t.name,
                description: t.description,
                input_schema: t.parameters,
            })
            .collect();

        let body = AnthropicRequest {
            model: &req.model,
            max_tokens: req.max_tokens.unwrap_or(4096),
            system: req.system,
            messages,
            tools,
        };

        let resp = self
            .client
            .post(&self.base_url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {}", self.base_url))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("anthropic HTTP {status}: {text}");
        }
        let parsed: AnthropicResponse = serde_json::from_str(&text)
            .with_context(|| format!("parse anthropic response: {text}"))?;

        let mut content = String::new();
        let mut tool_calls = Vec::new();
        for block in parsed.content {
            match block {
                RespBlock::Text { text } => content.push_str(&text),
                RespBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments: input,
                    });
                }
            }
        }
        let _ = parsed.stop_reason;

        Ok(ChatResponse {
            message: AssistantMessage { content, tool_calls },
            prompt_tokens: parsed.usage.as_ref().and_then(|u| u.input_tokens),
            output_tokens: parsed.usage.as_ref().and_then(|u| u.output_tokens),
        })
    }
}

fn build_messages(input: &[Message]) -> Vec<AnthropicMessage> {
    let mut out: Vec<AnthropicMessage> = Vec::new();
    for m in input {
        match m.role {
            MessageRole::System => continue, // surfaced via top-level `system`
            MessageRole::User => {
                out.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: vec![AnthropicBlock::Text {
                        kind: "text",
                        text: m.content.clone(),
                    }],
                });
            }
            MessageRole::Assistant => {
                let mut blocks = Vec::new();
                if !m.content.is_empty() {
                    blocks.push(AnthropicBlock::Text {
                        kind: "text",
                        text: m.content.clone(),
                    });
                }
                for tc in &m.tool_calls {
                    blocks.push(AnthropicBlock::ToolUse {
                        kind: "tool_use",
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input: tc.arguments.clone(),
                    });
                }
                out.push(AnthropicMessage {
                    role: "assistant".to_string(),
                    content: blocks,
                });
            }
            MessageRole::Tool => {
                let id = m
                    .tool_call_id
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                // Anthropic puts tool results inside a user message.
                let result_block = AnthropicBlock::ToolResult {
                    kind: "tool_result",
                    tool_use_id: id,
                    content: m.content.clone(),
                };
                if let Some(last) = out.last_mut() {
                    if last.role == "user" {
                        last.content.push(result_block);
                        continue;
                    }
                }
                out.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: vec![result_block],
                });
            }
        }
    }
    out
}

