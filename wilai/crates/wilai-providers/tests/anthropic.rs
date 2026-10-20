//! Wire-level tests for AnthropicProvider. Covers the asymmetry with
//! OpenAI/Ollama: assistant tool_calls become `tool_use` content blocks,
//! tool-role messages fold into `tool_result` blocks inside a user
//! message, and the `system` field goes top-level (not in messages[]).

use serde_json::json;
use wilai_providers::{
    AnthropicProvider, ChatRequest, Message, MessageRole, Provider, ToolCall, ToolSchema,
};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_request() -> ChatRequest {
    ChatRequest {
        model: "claude-opus-4-7".to_string(),
        system: Some("you are wilai".to_string()),
        messages: vec![
            Message {
                role: MessageRole::User,
                content: "list /etc".to_string(),
                tool_call_id: None,
                tool_calls: vec![],
            },
            Message {
                role: MessageRole::Assistant,
                content: "let me check".to_string(),
                tool_call_id: None,
                tool_calls: vec![ToolCall {
                    id: "toolu_1".into(),
                    name: "fs.list".into(),
                    arguments: json!({ "path": "/etc" }),
                }],
            },
            Message {
                role: MessageRole::Tool,
                content: "d etc\nf passwd".to_string(),
                tool_call_id: Some("toolu_1".to_string()),
                tool_calls: vec![],
            },
        ],
        tools: vec![ToolSchema {
            name: "fs.list".to_string(),
            description: "list dir".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        }],
        max_tokens: Some(1024),
    }
}

#[tokio::test]
async fn text_and_tool_use_blocks_parse() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [
                { "type": "text", "text": "running fs.list now" },
                { "type": "tool_use", "id": "toolu_2", "name": "fs.list", "input": {"path": "/var"} }
            ],
            "usage": { "input_tokens": 123, "output_tokens": 45 },
            "stop_reason": "tool_use"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let p = AnthropicProvider::new("claude", "test-key".to_string(), Some(format!("{}/v1/messages", server.uri())))
        .unwrap();
    let resp = p.chat(make_request()).await.expect("chat");
    assert_eq!(resp.message.content, "running fs.list now");
    assert_eq!(resp.message.tool_calls.len(), 1);
    assert_eq!(resp.message.tool_calls[0].id, "toolu_2");
    assert_eq!(resp.message.tool_calls[0].name, "fs.list");
    assert_eq!(resp.message.tool_calls[0].arguments, json!({ "path": "/var" }));
    assert_eq!(resp.prompt_tokens, Some(123));
    assert_eq!(resp.output_tokens, Some(45));
}

#[tokio::test]
async fn request_shape_matches_anthropic_schema() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type":"text","text":"ok"}],
            "usage": {"input_tokens":1,"output_tokens":1}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let p = AnthropicProvider::new("claude", "k".into(), Some(format!("{}/v1/messages", server.uri()))).unwrap();
    let _ = p.chat(make_request()).await.expect("chat");

    let received = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();

    assert_eq!(body["model"], "claude-opus-4-7");
    assert_eq!(body["max_tokens"], 1024);
    assert_eq!(body["system"], "you are wilai");

    // Tools come across in Anthropic shape: {name, description, input_schema}.
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "fs.list");
    assert!(tools[0]["input_schema"].is_object());

    // Messages are a mix of role=user and role=assistant, system is top-level only.
    let msgs = body["messages"].as_array().unwrap();
    // Expected: user "list /etc", assistant [text + tool_use], user [tool_result]
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[0]["content"][0]["type"], "text");
    assert_eq!(msgs[0]["content"][0]["text"], "list /etc");

    assert_eq!(msgs[1]["role"], "assistant");
    let a_blocks = msgs[1]["content"].as_array().unwrap();
    assert!(a_blocks.iter().any(|b| b["type"] == "text"));
    let tu = a_blocks.iter().find(|b| b["type"] == "tool_use").unwrap();
    assert_eq!(tu["name"], "fs.list");
    assert_eq!(tu["id"], "toolu_1");

    assert_eq!(msgs[2]["role"], "user");
    let tr = &msgs[2]["content"][0];
    assert_eq!(tr["type"], "tool_result");
    assert_eq!(tr["tool_use_id"], "toolu_1");
    assert!(tr["content"].as_str().unwrap().contains("passwd"));
}

#[tokio::test]
async fn empty_api_key_rejected_before_wire() {
    let err = AnthropicProvider::new("claude", "".to_string(), None)
        .err()
        .expect("empty key should be rejected");
    let s = format!("{err:#}");
    assert!(s.to_lowercase().contains("key"));
}

#[tokio::test]
async fn http_error_bubbles() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .expect(1)
        .mount(&server)
        .await;

    let p = AnthropicProvider::new("claude", "k".into(), Some(format!("{}/v1/messages", server.uri()))).unwrap();
    let err = p.chat(make_request()).await.unwrap_err();
    let s = format!("{err:#}");
    assert!(s.contains("401"), "error should mention status: {s}");
}
