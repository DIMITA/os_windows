# Wilai - Audit Format

Status: design, pre-code.
Audience: operators auditing Wilai activity, and implementers writing
the audit subsystem. Familiarity with JSONL, ext4 attributes, and basic
log forensics is assumed.

The audit log records every tool execution Wilai performs, every mode
change, every confirmation outcome, and every provider interaction at
the metadata level. It is the operator's primary forensic surface.

## 1. Goals

- Append-only on disk, with minimal hassle. No external service.
- Plain JSONL so `jq`, `grep`, `awk`, and any pipeline tool work.
- Forward-compatible: adding fields must not break existing readers.
- Replayable: enough information per entry to reconstruct what happened
  and, where the operation was deterministic, re-run it.

## 2. Non-goals (v1)

- No cryptographic chaining or signing. The format is upgrade-ready
  (see Section 11), but v1 stays simple.
- No remote shipping. If the operator wants offsite copies, they wire
  `journalctl` or `rsync` themselves; Wilai does not ship anything.
- No structured query engine beyond the CLI's `grep` / `tail` / `show`.
  Heavy analytics belong in DuckDB or similar, fed from the JSONL.

## 3. On-disk layout

```
~/.local/share/wilai/audit/
  current.jsonl          -> symlink to today's file
  2026-05-09.jsonl       <- active, no chattr
  2026-05-08.jsonl       <- rotated, chattr +a applied
  2026-05-07.jsonl       <- rotated, chattr +a applied
  ...
```

Rules:

- One file per UTC day, named `YYYY-MM-DD.jsonl`.
- The daemon holds the current file open in `O_APPEND` mode for its
  lifetime; concurrent writers are not supported (the daemon is the
  sole writer).
- On day rollover the daemon: closes the previous file, runs
  `chattr +a` on it (best-effort), updates `current.jsonl` symlink,
  opens the new file in `O_APPEND`.
- On filesystems where `chattr +a` is not supported (zfs, btrfs without
  the right kernel, NFS, fuse), the daemon emits a single warning at
  start and continues with no append-only enforcement; this is logged
  at the start of the day's file as a `system.audit_chattr_unavailable`
  entry.

## 4. Entry schema

Every line is a single JSON object terminated by `\n`. Field order is
not significant. Schema version is carried in the `v` field.

### Common fields (every entry)

| Field        | Type    | Required | Description                                |
|--------------|---------|----------|--------------------------------------------|
| `v`          | int     | yes      | Schema version. v1 in the MVP.             |
| `ts`         | string  | yes      | RFC 3339 with millis, UTC.                 |
| `id`         | string  | yes      | ULID, monotonic per process.               |
| `kind`       | string  | yes      | Entry type (see below).                    |
| `session`    | string  | yes      | ULID of the chat session.                  |
| `mode`       | string  | yes      | `normal` or `pentest` at the time.         |

### Per-`kind` payloads

#### `kind: "tool.exec"` - a tool was run

| Field            | Type    | Description                                  |
|------------------|---------|----------------------------------------------|
| `tool.name`      | string  | Dotted name, e.g. `fs.read`.                 |
| `tool.version`   | int     | Tool YAML `version`.                         |
| `tool.category`  | string  | Category at execution.                       |
| `tool.risk`      | string  | Risk level at execution.                     |
| `args`           | object  | Per `audit.fields`; non-listed fields summarized. |
| `executor`       | string  | `subprocess` / `builtin` / `http`.           |
| `cmd`            | array?  | Rendered argv if subprocess.                 |
| `exit_code`      | int     | Process exit code (subprocess) or 0/!=0 from builtin. |
| `duration_ms`    | int     | Wall clock, integer milliseconds.            |
| `output.bytes`   | int     | Bytes captured before truncation.            |
| `output.truncated`| bool   | True if the output exceeded `max_bytes`.     |
| `output.sample`  | string? | First 1 KiB of (redacted) output, for debugging. |
| `provider`       | string  | Provider name that requested the call.       |
| `model`          | string  | Model identifier.                            |
| `turn_id`        | string  | LLM turn identifier; supports replay scoping.|
| `tool_call_id`   | string  | Provider-supplied tool-call identifier.      |

#### `kind: "tool.deny"` - a guard refused the call

| Field        | Type    | Description                                |
|--------------|---------|--------------------------------------------|
| `tool.name`  | string  | Same as above.                             |
| `args`       | object  | Same redaction policy.                     |
| `reason`     | string  | Guard's rendered `deny` message.           |
| `guard`      | string  | Stringified `if` expression that matched.  |

#### `kind: "tool.confirm"` - confirmation outcome

| Field        | Type    | Description                                |
|--------------|---------|--------------------------------------------|
| `tool.name`  | string  |                                            |
| `args`       | object  |                                            |
| `prompt`     | string  | Rendered prompt shown to the user.         |
| `answer`     | string  | `yes` / `no` / `timeout`.                  |
| `latency_ms` | int     | Time from prompt to answer.                |

If the answer was `yes`, the subsequent `tool.exec` entry follows. If
`no` or `timeout`, no execution happens and the chain ends here.

#### `kind: "mode.change"` - mode transition

| Field        | Type    | Description                                |
|--------------|---------|--------------------------------------------|
| `from`       | string  | Previous mode.                             |
| `to`         | string  | New mode.                                  |
| `trigger`    | string  | `manual` / `autodetect` / `wilrc`.         |
| `score`      | int?    | Auto-detect cumulative score (if applicable). |
| `signals`    | array?  | Names of contributing signals (if applicable). |

#### `kind: "provider.call"` - a chat completion request

| Field          | Type    | Description                              |
|----------------|---------|------------------------------------------|
| `provider`     | string  |                                          |
| `model`        | string  |                                          |
| `prompt_tokens`| int?    | If reported by the provider.             |
| `output_tokens`| int?    | Same.                                    |
| `duration_ms`  | int     |                                          |
| `n_tools`      | int     | Number of tool definitions sent.         |
| `n_messages`   | int     | Conversation length sent.                |
| `error`        | string? | Present iff the call failed.             |

The message contents themselves are not logged. The audit records that
a call happened, to whom, and how big it was, not what was said.

#### `kind: "session.start"` / `kind: "session.end"`

Bookend entries with the session id, the entry binary
(`wilai-cli` / `wilai-overlay` / `wilai-voice`), the user, and the
working directory at start.

#### `kind: "system.*"` - audit subsystem events

`system.rotate`, `system.chattr_applied`, `system.chattr_unavailable`,
`system.daemon_start`, `system.daemon_stop`, `system.config_reload`.

## 5. Append-only enforcement

For ext4 and xfs:

- After rotation, the daemon runs `chattr +a` on the rotated file. This
  prevents truncation and overwrite by any user (only root with
  `CAP_LINUX_IMMUTABLE` can clear it). In practice the user can clear
  it on their own machine; the goal is not to defeat the user, but to
  defeat malware running as the user.
- The active day file is **not** marked append-only - the daemon is
  writing to it. Tampering with the active file before rotation is
  detectable indirectly (entry id ULIDs are monotonic; out-of-order
  ids on rotation flag tampering).

For filesystems without append-only support, the daemon emits a single
warning at startup, logs `system.chattr_unavailable` as the first entry
of the day, and continues. Operators who want hard guarantees should
either run on ext4/xfs or pipe the audit log to an external append-only
store of their choosing.

`wilai audit verify` walks the audit directory and reports:

- Files missing `chattr +a` that should have it.
- Days with non-monotonic id sequences (suggests reordering).
- Days where the size on disk is smaller than the recorded byte count
  in the most recent `system.daemon_stop` entry (suggests truncation).

## 6. Rotation

Trigger: UTC midnight, or on `SIGHUP`, or when the active file exceeds
`audit.max_size_mb` (default 256). The first cause to fire wins.

Sequence:

1. Write a `system.daemon_stop`-shaped `system.rotate_pre` entry.
2. `fsync(2)` the active file.
3. Close the file descriptor.
4. Run `chattr +a` on the closed file (best-effort; warn if it fails).
5. Update the `current.jsonl` symlink atomically (`rename(2)`).
6. Open the new file in `O_APPEND | O_CREAT`.
7. Write a `system.rotate_post` entry as the first line.

If rotation fails between steps 3 and 6 (rare; disk full, perms wrong),
the daemon refuses to continue executing tools and surfaces an error
to the active session. Tools are not executed without an open audit
descriptor.

## 7. Query CLI

```
wilai audit tail [-n N] [-f]
wilai audit grep <regex> [--since <ts>] [--until <ts>] [--kind <kind>]
wilai audit show <session_id>
wilai audit replay <session_id> [--dry-run] [--from <id>] [--to <id>]
wilai audit verify
wilai audit stats [--since <ts>]   # tool counts, durations, denies
```

`grep` matches against the JSON line. For structural matches use
`jq`-style filters via `--jq <expr>`. The CLI does not implement a
query language; it shells out to `jq` if available, otherwise falls
back to substring grep.

`show <session_id>` reconstructs a session in chronological order,
suitable for piping to `less` or to a report. The output groups
`provider.call` -> [`tool.confirm`] -> `tool.exec` triples for
readability.

`replay` is intentionally limited:

- It re-runs only `tool.exec` entries whose tool is still installed at
  the same major version.
- It refuses to replay `destructive` or `pentest` categories without
  an explicit `--allow-category <cat>` flag.
- It runs each tool through the live policy engine; if a guard now
  denies what it allowed then, the replay stops and the operator is
  shown the divergence.

## 8. Privacy

The audit log is the most sensitive file Wilai writes.

- It lives under `~/.local/share/wilai/audit/` with mode `0700` on the
  directory and `0600` on each file. The daemon enforces these on each
  rotation.
- `output.sample` is capped at 1 KiB and is run through the tool's
  declared redactors before being written. Redactors are heuristic;
  operators handling sensitive data should disable `output.sample` via
  `audit.include_output_sample = false` in the global config.
- No prompt or completion text is logged. If the operator wants prompt
  capture for debugging, they enable `provider.debug_log_dir` which
  writes a separate, unrotated, **not** append-only stream that the
  operator is responsible for handling.
- Pentest mode increases verbosity (full args, full `output.sample`)
  but does not include prompt text.

## 9. Examples

```jsonl
{"v":1,"ts":"2026-05-09T14:23:11.421Z","id":"01HX...","kind":"session.start","session":"01HX...A","mode":"normal","entry":"wilai-cli","cwd":"/home/dimita/work","user":"dimita"}
{"v":1,"ts":"2026-05-09T14:23:14.005Z","id":"01HX...","kind":"provider.call","session":"01HX...A","mode":"normal","provider":"ollama","model":"mistral:7b-instruct","duration_ms":312,"n_tools":8,"n_messages":2}
{"v":1,"ts":"2026-05-09T14:23:14.211Z","id":"01HX...","kind":"tool.exec","session":"01HX...A","mode":"normal","tool":{"name":"fs.read","version":1,"category":"read","risk":"none"},"args":{"path":"/etc/hosts","max_bytes":65536},"executor":"builtin","exit_code":0,"duration_ms":3,"output":{"bytes":221,"truncated":false,"sample":"127.0.0.1 localhost\n..."},"provider":"ollama","model":"mistral:7b-instruct","turn_id":"t1","tool_call_id":"call_a"}
{"v":1,"ts":"2026-05-09T14:23:18.700Z","id":"01HX...","kind":"tool.confirm","session":"01HX...A","mode":"normal","tool":{"name":"fs.write"},"args":{"path":"/etc/hosts"},"prompt":"Path /etc/hosts is outside $HOME. Write?","answer":"no","latency_ms":1842}
{"v":1,"ts":"2026-05-09T14:25:02.118Z","id":"01HX...","kind":"mode.change","session":"01HX...A","mode":"pentest","from":"normal","to":"pentest","trigger":"autodetect","score":150,"signals":["workspace_name","binary_active"]}
```

## 10. Daily summary (optional)

Off by default. When `audit.daily_summary = true`, the daemon writes a
compact YAML summary at rotation time to
`~/.local/share/wilai/audit/summary/YYYY-MM-DD.yaml` containing tool
counts, deny counts, mode time-in-mode, and provider call totals. The
summary is derived; the JSONL remains source of truth.

## 11. Future: hash chain and signing

When v2 lands, each entry gains:

- `prev_hash`: hex SHA-256 of the previous line's bytes (excluding
  trailing newline). The first line of a file uses the previous file's
  last-line hash; the very first day uses 64 zero bytes.
- `sig`: optional Ed25519 signature over `id || ts || prev_hash`,
  produced by a key the daemon controls (or a YubiKey-backed key when
  `audit.sign = "yubikey"`).

v1 entries remain valid in a v2 world; readers ignore unknown fields.
The chain restarts at the boundary, with a `system.chain_restart` entry
recording the genesis hash. There is no in-place migration of v1 logs.
