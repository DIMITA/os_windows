use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClientOp {
    /// Send a user prompt; the daemon will run an agent loop and stream
    /// events back until `TurnDone` or `Error`.
    Prompt {
        text: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        provider: Option<String>,
    },
    /// Reply to a previous `ConfirmAsk` event by id.
    ConfirmAnswer { id: String, answer: ConfirmReply },
    /// Query the current mode. Daemon replies with a `Mode` event.
    ModeGet,
    /// Switch the daemon's mode. Replies with `Mode` on success or `Error`
    /// (e.g. when leaving pentest while a pentest tool is in flight).
    ModeSet { to: String, trigger: Option<String> },
    /// Ask the daemon for the current registry snapshot. Includes both
    /// YAML-loaded and dynamically-registered (e.g. MCP) tools.
    ToolsList,
    /// Ask the daemon for MCP supervisor status (one row per configured
    /// server). Replies with `McpServers`.
    McpList,
    /// Trigger an immediate restart of the named MCP server. Replies
    /// with `McpServers` (updated snapshot).
    McpRestart { server: String },
    /// Scan every live MCP supervisor and insert any tool not yet in the
    /// registry. Returns `McpServers` with current status.
    McpResync,
    /// Close the session cleanly.
    Quit,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConfirmReply {
    Yes,
    No,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Sent once per connection right after handshake.
    SessionStart { session: String },
    /// Plain text content from the assistant. Non-streaming providers
    /// (or the agent loop between tool calls) send this once per turn
    /// with the full content.
    Text { content: String },
    /// Incremental token from a streaming provider. Shells concatenate
    /// into the current assistant message and redraw as chunks arrive.
    /// A streaming turn ends with TurnDone (same as non-streaming); the
    /// shell may want to emit the concatenated full text as a single
    /// final Text at that point for TTS / history purposes.
    TextChunk { delta: String },
    /// Informational: a tool is about to be invoked. Useful for UIs to
    /// render progress before the tool returns.
    ToolCall { name: String, args: Value },
    /// Result of a tool call; ok=false carries the error message.
    ToolResult {
        name: String,
        ok: bool,
        output: String,
    },
    /// Confirmation request. The client must answer with `ConfirmAnswer
    /// { id, answer }` within `timeout_s` seconds.
    ConfirmAsk {
        id: String,
        prompt: String,
        default: String,
        timeout_s: u32,
    },
    /// One agent turn finished (assistant produced a non-tool message).
    TurnDone,
    /// Hard error; the agent loop terminated.
    Error { message: String },
    /// Daemon's current mode (response to `ModeGet`/`ModeSet`, or volunteered
    /// when an auto-detect switch happens).
    Mode {
        current: String,
        pentest_in_flight: u32,
    },
    /// Snapshot of the live tool registry (response to `ToolsList`).
    Tools { tools: Vec<ToolSummary> },
    /// Pushed asynchronously when the daemon registers new tools (e.g.
    /// a late-starting MCP server finally came up). Clients can act on
    /// this to refresh their UI without polling.
    ToolsAdded { names: Vec<String> },
    /// Snapshot of MCP supervisor state (response to `McpList` / `McpRestart`).
    McpServers { servers: Vec<McpServerInfo> },
    /// Final event before the connection closes.
    Bye,
    /// Proactive signal emitted by a daemon-side watcher (systemd,
    /// disk, process). Not tied to a user turn; may arrive at any time.
    Nudge {
        id: String,
        kind: String,
        severity: String,
        message: String,
        #[serde(default)]
        context: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub name: String,
    pub alive: bool,
    pub uptime_s: Option<u64>,
    pub restart_count: u32,
    pub tool_count: usize,
    pub last_error: Option<String>,
    pub gated_in_pentest: bool,
    /// True when the supervisor has given up retrying because the
    /// server failed to spawn too many times (typically: wrong binary
    /// name). Clear with `wilai mcp restart <name>`.
    #[serde(default)]
    pub fatal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSummary {
    pub name: String,
    pub version: u32,
    pub category: String,
    pub risk: String,
    pub description: String,
    /// "yaml" for tools loaded from disk, "mcp" for MCP-contributed,
    /// "builtin" for in-process. Used by clients to render origin badges.
    pub origin: String,
}
