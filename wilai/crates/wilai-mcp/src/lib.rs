//! Wilai MCP client: spawn JSON-RPC 2.0 MCP servers over stdio, initialize
//! them, list their tools, and forward tool calls. Tools surface in the
//! Wilai registry under the prefix `mcp.<server>.<tool>` and are
//! validator-/guard-/audit-clean like any other tool.

pub mod client;
pub mod convert;
pub mod wire;

pub use client::{McpClient, McpToolHandle};
pub use wire::{McpToolDescriptor, ToolContent};
