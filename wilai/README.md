# Wilai

Wilai is the system agent shipped with WilOS Aurora. It runs as a user-level
daemon, exposes a typed tool registry to a configurable LLM provider, and
mediates every action through a category-based confirmation policy with a
local append-only audit trail.

This directory currently contains the design documents only. Code lands in
`crates/` once the design is locked.

## Documents

- [`docs/architecture.md`](docs/architecture.md) - daemon layout, components,
  trust model, roadmap.
- [`docs/tool-format.md`](docs/tool-format.md) - YAML schema for tools,
  validators, guards, executors, authoring guide.
- [`docs/audit-format.md`](docs/audit-format.md) - on-disk audit log format,
  append-only enforcement, query and replay semantics.

## Status

Design phase. Target for first code (v0.5 MVP):

- Rust workspace under `wilai/crates/`
- CLI `wilai chat` over Ollama (local) only
- 6-8 core tools (fs, hyprland, sysinfo, pkg, git)
- Audit log in `~/.local/share/wilai/audit/`
- No voice, no overlay, no pentest mode yet

See `docs/architecture.md` section "Roadmap" for the full plan.

## Non-goals

- Wilai is not a generic shell wrapper. The LLM does not generate raw shell
  commands; it fills typed tool schemas and the daemon serializes them.
- Wilai is not a cloud service. Even with a cloud provider configured (BYOK),
  the agent runs locally and cloud calls are bounded by the policy engine.
- Wilai is not a replacement for the user's judgment in security work. It is
  an auditable executor for operations the user understands.
