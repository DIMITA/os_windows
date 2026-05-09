//! JSON-RPC 2.0 wire types for the Model Context Protocol (MCP).
//!
//! Only the subset we need: initialize / initialized / tools.list /
//! tools.call. Resources, prompts, sampling, and notifications beyond
//! `notifications/initialized` are not implemented in v2.0.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Clone, Serialize)]
pub struct Request<P: Serialize> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    pub params: P,
}

impl<P: Serialize> Request<P> {
    pub fn new(id: u64, method: impl Into<String>, params: P) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Notification<P: Serialize> {
    pub jsonrpc: &'static str,
    pub method: String,
    pub params: P,
}

impl<P: Serialize> Notification<P> {
    pub fn new(method: impl Into<String>, params: P) -> Self {
        Self {
            jsonrpc: "2.0",
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(bound(deserialize = "R: serde::de::DeserializeOwned"))]
pub struct Response<R> {
    #[serde(default)]
    pub jsonrpc: Option<String>,
    pub id: Option<u64>,
    #[serde(default = "none_result")]
    pub result: Option<R>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

fn none_result<R>() -> Option<R> { None }

#[derive(Debug, Clone, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: &'static str,
    pub capabilities: Value,
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo {
    pub name: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: Option<String>,
    pub capabilities: Option<Value>,
    #[serde(rename = "serverInfo")]
    pub server_info: Option<ServerInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerInfo {
    pub name: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<McpToolDescriptor>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpToolDescriptor {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CallToolParams<'a> {
    pub name: &'a str,
    pub arguments: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CallToolResult {
    #[serde(default)]
    pub content: Vec<ToolContent>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolContent {
    Text { text: String },
    Image {
        #[serde(default)]
        data: Option<String>,
        #[serde(default, rename = "mimeType")]
        mime_type: Option<String>,
    },
    Resource {
        #[serde(default)]
        resource: Option<Value>,
    },
}

impl ToolContent {
    pub fn render(&self) -> String {
        match self {
            ToolContent::Text { text } => text.clone(),
            ToolContent::Image { mime_type, .. } => {
                format!("[image {}]", mime_type.as_deref().unwrap_or("?"))
            }
            ToolContent::Resource { .. } => "[resource]".to_string(),
        }
    }
}
