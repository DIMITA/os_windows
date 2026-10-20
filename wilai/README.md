# Wilai

Wilai is the system agent shipped with WilOS Aurora. A user-level daemon
drives an LLM through a typed tool registry; every tool call goes
validator -> guards -> confirm -> executor -> hash-chained audit, with
optional bubblewrap isolation per call.

- Local-first: Ollama is the default provider, Anthropic is BYOK.
- Operator-controlled: `wilai mode normal | pentest` kills cloud egress
  on demand and swaps the active tool set.
- Tamper-evident: audit log is append-only JSONL with a SHA-256 hash
  chain across every entry and an optional Ed25519 signature.
- Extensible: declare an MCP server in config and its tools appear in
  the registry under `mcp.<server>.<tool>` with a supervisor that
  restarts them on crash.

## Binaries

| Binary          | What it is                                                     |
|-----------------|----------------------------------------------------------------|
| `wilai`         | CLI: chat, mode, audit, tool, mcp subcommands                  |
| `wilai-daemon`  | long-running agent on a Unix socket                            |
| `wilai-voice`   | PTT + wake-word bridge to whisper.cpp / piper                  |
| `wilai-overlay` | notify-send / zenity bridge for a Hyprland keybind             |

## Crates

```
wilai/
├── crates/
│   ├── wilai-core/        types, config, mode, paths
│   ├── wilai-audit/       hash-chained JSONL + Ed25519 signing
│   ├── wilai-tools/       YAML registry, validator, guards, exec, bwrap
│   ├── wilai-providers/   Provider trait + Ollama + Anthropic + Mock
│   ├── wilai-mcp/         MCP client + supervisor
│   ├── wilai-daemon/      Unix socket, agent loop, IPC protocol
│   ├── wilai-cli/         `wilai` binary
│   ├── wilai-voice/       `wilai-voice` binary
│   └── wilai-overlay/     `wilai-overlay` binary
├── tools/
│   ├── core/              YAML specs loaded by default
│   └── pentest/           loaded with mode_required: pentest
├── dist/                  systemd unit + PKGBUILD + MCP examples
└── docs/                  architecture / tool-format / audit-format
```

## Documents

- [`docs/architecture.md`](docs/architecture.md) - daemon layout,
  trust model, roadmap by version.
- [`docs/tool-format.md`](docs/tool-format.md) - YAML tool schema,
  category/risk matrix, guards, executors, bubblewrap sandbox.
- [`docs/audit-format.md`](docs/audit-format.md) - JSONL schema,
  hash chain, signing, replay semantics.

## Build

```sh
cd wilai
cargo build --release
# produces target/release/{wilai,wilai-daemon,wilai-voice,wilai-overlay}
```

Local test loop:

```sh
./scripts/test-wilai.sh           # from repo root; build + test
./scripts/test-wilai.sh --release # + release build sanity
```

CI (GitHub Actions) runs the same on every push touching `wilai/**`.

## Install

Arch host, system-wide:

```sh
cd wilai/dist/arch
makepkg -si
```

The ISO profile also stages the four binaries + YAML registry +
systemd user unit into airootfs when `scripts/build-iso.sh` is run on
an Arch host.

## v0.5 → v3.0 at a glance

| Phase | Highlights                                                               |
|-------|--------------------------------------------------------------------------|
| v0.5  | Rust workspace, Ollama agent, hash-chained audit, 3 builtin tools        |
| v0.6  | Confirm UX, guards, daemon split + Unix socket, rotation, Anthropic      |
| v0.7  | Voice client, overlay, systemd unit, auto-spawn                          |
| v0.8  | Pentest mode with auto-detect + cloud kill-switch + verbose audit        |
| v0.9  | Tool authoring CLI, wilbar mode pill, end-to-end tests, PKGBUILD         |
| v1.0  | Ed25519 audit signing                                                    |
| v1.1  | Integrated into the WilOS Aurora ISO profile                             |
| v2.0  | MCP client integration (JSON-RPC 2.0 over stdio)                         |
| v2.1  | Live tool list over IPC + pentest gate on MCP                            |
| v2.2  | MCP supervisor with restart-on-crash + `wilai mcp` ops subcommand        |
| v2.3  | GitHub Actions CI + MCP examples + config parse test                     |
| v2.4  | MockProvider + end-to-end agent-loop tests                               |
| v2.5  | Registry backfill after late-starting MCP servers                        |
| v2.6  | wiremock-based Ollama/Anthropic tests + MCP fail-fast                    |
| v2.7  | Live `ToolsAdded` push + `wilai audit replay` dry-run                    |
| v3.0  | Per-tool bubblewrap sandbox + `WILAI_BWRAP` override                     |
