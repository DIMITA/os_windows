# Wilai - Architecture

Status: design, pre-code.
Audience: implementers and operators familiar with Linux systems and LLM
tool calling. No tutorials.

## 1. Goals

- A local agent that turns natural-language intent into typed, audited
  operations on the user's machine.
- The LLM never produces raw shell commands. It fills typed schemas; the
  daemon serializes them to subprocess invocations or in-process handlers.
- The same binary serves desktop use, dev work, and pentest sessions, with
  a mode bascule that gates capabilities and providers.
- Bring-your-own-key for cloud providers; ships with Ollama as default so
  the system is fully functional offline and on day one.
- Append-only audit log for every executed operation, queryable from CLI.

## 2. Non-goals

- No cloud service, no telemetry, no remote control.
- No code-generation IDE replacement; Wilai is a system agent, not Copilot.
- No attempt to sandbox the user. Wilai protects the user from accidents
  and from LLM hallucinations, not from themselves.
- No support for arbitrary plugin code. Tools are declarative YAML plus
  vetted executors. No `eval`, no embedded scripting in v1.

## 3. High-level layout

```
+------------------------------------------------------------------+
|  Clients                                                         |
|  - wilai CLI (chat, audit, mode, tool)                           |
|  - wilai-overlay (Hyprland-attached UI, post v0.7)               |
|  - wilai-voice  (wake-word + STT + TTS, post v0.7)               |
+----------------------------+-------------------------------------+
                             |  Unix socket (newline-delimited JSON)
+----------------------------v-------------------------------------+
|  wilai-daemon (Rust, user systemd unit)                          |
|                                                                  |
|  +-------------------+    +----------------------------------+   |
|  |  ipc              |    |  policy                          |   |
|  |  - socket server  |    |  - mode (normal | pentest)       |   |
|  |  - session mgr    |    |  - auto-detect signals           |   |
|  +---------+---------+    |  - confirm-required matrix       |   |
|            |              +----------------+-----------------+   |
|            v                               |                     |
|  +-------------------+                     v                     |
|  |  agent loop       |    +----------------+-----------------+   |
|  |  - prompt build   |    |  tools                           |   |
|  |  - provider call  |--->|  - YAML registry loader          |   |
|  |  - tool dispatch  |    |  - schema validator              |   |
|  |  - retry/repair   |    |  - executor (subprocess/builtin) |   |
|  +---------+---------+    +----------------+-----------------+   |
|            |                               |                     |
|            v                               v                     |
|  +-------------------+    +----------------------------------+   |
|  |  providers        |    |  audit                           |   |
|  |  - trait Provider |    |  - jsonl writer                  |   |
|  |  - ollama (v0.5)  |    |  - daily rotate + chattr +a      |   |
|  |  - anthropic v0.6 |    |  - query / replay                |   |
|  |  - openai, gemini |    +----------------------------------+   |
|  |  - mistral cloud  |                                           |
|  +-------------------+                                           |
+------------------------------------------------------------------+
```

## 4. Crates

```
wilai/
  Cargo.toml                  # workspace
  crates/
    wilai-core/               # types, errors, config, mode, schema defs
    wilai-tools/              # registry loader + validator + executor
    wilai-providers/          # Provider trait + ollama / anthropic / ...
    wilai-audit/              # jsonl writer, rotation, query
    wilai-policy/             # confirm matrix, mode auto-detect
    wilai-daemon/             # binary: daemon, ipc, agent loop
    wilai-cli/                # binary: `wilai` (chat, audit, mode, tool)
```

Each crate has its own README and unit tests. The `daemon` and `cli`
crates are the only binaries; everything else is library code so
clients written later (overlay, voice) can link directly.

## 5. Daemon vs CLI

The daemon is a long-running user process started by a systemd user unit
(`wilai.service`). It owns:

- The Unix socket at `$XDG_RUNTIME_DIR/wilai.sock`.
- The session multiplexer (see below).
- The agent loops and provider connections.
- The audit writer (the daemon is the sole writer for the audit log;
  see Section 10 and `audit-format.md` for the hash-chained format).
- The policy engine (single source of truth for the active mode).

The CLI is short-lived. It opens the socket, sends a request, streams
the response, and exits. It contains no LLM logic. This split keeps the
attack surface small: only one process talks to providers and writes to
the audit log.

For the v0.5 MVP we may temporarily fold the daemon into the CLI to
shorten the iteration loop, behind a `--in-process` flag, but the IPC
boundary is the design target.

### Multi-session concurrency

A single daemon serves any number of concurrent sessions. Each client
connection on the Unix socket opens one or more sessions; sessions
from different clients are fully independent except for two shared
resources:

- **The policy engine.** Mode is global; a mode change in any session
  affects all sessions. This is intentional - the operator must not
  be able to mix `normal` and `pentest` work in parallel against the
  same provider context.
- **The audit log.** All sessions write to the same daily file via the
  daemon's single writer. Writes are serialized through an internal
  mpsc channel so the hash chain stays linear and deterministic.

Each session carries its own conversation state, its own `turn_id`
counter, and its own selected provider/model (subject to mode
restrictions). The session id (`ULID`) is recorded on every audit
entry so a session can be reconstructed end-to-end after the fact.

Concurrency model inside the daemon:

```
clients (N)  -- ipc -->  session tasks (N)  -- mpsc -->  audit writer (1)
                                              \
                                               -- pool -->  provider clients
                                              /
                                              -- registry -->  tool executors
```

Provider clients are pooled per provider; tool executors are spawned
per call (subprocess) or run in a Tokio task (builtin). The audit
writer is a single task that owns the current-day file descriptor and
the running hash; this is what guarantees the chain.

Multi-profile support (separate daemons with separate audit chains for
e.g. personal vs client work) is planned for v0.7+ via
`wilai --profile <name>` selecting between
`wilai-<name>.sock` sockets and per-profile audit directories. Not in
v0.5.

## 6. Provider abstraction

```rust
trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn capabilities(&self) -> Capabilities;          // tool calling, streaming, vision
    async fn chat(&self, req: ChatRequest) -> Result<ChatStream>;
}

struct ChatRequest {
    model: String,
    system: String,
    messages: Vec<Message>,
    tools: Vec<ToolSchema>,                          // OpenAI-shaped
    max_tokens: u32,
    stream: bool,
}
```

All providers normalize to OpenAI-shaped tool calling on the request side
and emit a unified `ChatEvent` enum on the response side
(`Token`, `ToolCall`, `Done`, `Error`). Provider-specific quirks
(Anthropic's content blocks, Ollama's stream of dicts) are hidden.

The selection rule is:

1. The active mode dictates an allowed-providers set. In `normal`, all
   configured providers are allowed. In `pentest`, only providers tagged
   `local: true` are allowed.
2. Within the allowed set, the user's `default_provider` config wins.
3. Per-conversation override via `wilai chat --provider <name>` is
   subject to the mode's allowed set; if the override is disallowed, the
   CLI errors out before the request leaves the host.

The router is implemented in `wilai-providers::Router`. Adding a
provider is a trait impl plus a config entry; no daemon changes.

## 7. Tool execution pipeline

```
LLM -> tool_call(name, args_json)
  -> registry.lookup(name)            # 404 -> repair turn (max 2)
  -> validator.check(schema, args)    # invalid -> repair turn
  -> policy.guards(tool, args)        # may inject confirm step
  -> mode.check(tool.category)        # disallowed -> hard fail
  -> executor.run(tool, args)         # subprocess or builtin
  -> audit.write(entry)
  -> ChatEvent::ToolResult(...)
  -> back into the loop
```

Key invariants:

- **Args never reach a shell as a string.** Subprocess executors pass
  args via `argv` exclusively. Templating in YAML
  (`cmd: ["nmap", "-p", "{ports}", "{target}"]`) substitutes per-arg,
  and the substitution is rejected if the resulting argv contains
  newlines or unrendered placeholders.
- **Repair turns are bounded.** If the LLM returns a tool call we cannot
  validate, we send back the validator error and ask once more. Two
  failures in a row terminate the turn with an error to the user.
- **Confirms are synchronous.** When a guard requires confirmation, the
  daemon emits a `ConfirmRequired` event over the socket and blocks the
  agent loop until the client answers `yes` / `no` / `timeout`. No
  confirm = no execution.

## 8. Mode model

Two modes only: `normal` and `pentest`. No third mode.

| Aspect                     | normal                | pentest                       |
|----------------------------|-----------------------|-------------------------------|
| Allowed providers          | all configured        | only `local: true`            |
| Long-term memory writes    | enabled               | disabled (ephemeral session)  |
| Tool category `pentest`    | hidden                | enabled                       |
| Tool category `keyring`    | enabled               | disabled                      |
| Audit verbosity            | standard              | verbose (full args, full out) |
| Visual indicator           | none                  | red badge in wilbar           |
| Workspace association      | any                   | tagged workspace expected     |
| Egress (DNS, HTTP)         | per-tool policy       | only to user-declared targets |

Switching modes is always explicit on the way out (`wilai mode normal`)
and may be implicit on the way in via auto-detect (see below).
Switching never happens while a tool of category `pentest` is running.

## 9. Pentest auto-detection

A small set of independent signals, each emitting a score. The mode
flips to `pentest` when the cumulative score exceeds a threshold and
the user has not opted out for the current session.

Signals (initial proposal, tunable):

| Signal                                                        | Score |
|---------------------------------------------------------------|-------|
| `.wilrc` in cwd with `mode: pentest`                          |  100  |
| Hyprland workspace name matches `pentest|redteam|client-*`    |   60  |
| Active window class matches `Burp\|Wireshark\|msfconsole\|…`  |   50  |
| Process running by user matches pentest binary list           |   40  |
| VPN connection up to an endpoint tagged `client`              |   40  |
| Container or VM tagged `pentest:*`                            |   60  |
| User explicitly invoked `wilai mode pentest`                  |  200  |

Threshold: 100. Below threshold, no change. Above threshold, the daemon
emits `ModeChangePending`, the wilbar shows a 3-second countdown with
`Super+Esc` to abort, then commits.

Exits are explicit only:

- `wilai mode normal` from CLI.
- `Super+Shift+P` global keybinding (configurable).
- Exit is denied while a tool of category `pentest` is in flight; the
  daemon answers `BUSY` and the user must wait for completion.

The signal list, scores, and threshold live in `wilai.toml` so the user
can tune them without recompiling.

## 10. Audit

See [`audit-format.md`](audit-format.md) for the full format.

Summary:

- One JSONL file per day under `~/.local/share/wilai/audit/`.
- The daemon holds the file open in `O_APPEND` mode; on day rollover it
  closes the previous file and runs `chattr +a` on it before opening the
  new one. Append-only is best-effort on filesystems that support it
  (ext4, xfs); a warning is logged on filesystems that do not.
- Schema is versioned (`v` field). Upgrades append a new version; old
  entries remain readable.
- **Hash-chained from v1.** Every entry carries `prev_hash` (SHA-256 of
  the previous entry's bytes) and `seq` (monotonic per file). The
  chain spans rotations: the first entry of day N+1 hashes the last
  entry of day N. Tampering or reordering is detectable with
  `wilai audit verify`.
- **Ed25519 signing** in v1.0 (software key under
  `~/.local/share/wilai/keys/audit.ed25519`). Opt-in via
  `wilai audit keygen`; daemon picks it up at start. Each entry's
  `sig` is over the canonical unsigned form ending in `}`; tamper
  detection is layered with the hash chain.

CLI:

```
wilai audit tail [-n N] [-f]
wilai audit grep <regex> [--since <ts>] [--until <ts>]
wilai audit show <session_id>
wilai audit replay <session_id> [--dry-run]
wilai audit verify              # checks chattr +a is set on rotated files
```

## 11. Configuration

Single file: `~/.config/wilai/wilai.toml`.

```toml
[general]
default_provider = "ollama"
default_model    = "mistral:7b-instruct"
confirm_timeout_s = 30

[providers.ollama]
type = "ollama"
url  = "http://127.0.0.1:11434"
local = true

[providers.claude]
type   = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
local  = false

[mode]
auto_detect = true
threshold   = 100
exit_on_workspace_change = false

[tools]
extra_dirs = ["~/.config/wilai/tools"]
disabled   = []                       # tool names to force-disable

[audit]
dir = "~/.local/share/wilai/audit"
chattr_append = true
```

API keys are pulled from environment at start; never stored in the file.
Validation runs at daemon start and on `SIGHUP`; invalid config refuses
to start with a precise error.

## 12. Trust model

Threats considered:

| Threat                                        | Mitigation                                  |
|-----------------------------------------------|---------------------------------------------|
| LLM hallucinates a destructive command        | Tools are typed; no shell strings produced  |
| LLM mis-fills args (wrong path, wrong flag)   | Schema validator + per-tool guards          |
| Compromised provider replays old tool calls   | Each turn carries a fresh nonce; daemon refuses re-execution of the same `(turn_id, tool_call_id)` |
| Local malware tampers with audit log          | Hash chain over every entry; `chattr +a` on rotated files; `wilai audit verify` detects break, reorder, truncate |
| Unintended cloud egress in pentest mode       | Mode guard rejects non-local providers; CLI override is filtered before egress |
| Voice wake-word records ambient conversation  | Wake-word engine runs locally; no audio leaves the host until the user-bound action begins; visible state in wilbar |

Threats explicitly out of scope:

- Root-level compromise of the user's machine. Wilai assumes the kernel
  and the user account are intact. If root is owned, the audit log is
  not a defence.
- Side-channel inference of user secrets through the LLM context window.
  Users are responsible for what they paste; pentest mode reduces but
  does not eliminate exposure.

## 13. Voice (deferred)

Not in v0.5. Design target for v0.7:

- `openwakeword` for "Hey Wil" wake detection, fully local.
- `whisper.cpp` `small-en` (or multilingual `small`) for STT, local.
- `piper` for TTS, local.
- A separate binary `wilai-voice` connects to the daemon's socket like
  any other client. Wilai daemon never handles audio.
- Visual state in wilbar at all times: `idle`, `listening`, `thinking`,
  `speaking`. Mute toggle is a global keybinding.

## 14. Roadmap

| Version | Scope                                                                                |
|---------|--------------------------------------------------------------------------------------|
| v0.5    | Rust workspace, CLI `wilai chat`, Ollama provider, 6-8 core tools, audit, no daemon  |
| v0.6    | Daemon split + Unix socket, Anthropic provider, BYOK config, repair turns hardened   |
| v0.7    | Voice (`wilai-voice`), wilbar pill, OpenAI + Gemini + Mistral cloud providers        |
| v0.8    | Pentest mode + auto-detect, pentest tool category, mode-aware audit verbosity        |
| v0.9    | Overlay (`wilai-overlay`) attached to Hyprland, confirm toasts, tool authoring CLI   |
| v1.0    | Docs, packaging into the Aurora ISO, end-to-end tests, signed releases               |

Each version is independently shippable and has its own acceptance
criteria documented in the corresponding milestone issue.
