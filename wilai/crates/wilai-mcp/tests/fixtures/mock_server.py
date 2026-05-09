#!/usr/bin/env python3
"""Tiny MCP-over-stdio mock for the wilai-mcp client integration test.

Implements just enough of the protocol to:
  - respond to `initialize`
  - acknowledge the `notifications/initialized` notification (silently)
  - return one tool from `tools/list`
  - echo arguments back for `tools/call`
"""
import json
import sys

TOOLS = [
    {
        "name": "echo",
        "description": "Echo arguments as a single text block.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "message": {"type": "string", "description": "what to echo"}
            },
            "required": ["message"],
        },
    }
]


def reply(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def main():
    for raw in sys.stdin:
        raw = raw.strip()
        if not raw:
            continue
        msg = json.loads(raw)
        method = msg.get("method")
        if "id" not in msg:
            # notification (e.g. notifications/initialized) — drop
            continue
        if method == "initialize":
            reply({
                "jsonrpc": "2.0",
                "id": msg["id"],
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "mock", "version": "0.1"},
                },
            })
        elif method == "tools/list":
            reply({"jsonrpc": "2.0", "id": msg["id"], "result": {"tools": TOOLS}})
        elif method == "tools/call":
            args = (msg.get("params") or {}).get("arguments", {})
            text = "echo:" + json.dumps(args, separators=(",", ":"), sort_keys=True)
            reply({
                "jsonrpc": "2.0",
                "id": msg["id"],
                "result": {
                    "content": [{"type": "text", "text": text}],
                    "isError": False,
                },
            })
        else:
            reply({
                "jsonrpc": "2.0",
                "id": msg["id"],
                "error": {"code": -32601, "message": f"method not found: {method}"},
            })


if __name__ == "__main__":
    try:
        main()
    except (BrokenPipeError, KeyboardInterrupt):
        pass
