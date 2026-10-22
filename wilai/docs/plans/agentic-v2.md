# v2.0 — agentic Wilai

User feedback that drove this: "pourquoi on prends un cerveau s'il
va pas s'en servir". Fair point. The v1.x architecture used an LLM
with real reasoning / code-gen capability as a glorified intent
router over 50 YAML tools. Any novel request fell through the
cracks ("I don't have a tool for that") OR hallucinated a
non-existent tool.

v2.0 redirects: the LLM gets to think, write code, execute it in a
sandbox, verify the result, retry — bounded by a safety layer that
sits AROUND the model, not inside it.

## Principles

1. **The LLM reasons; the daemon enforces.** The model is free to
   plan, decompose, write scripts, and change its mind. The daemon
   is the layer that:
     - sandboxes the execution (bubblewrap)
     - pattern-denies obvious destructive commands outright
     - requires confirmation for anything with real reach
     - audits every step (plan, script, output)

2. **Safety is NOT a cage.** v1.x's cage ("you may only call these
   50 verbs") was over-conservative. The new boundary: "you may
   execute anything the sandbox allows AND the deny-pattern
   accepts; everything outside that needs an explicit confirm".

3. **Fast path for well-known patterns.** The 50 existing tool
   specs stay. The LLM uses them for everyday verbs
   (`sys.service.list`, `memory.recall`, …) because that's faster
   AND more legible than code-gen for routine ops. Code-gen is for
   what the catalogue doesn't cover.

4. **Verifiable, not opaque.** Every planned step, generated
   script, subprocess invocation, exit code, output snippet lands
   in the hash-chained audit log. Reproducible after the fact.

## Primitives

Four new tool-like primitives, exposed to the LLM:

### `think(reason: string, plan: string[])`
Chain-of-thought + plan declaration. Pure logging — no side
effects. Audit log records `reason` + `plan` as a `PlanDeclared`
entry. UI can show a muted "planning…" state. The LLM is prompted
to always `think` before any `script.run` so the audit log carries
intent.

### `script.run(interpreter: enum, source: string, timeout_s: int)`
Sandboxed script execution.
- `interpreter`: `bash` | `sh` | `python3` | `node` (first two
  shipped at launch; python3 if present).
- `source`: the script body.
- `timeout_s`: 1..=120, default 30.
- Sandbox: bubblewrap with:
    - `--ro-bind / /` (whole fs read-only)
    - `--tmpfs /tmp` (fresh writable tmpfs per run)
    - `--unshare-net` (no network)
    - `--unshare-pid`, `--unshare-ipc`, `--unshare-uts`
    - `--die-with-parent`
    - Memory + CPU cgroup caps (configurable; defaults 256 MB,
      1 CPU core)
- Output: stdout + stderr merged, capped at 256 KiB, truncated
  with `…(+N bytes)` suffix.
- Risk: `low`. No confirm needed — the sandbox is the safety.
- Deny patterns applied to `source` BEFORE execution (see
  "deny-pattern" below).

### `script.run.privileged(interpreter, source, timeout_s)`
Same shape as `script.run` but with the sandbox relaxed:
- Writable `/tmp` AND user `$HOME`
- Network allowed
- Can call `sudo` (if the daemon user is in sudoers)
- Risk: `critical`. Always confirms, default NO.
- Forbidden in pentest mode (same as the current cloud providers).
- Deny patterns still apply.
- Config knob `[security] allow_privileged_script = false` by
  default. User must set true to make this primitive callable at
  all.

### `tool.call(name, args)` — kept
Fast path for the 50 YAML specs. Identical to v1.x. The LLM
picks it over `script.run` whenever a catalogued verb matches, so
the trip through the audit log for a mundane `sys.service.list` is
short + legible.

## Deny-pattern

Before any `script.run{,.privileged}` dispatches to bubblewrap, the
source is scanned for a hardcoded list of regex patterns. A match
produces an immediate `ToolDeny` audit entry and a user-visible
"refused: pattern <id>" message. No confirm bypass.

Opening patterns (will grow with usage):
- `rm\s+-rf\s+/\s*$` (whole root)
- `rm\s+-rf\s+/\*`
- `rm\s+-rf\s+(/etc|/boot|/usr|/var/lib|/home)` (critical dirs)
- `dd\s+.*of=/dev/(sd|hd|nvme|mmc|vd)` (overwriting block devs)
- `mkfs\.[a-z0-9]+\s+/dev/(sd|hd|nvme|mmc|vd)`
- `:\(\)\{\s*:\|:&\s*\};:` (fork bomb)
- `(curl|wget)\s+[^|]*\|\s*(bash|sh|zsh)` (pipe-to-shell)
- `>\s*/dev/(sda|nvme|hd|mmc)` (direct device write)
- `chmod\s+-R\s+777\s+/`
- `userdel\s+-r\s+root`
- `shutdown|reboot|halt|poweroff` (unless script.run.privileged
  with explicit confirm)
- `iptables\s+-F` and `nft\s+flush` (clearing firewall)

Patterns live in a shipped `tools/deny-patterns.yaml` so operators
can audit + extend. Changes trigger a daemon reload (same path as
tool catalogue).

## Agent loop (replaces v1.x's one-shot)

```
history = [system_prompt, user_prompt]
for turn in 1..MAX_AGENT_TURNS:  # 8
    response = llm.chat_streaming(history, provider, model)

    if response.tool_calls.is_empty():
        emit Text / TextChunk, append to history, return.

    for call in response.tool_calls:
        if call.name == "think":
            audit PlanDeclared
            history.append(ToolResult { call.id, "noted" })
            continue
        result = execute(call)  # with sandbox + patterns + confirm
        audit ToolCall / ToolResult (as today)
        history.append(ToolResult { call.id, result.serialize() })

    # Self-verify (new):
    if response.indicates_final_answer():
        verification = llm.verify(user_prompt, history)
        if verification.ok:
            emit Text (verification.summary)
            return
        else:
            history.append(SystemHint { "verification failed: …" })
            # continue the loop with the failure context
```

The verify step is a tiny LLM call: "Given the user's original ask
and the actions you took, is the task actually done? Reply JSON
`{ok: bool, reason: string, next: ?string}`." If `ok=false`, the
loop runs another iteration with the model's own stated reason.

MAX_AGENT_TURNS caps infinite loops; existing MAX_TOOL_TURNS (8)
is reused.

## Slice plan

| # | Scope | Days |
|---|---|---|
| 1 | `script.run` tool + bubblewrap sandbox + deny-pattern scan + audit | 2 |
| 2 | Agent-loop rewrite with multi-turn + self-verify | 2 |
| 3 | `think` tool + audit of reasoning | 1 |
| 4 | `script.run.privileged` + confirm plumbing + config knob | 1 |
| 5 | System-prompt redesign: how+when to pick fast path vs code-gen vs script.run.privileged | 1 |
| 6 | Tests (deny patterns, sandbox escape attempts, multi-turn) + migration doc | 2 |

**Total ~9 days.**

Milestone cutover: once slice 6 is green, bump to `2.0.0` and tag.
Users on 1.x upgrade via the same `curl … | sudo sh`. The config
needs manual edit only to opt into `script.run.privileged`.

## Non-goals (explicitly)

- **No LLM self-training / plugin install** by the agent. Model
  stays static, catalogue grows only via files on disk.
- **No persistent `shell.exec`**. Everything is per-call sandboxed;
  no persistent shell state carries between tool calls.
- **No fetching code from the internet** in `script.run`
  (`--unshare-net`). `script.run.privileged` can, with confirm.
- **No code-generation of new YAML tool specs**. Keep the
  catalogue + the code-gen primitive as two orthogonal escape
  hatches; synthesising specs on the fly is a different, bigger
  design — defer.
