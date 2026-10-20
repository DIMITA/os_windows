//! Wire-level tests for OllamaProvider. Spin a wiremock server, point
//! the provider at it, assert both the request shape (what Ollama will
//! see) and the response parsing (what the daemon will see).

use serde_json::json;
use wilai_providers::{ChatRequest, Message, MessageRole, OllamaProvider, Provider, ToolSchema};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_request() -> ChatRequest {
    ChatRequest {
        model: "mistral:7b-instruct".to_string(),
        system: Some("you are wilai".to_string()),
        messages: vec![Message {
            role: MessageRole::User,
            content: "list /etc".to_string(),
            tool_call_id: None,
            tool_calls: vec![],
        }],
        tools: vec![ToolSchema {
            name: "fs.list".to_string(),
            description: "list dir".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        }],
        max_tokens: Some(512),
    }
}

#[tokio::test]
async fn text_only_response_parses() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "mistral:7b-instruct",
            "message": { "role": "assistant", "content": "/etc has many files" },
            "prompt_eval_count": 42,
            "eval_count": 7,
            "done": true,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let p = OllamaProvider::new("ollama", server.uri()).unwrap();
    let resp = p.chat(make_request()).await.expect("chat");
    assert_eq!(resp.message.content, "/etc has many files");
    assert!(resp.message.tool_calls.is_empty());
    assert_eq!(resp.prompt_tokens, Some(42));
    assert_eq!(resp.output_tokens, Some(7));
}

#[tokio::test]
async fn tool_calls_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "mistral:7b-instruct",
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "function": {
                        "name": "fs.list",
                        "arguments": {"path": "/etc"}
                    }
                }]
            },
            "done": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let p = OllamaProvider::new("ollama", server.uri()).unwrap();
    let resp = p.chat(make_request()).await.expect("chat");
    assert_eq!(resp.message.tool_calls.len(), 1);
    let tc = &resp.message.tool_calls[0];
    assert_eq!(tc.name, "fs.list");
    assert_eq!(tc.arguments, json!({ "path": "/etc" }));
}

#[tokio::test]
async fn request_includes_tools_and_system() {
    let server = MockServer::start().await;
    // Capture body shape by returning a canned success and letting the
    // mock recorder assert on request bodies.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "mistral:7b-instruct",
            "message": { "role": "assistant", "content": "ok" },
            "done": true,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let p = OllamaProvider::new("ollama", server.uri()).unwrap();
    let _ = p.chat(make_request()).await.expect("chat");

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(body["model"], "mistral:7b-instruct");
    assert_eq!(body["stream"], false);
    // First message is the system prompt we passed, second is the user.
    let msgs = body["messages"].as_array().expect("messages array");
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(msgs[0]["content"], "you are wilai");
    assert_eq!(msgs[1]["role"], "user");
    assert_eq!(msgs[1]["content"], "list /etc");
    // Tool schema was sent in Ollama shape.
    let tools = body["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "fs.list");
}

#[tokio::test]
async fn non_200_bubbles_as_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .expect(1)
        .mount(&server)
        .await;

    let p = OllamaProvider::new("ollama", server.uri()).unwrap();
    let err = p.chat(make_request()).await.unwrap_err();
    let s = format!("{err:#}");
    assert!(s.contains("500"), "error should mention status: {s}");
    assert!(s.contains("boom"), "error should include body: {s}");
}
