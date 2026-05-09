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
- Tamper-evident: a hash chain over every entry, spanning rotations.
- Forward-compatible: adding fields must not break existing readers.
- Replayable: enough information per entry to reconstruct what happened
  and, where the operation was deterministic, re-run it.

## 2. Non-goals (v1)

- No remote shipping. If the operator wants offsite copies, they wire
  `journalctl` or `rsync` themselves; Wilai does not ship anything.
- No structured query engine beyond the CLI's `grep` / `tail` / `show`.
  Heavy analytics belong in DuckDB or similar, fed from the JSONL.
- No hardware-backed signing in v1.0. The format and codepath are
  ready (the `sig` field is produced by an Ed25519 software key) but
  the key lives in `~/.local/share/wilai/keys/audit.ed25519` and is
  protected by filesystem permissions only. A YubiKey/PIV variant
  reuses the same `sig` field at the wire level.

## 3. On-disk layout

```
~/.local/share/wilai/audit/
  current.jsonl          -> symlink to today's file
  chain.head             <- last hash + last seq + last file (atomic update)
  2026-05-09.jsonl       <- active, no chattr
  2026-05-08.jsonl       <- rotated, chattr +a applied
  2026-05-07.jsonl       <- rotated, chattr +a applied
  ...
```

Rules:

- One file per UTC day, named `YYYY-MM-DD.jsonl`.
- The daemon holds the current file open in `O_APPEND` mode for its
  lifetime and is the sole writer for the audit log; concurrent
  sessions write through an internal mpsc channel that the audit
  writer task drains in order.
- `chain.head` is a small file (~200 bytes) that the daemon updates
  atomically (write to `chain.head.tmp`, fsync, rename) after every
  entry. It carries the running hash so the daemon can recover the
  chain after a clean restart without re-reading the day's file. After
  an unclean shutdown the daemon re-reads the tail of `current.jsonl`
  and rebuilds the running hash from there; `chain.head` is treated
  as a hint, not a source of truth.
- On day rollover the daemon: closes the previous file, runs
  `chattr +a` on it (best-effort), updates `current.jsonl` symlink,
  opens the new file in `O_APPEND`, writes a `system.rotate_post`
  entry whose `prev_hash` chains back to the last entry of the
  previous file.
- On filesystems where `chattr +a` is not supported (zfs, btrfs without
  the right kernel, NFS, fuse), the daemon emits a single warning at
  start and continues with no append-only enforcement; this is logged
  at the start of the day's file as a `system.audit_chattr_unavailable`
  entry. The hash chain is unaffected and remains the primary
  tamper-evidence mechanism.

## 4. Entry schema

Every line is a single JSON object terminated by `\n`. Field order is
not significant for the hash (we hash the bytes as written, see
Section 5). Schema version is carried in the `v` field.

### Common fields (every entry)

| Field        | Type    | Required | Description                                |
|--------------|---------|----------|--------------------------------------------|
| `v`          | int     | yes      | Schema version. v1 in the MVP.             |
| `seq`        | int     | yes      | Monotonic per-file sequence, starts at 0.  |
| `ts`         | string  | yes      | RFC 3339 with millis, UTC.                 |
| `id`         | string  | yes      | ULID, monotonic per process.               |
| `prev_hash`  | string  | yes      | Hex SHA-256 of the previous entry's bytes. |
| `kind`       | string  | yes      | Entry type (see below).                    |
| `session`    | string  | yes      | ULID of the chat session.                  |
| `mode`       | string  | yes      | `normal` or `pentest` at the time.         |
| `sig`        | string? | no       | Hex Ed25519 signature over the unsigned entry bytes. Absent when no key is configured. |

`prev_hash` and `seq` are the tamper-evidence pair. Verifying one
without the other catches different attacks: `prev_hash` catches
content tampering and reordering, `seq` catches deletions of
contiguous entries that would otherwise leave a valid chain on the
remaining entries. They are cheap to maintain and add ~80 bytes per
entry.

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
working directory at start. Multiple sessions can be live in parallel;
their `start` and `end` entries interleave with other sessions' entries
in the same file, and the `session` field on every entry is what binds
them together at query time.

#### `kind: "system.*"` - audit subsystem events

`system.daemon_start`, `system.daemon_stop`, `system.config_reload`,
`system.rotate_pre`, `system.rotate_post`, `system.chattr_applied`,
`system.chattr_unavailable`, `system.chain_genesis`,
`system.chain_resume`.

## 5. Hash chain

### 5.1. What is hashed

For each entry, `prev_hash` is the lowercase hex SHA-256 of the **exact
bytes of the previous entry's serialized JSON line, excluding the
trailing `\n`**. We hash the bytes as written, not a canonical form.
This avoids any dependency on JSON canonicalization rules and makes
verification a stream operation.

The first entry of any file (whether after rotation or after first
install) chains to the last entry of the previous file. Genesis
(very first line ever written by Wilai on this host) uses
`prev_hash = "00..00"` (64 zeros).

### 5.2. Building an entry

Pseudocode for the audit writer:

```
fn write_entry(payload):
    seq      = next_seq_for_current_file()
    prev_hash = running_hash                  # 64 hex chars
    line_obj = { v:1, seq, ts, id, prev_hash, ...payload, sig: <maybe> }
    line_bytes = serialize(line_obj)          # no trailing newline
    file.write(line_bytes); file.write("\n"); file.fdatasync()

    running_hash = sha256_hex(line_bytes)
    chain_head.atomically_update({running_hash, seq, file: current_path})
```

Concurrency: the audit writer is a single task. All sessions submit
entries through an mpsc channel. The writer drains them in order, so
the chain is well-defined even with N concurrent sessions producing
entries simultaneously.

### 5.3. Genesis and resume

- **Genesis** (first ever boot, no audit dir present):
  the writer creates the dir, writes a `system.chain_genesis` entry
  with `prev_hash = "00..00"`, then proceeds.
- **Clean shutdown / restart**: writer reads `chain.head`, validates
  it against the last line of `current.jsonl` (if present), and
  resumes. Mismatch downgrades to the unclean path.
- **Unclean shutdown / restart**: writer reads the tail of
  `current.jsonl` to find the last well-formed line, recomputes its
  SHA-256, sets `running_hash` to that, writes a
  `system.chain_resume` entry that explicitly carries the recovered
  hash, then proceeds.

### 5.4. Cross-file chain

At rotation:

1. Last entry of file N is `system.rotate_pre`. Its hash becomes the
   `prev_hash` of the first entry of file N+1.
2. File N is closed, fsync'd, `chattr +a` applied.
3. File N+1 is opened. First entry is `system.rotate_post` with
   `seq = 0` and `prev_hash` = hash of `system.rotate_pre`.
4. `chain.head` is updated to point at `system.rotate_post`.

If the daemon crashes between step 2 and step 3, recovery on next
start finds the closed file N with its last entry `system.rotate_pre`,
opens (or creates) file N+1, and writes `system.rotate_post` chained
to N. The chain survives.

### 5.5. Verification

`wilai audit verify` walks the entire audit directory in chronological
order. For each entry:

- Recomputes SHA-256 of the previous entry's bytes and compares to
  this entry's `prev_hash`. Mismatch is a hard fail with the file,
  byte offset, and entry id.
- Checks `seq` is strictly monotone within a file, starting at 0.
- Checks the last entry of file N's hash equals the first entry of
  file N+1's `prev_hash`.
- Checks `chattr +a` is set on rotated files (warning, not failure).

Output is human-readable by default; `--json` emits a structured
report for downstream tooling.

Performance: SHA-256 over a typical entry (~300-1500 bytes) costs a
few microseconds on any modern x86. Verifying a year of audit logs
(tens of millions of entries) runs in seconds.

### 5.6. What the chain does and does not protect

**Detects**:
- In-place modification of any entry (changes its bytes, breaks the
  next entry's `prev_hash`).
- Insertion of an entry between two existing ones.
- Deletion of trailing entries from a file (breaks the cross-file
  chain when the next file is processed).
- Deletion of a contiguous block in the middle (breaks the chain at
  the deletion boundary).
- Reordering of entries (breaks the chain).

**Does not detect on its own**:
- Wholesale deletion of the entire most-recent file before any
  cross-file linking has happened. Mitigation: `chain.head` has the
  expected first hash of the next file; verification reports that
  the expected continuation is missing.
- Modification by an attacker who controls the daemon (then they
  also control the running hash). Mitigation belongs to a future
  signing scheme with a hardware-held key.
- Truncation of the very first entry on first install. Mitigation:
  `system.chain_genesis` is logged with a known shape; readers can
  detect its absence.

## 6. Append-only enforcement

For ext4 and xfs:

- After rotation, the daemon runs `chattr +a` on the rotated file. This
  prevents truncation and overwrite by any user (only root with
  `CAP_LINUX_IMMUTABLE` can clear it). In practice the user can clear
  it on their own machine; the goal is not to defeat the user, but to
  defeat malware running as the user.
- The active day file is **not** marked append-only - the daemon is
  writing to it. Tampering with the active file before rotation is
  detectable through the hash chain and through `seq` gaps.

For filesystems without append-only support, the daemon emits a single
warning at startup, logs `system.chattr_unavailable` as the first
entry of the day, and continues. The hash chain remains the primary
tamper-evidence mechanism even in that case.

`wilai audit verify` walks the audit directory and reports:

- Hash chain breaks (Section 5.5).
- Files missing `chattr +a` that should have it.
- Days with non-monotonic `seq` sequences.
- Days where the size on disk is smaller than the recorded byte count
  in the most recent `system.daemon_stop` entry (suggests truncation).

## 7. Rotation

Trigger: UTC midnight, or on `SIGHUP`, or when the active file exceeds
`audit.max_size_mb` (default 256). The first cause to fire wins.

Sequence (chained-aware, see Section 5.4):

1. Write a `system.rotate_pre` entry as the last line of the active
   file. This entry is a normal entry with its own `prev_hash`.
2. `fsync(2)` the active file.
3. Close the file descriptor.
4. Run `chattr +a` on the closed file (best-effort; warn if it fails).
5. Update the `current.jsonl` symlink atomically (`rename(2)`).
6. Open the new file in `O_APPEND | O_CREAT`.
7. Write a `system.rotate_post` entry as the first line, with
   `seq = 0` and `prev_hash` set to the SHA-256 of `rotate_pre`.
8. Atomically update `chain.head`.

If rotation fails between steps 3 and 7 (rare; disk full, perms wrong),
the daemon refuses to continue executing tools and surfaces an error
to all active sessions. Tools are not executed without an open audit
descriptor and a valid hash chain head.

## 8. Query CLI

```
wilai audit tail [-n N] [-f]
wilai audit grep <regex> [--since <ts>] [--until <ts>] [--kind <kind>]
wilai audit show <session_id>
wilai audit replay <session_id> [--dry-run] [--from <id>] [--to <id>]
wilai audit verify [--json] [--since <date>]
wilai audit stats [--since <ts>]   # tool counts, durations, denies
```

`grep` matches against the JSON line. For structural matches use
`jq`-style filters via `--jq <expr>`. The CLI does not implement a
query language; it shells out to `jq` if available, otherwise falls
back to substring grep.

`show <session_id>` reconstructs a session in chronological order,
suitable for piping to `less` or to a report. The output groups
`provider.call` -> [`tool.confirm`] -> `tool.exec` triples for
readability. Multiple sessions with overlapping timelines are
isolated by the `session` field.

`replay` is intentionally limited:

- It re-runs only `tool.exec` entries whose tool is still installed at
  the same major version.
- It refuses to replay `destructive` or `pentest` categories without
  an explicit `--allow-category <cat>` flag.
- It runs each tool through the live policy engine; if a guard now
  denies what it allowed then, the replay stops and the operator is
  shown the divergence.
- Replay does not modify the audit log of the original session; new
  entries land in the current day's file with a fresh session id and
  a `replay_of` field pointing at the original.

## 9. Privacy

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

## 10. Examples

```jsonl
{"v":1,"seq":0,"ts":"2026-05-09T00:00:00.001Z","id":"01HX...","prev_hash":"a3f1...","kind":"system.rotate_post","session":"-","mode":"normal","prev_file":"2026-05-08.jsonl"}
{"v":1,"seq":1,"ts":"2026-05-09T14:23:11.421Z","id":"01HX...","prev_hash":"7e22...","kind":"session.start","session":"01HX...A","mode":"normal","entry":"wilai-cli","cwd":"/home/dimita/work","user":"dimita"}
{"v":1,"seq":2,"ts":"2026-05-09T14:23:14.005Z","id":"01HX...","prev_hash":"4b9c...","kind":"provider.call","session":"01HX...A","mode":"normal","provider":"ollama","model":"mistral:7b-instruct","duration_ms":312,"n_tools":8,"n_messages":2}
{"v":1,"seq":3,"ts":"2026-05-09T14:23:14.090Z","id":"01HX...","prev_hash":"d106...","kind":"session.start","session":"01HX...B","mode":"normal","entry":"wilai-cli","cwd":"/home/dimita/blog","user":"dimita"}
{"v":1,"seq":4,"ts":"2026-05-09T14:23:14.211Z","id":"01HX...","prev_hash":"f810...","kind":"tool.exec","session":"01HX...A","mode":"normal","tool":{"name":"fs.read","version":1,"category":"read","risk":"none"},"args":{"path":"/etc/hosts","max_bytes":65536},"executor":"builtin","exit_code":0,"duration_ms":3,"output":{"bytes":221,"truncated":false,"sample":"127.0.0.1 localhost\n..."},"provider":"ollama","model":"mistral:7b-instruct","turn_id":"t1","tool_call_id":"call_a"}
{"v":1,"seq":5,"ts":"2026-05-09T14:23:18.700Z","id":"01HX...","prev_hash":"2cd4...","kind":"tool.confirm","session":"01HX...B","mode":"normal","tool":{"name":"fs.write"},"args":{"path":"/etc/hosts"},"prompt":"Path /etc/hosts is outside $HOME. Write?","answer":"no","latency_ms":1842}
{"v":1,"seq":6,"ts":"2026-05-09T14:25:02.118Z","id":"01HX...","prev_hash":"9b71...","kind":"mode.change","session":"01HX...A","mode":"pentest","from":"normal","to":"pentest","trigger":"autodetect","score":150,"signals":["workspace_name","binary_active"]}
```

Note that sessions `01HX...A` and `01HX...B` interleave their entries
in the same file; the chain (`seq` and `prev_hash`) remains linear.
The mode change at `seq:6` affects both sessions even though it was
triggered from session A's context.

## 11. Daily summary (optional)

Off by default. When `audit.daily_summary = true`, the daemon writes a
compact YAML summary at rotation time to
`~/.local/share/wilai/audit/summary/YYYY-MM-DD.yaml` containing tool
counts, deny counts, mode time-in-mode, provider call totals, and the
first/last `prev_hash` of the day for quick external pinning. The
summary is derived; the JSONL remains source of truth.

## 12. Signing (v1.0, software key)

Each entry can carry an Ed25519 signature in the `sig` field. Signing
is opt-in; absence of a key produces unsigned entries and the chain
remains valid (verify reports `unsigned` counts but does not fail
unless `--require-sig` is passed).

### 12.1. Key generation

```
wilai audit keygen
```

Creates `~/.local/share/wilai/keys/audit.ed25519` (32-byte raw secret,
mode `0600`) and `audit.ed25519.pub` (32-byte raw public, mode `0644`).
The CLI prints the public key in hex and an 8-byte fingerprint
(`sha256(pub)[..8]`) for offsite pinning.

### 12.2. What is signed

The signature covers the entry's bytes BEFORE the `,"sig":"<hex>"`
suffix is appended. Concretely:

- The writer builds the JSON object without a `sig` key, serializes
  it (call this `unsigned_bytes`, ending with `}`).
- It signs `unsigned_bytes` with Ed25519.
- It removes the trailing `}`, appends `,"sig":"<hex>"}`, and writes
  the resulting bytes to disk.
- The entry's `prev_hash` for the NEXT line is computed over the
  signed bytes (so a tamper that changes the sig also breaks the
  chain).

This avoids JSON canonicalization headaches: the verifier slices the
trailing `,"sig":"..."}` off the line, restores `}`, and gets the
exact bytes that were signed.

### 12.3. Verification

```
wilai audit verify              # warns on missing sigs
wilai audit verify --require-sig  # treats missing sigs as errors
```

Verify loads `audit.ed25519.pub` from the default path. For each
signed entry it splits the line, reconstructs the unsigned bytes, and
calls Ed25519 verify. A mismatch is reported with the offending
`seq` and the line proceeds to the next so the operator sees the full
extent of the breach in one pass.

### 12.4. Future: hardware key

The `sig` field is opaque to the format - swapping the software
implementation for a YubiKey/PIV signer requires no schema change.
The trade-off:

- **Software key (v1.0)**: same trust boundary as the daemon. Trivial
  to deploy. A privileged attacker who gets to the keyfile can
  retroactively re-sign the log; `chattr +a` and the hash chain are
  the floor against this.
- **Hardware key**: every signature requires a user touch. To make
  this viable for a per-entry log, sign the rotated file's tail hash
  rather than every entry. A `system.rotate_post.sig` field over the
  closed file's last hash gives a verifiable seal at file granularity
  without a button press per tool call.
