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
    /// Plain text content from the assistant.
    Text { content: String },
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
    /// Final event before the connection closes.
    Bye,
}
