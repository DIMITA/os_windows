# Wilai - Tool Format

Status: design, pre-code.
Audience: tool authors and reviewers. The reader is expected to know JSON
Schema, basic YAML, and Linux subprocess semantics.

Tools are the unit of capability that Wilai exposes to the LLM. Every
operation Wilai performs goes through a tool. There is no escape hatch
to raw shell beyond the explicitly-declared `shell.exec` tool, which is
itself a tool with strict guards.

## 1. Layout

Tool files live under one of:

- `wilai/tools/core/` - shipped with WilOS, always available.
- `wilai/tools/pentest/` - shipped, gated by `mode == pentest`.
- `~/.config/wilai/tools/` - user-defined, loaded last, may override
  shipped tools by name (with a warning at daemon start).

One file per tool. Filename matches the tool name with `/` replaced by
`.`: `fs.read.yaml` declares the tool `fs.read`.

The registry loader walks all configured directories on daemon start
(and on `SIGHUP`), parses each file, validates against the meta-schema,
and rejects the daemon start on any error. There is no partial load;
either every tool parses or the daemon fails to come up.

## 2. File schema (reference)

```yaml
# Required identity
name: string                  # dotted lowercase, e.g. "fs.read"
version: integer              # bump on breaking changes; loader picks max
description: string           # one-line, surfaced to the LLM verbatim

# Categorization
category: enum                # see "Categories" below
risk: enum                    # see "Risk levels" below
tags: [string]                # optional, free-form; for filtering

# Mode gating
mode_required: enum?          # if set, tool only loads in this mode
mode_forbidden: enum?         # if set, tool is hidden in this mode

# Inputs (JSON-Schema-shaped, simplified)
inputs:
  <name>:
    type: enum                # string | integer | number | boolean | enum | array | object
    description: string       # surfaced to the LLM
    required: bool            # default false
    default: any              # optional
    pattern: string?          # named validator, see "Patterns"
    min: number?              # for integer/number
    max: number?              # for integer/number
    values: [any]?            # for enum
    items: {...}?             # for array; recursive

# Guards: pre-execution policies
guards:
  - if: string                # CEL-like expression over args + context
    confirm: string            # prompt template; if present, requires user OK
    deny:    string?           # if present, hard refusal with this reason
  - require_mode: enum        # shorthand for an unconditional mode gate

# Executor
executor:
  type: enum                  # subprocess | builtin | http
  # subprocess fields:
  cmd: [string]               # argv with {placeholder} substitution
  cwd: string?                # default: caller's cwd
  env: {string: string}?      # key/value, no secrets
  stdin: string?              # template, optional
  timeout_s: integer          # required, max 3600
  output:
    capture: enum             # stdout | stderr | both | none
    max_bytes: integer        # truncates beyond this
    redact: [string]?         # named redactors to apply

# Audit shape
audit:
  fields: [string]            # which input fields to record verbatim
  redact: [string]?           # which output redactors to apply

# Optional documentation
examples:
  - args: {...}
    note: string
```

Unknown fields are rejected. The meta-schema is strict to keep tool files
forward-compatible: if you need a field that does not exist, propose it
upstream and bump the meta-schema version.

## 3. Categories

| Category     | Meaning                                                          |
|--------------|------------------------------------------------------------------|
| `read`       | Pure read of local state. No filesystem changes, no network.     |
| `write`      | Local filesystem changes, package metadata reads, git mutations. |
| `network`    | Outbound network: HTTP(S), DNS, SSH, etc.                        |
| `destructive`| Irreversible local action: rm, drop, format, force-push.         |
| `privileged` | Requires elevation (pkexec, sudo). Mutates system state.         |
| `pentest`    | Offensive tooling. Loaded only in `mode == pentest`.             |
| `keyring`    | Reads or writes the user's secret store. Hidden in pentest mode. |

A tool has exactly one category. If two seem to fit, pick the one with
higher confirmation friction; it is the conservative choice and the
guards can soften it case by case.

## 4. Risk levels

| Risk          | Default confirmation policy in `normal` |
|---------------|-----------------------------------------|
| `none`        | Run silently.                           |
| `low`         | Show a 1-line notice; no prompt.        |
| `medium`      | Prompt; default action is `yes`.        |
| `high`        | Prompt; no default; requires explicit input. |
| `critical`    | Prompt; user must type a confirmation token displayed in the prompt. |

Category and risk together determine the default confirmation. Guards
override the default for specific arg patterns (e.g. a `read` tool that
becomes `high` if the path is outside `$HOME`).

## 5. Patterns

Named validators applied to string arguments. Built-in patterns:

| Pattern         | Matches                                                  |
|-----------------|----------------------------------------------------------|
| `path`          | Absolute or relative POSIX path; no NUL, no newline.     |
| `path_in_home`  | Path that resolves under `$HOME` after canonicalization. |
| `cidr`          | IPv4/IPv6 CIDR.                                          |
| `host`          | DNS name or IP literal.                                  |
| `cidr_or_host`  | Either of the above.                                     |
| `nmap_port_spec`| `22`, `1-1024`, `top-1000`, `T:80,U:53`, etc.            |
| `url`           | Absolute URL with allowed scheme list (http, https).     |
| `git_ref`       | Valid git refname per `git check-ref-format`.            |
| `pkg_name`      | Valid pacman package name.                               |
| `username`      | POSIX username, `[a-z_][a-z0-9_-]{0,31}`.                |

Patterns are validated by `wilai-tools::patterns`. Adding a pattern is a
Rust function plus a registry entry; tool files do not embed regexes
inline. This guarantees that pattern semantics are reviewed once and
reused.

## 6. Guards

Guards run after schema validation, in order. Each guard is one of:

```yaml
- require_mode: pentest
- if: "rate_pps > 1000"
  confirm: "Rate {rate_pps} pps is high. Proceed?"
- if: "not target_in_local_lan(target)"
  deny: "target outside the local LAN; declare it in .wilrc to allow"
```

The expression language is intentionally small: literals, identifiers
(arg names + helpers), comparison, boolean ops, function calls from a
short whitelist (`target_in_local_lan`, `path_under`, `is_root_owned`,
etc.). No string interpolation in `if`. Templates in `confirm` and
`deny` use `{name}` substitution.

Guard outcomes:

- `deny` triggers a hard refusal, audited with reason.
- `confirm` triggers a `ConfirmRequired` IPC event. The user answers
  via the calling client; default is `no`. A `no` is audited and the
  tool is not executed.

## 7. Executors

### `subprocess`

The default. Runs a binary with argv constructed by per-arg
substitution into `cmd`. Substitution is whole-token only:
`["{flag}"]` becomes `["--target=10.0.0.0/24"]`, never two argv tokens.
If `flag` contained a space, that space stays inside the single argv
token. Newlines in any rendered token cause a hard refusal.

The executor inherits the daemon's environment minus a denylist
(`SUDO_*`, `SSH_*`, `*_TOKEN`, `*_KEY`), then merges the tool's `env`
map. It does not pass through interactive TTY; tools that need a
terminal must say so via a `tty: true` flag (not in v0.5).

Timeouts are enforced with `SIGTERM` then `SIGKILL` after 5 seconds.

### `builtin`

A function in `wilai-tools::builtins`. Used for `fs.read`, `fs.list`,
`fs.write`, `sysinfo.get`, `hyprland.dispatch`, etc. Builtins are
preferable to subprocess when the operation is small and pure-Rust;
they avoid fork overhead and are easier to test.

A builtin is referenced by name:

```yaml
executor:
  type: builtin
  fn: fs.read.v1
  timeout_s: 5
  output:
    capture: both
    max_bytes: 1048576
```

Builtins are versioned independently of their YAML wrappers. A v2
adding a new optional field can ship while v1 remains addressable.

### `http`

Reserved. Out of scope for v0.5. Intended for tools that talk to local
HTTP services (e.g. `ollama.list_models`).

## 8. Output handling

The executor's stdout / stderr is captured up to `max_bytes` and passed
back to the agent loop as the tool result. Beyond the limit the output
is truncated and the result includes a `truncated: true` flag.

`output.redact` runs named redactors in order over the captured bytes
before they enter the audit log and the LLM context. Built-in redactors:

| Name           | Effect                                                     |
|----------------|------------------------------------------------------------|
| `ipv4_private` | Replace 10/8, 172.16/12, 192.168/16 with `[private-ip]`.   |
| `home_paths`   | Replace `$HOME` prefix with `~`.                           |
| `tokens`       | Replace strings matching `[A-Za-z0-9_-]{32,}` with `[token]` (heuristic; opt-in). |
| `email`        | Replace local-part of email addresses.                     |

Redactors are heuristic; they reduce, not eliminate, exposure. Audit
fidelity for forensic replay may require disabling them in `wilai.toml`.

## 9. Audit fields

`audit.fields` lists the input fields that should be logged verbatim.
Fields not listed are recorded with their type and length only
(`{"path": "<string len=42>"}`). This lets a tool author keep sensitive
inputs out of the log while preserving non-sensitive context.

The audit entry always includes: tool name, tool version, executor type,
exit code, duration, timestamp, session id, mode, provider, model. See
[`audit-format.md`](audit-format.md) for the entry schema.

## 10. Versioning

`version` is an integer. Breaking changes (input schema removal, type
narrowing, semantics change) require a bump. The loader picks the
highest version present for a given tool name. Two versions of the same
tool can coexist on disk during a transition; older callers keep
working until the LLM is retrained or the user removes the v(N-1) file.

## 11. Authoring guide

To add a tool:

1. Pick the smallest category that fits. Prefer `read` over `write` over
   `destructive`. Prefer `builtin` over `subprocess` when the work is
   short and the deps are minimal.
2. Write the YAML, starting from the closest existing tool.
3. Run `wilai tool validate path/to/tool.yaml` (post v0.5: a CLI for this).
4. Run `wilai tool dry-run path/to/tool.yaml --args '{"...": "..."}'`
   which prints the rendered argv (or the resolved builtin call) without
   executing it.
5. Add at least one example to the `examples` block. The LLM sees
   examples; they materially improve fill quality on small models.
6. Open a PR. Tool reviews focus on: category honesty, guard coverage,
   redactor choices, and prompt clarity in `description`.

## 12. Examples

### `fs.read` (builtin, read)

```yaml
name: fs.read
version: 1
description: Read a file from disk and return its contents as text.
category: read
risk: none

inputs:
  path:
    type: string
    pattern: path
    required: true
    description: Absolute or relative path to a regular file.
  max_bytes:
    type: integer
    min: 1
    max: 1048576
    default: 65536
    description: Truncate the response after this many bytes.

guards:
  - if: "not path_under(path, [home, '/etc', '/usr/share', cwd])"
    confirm: "Path {path} is outside the usual roots. Read it?"

executor:
  type: builtin
  fn: fs.read.v1
  timeout_s: 5
  output:
    capture: stdout
    max_bytes: 1048576
    redact: [home_paths]

audit:
  fields: [path, max_bytes]

examples:
  - args: { path: "/etc/hosts" }
    note: "Inspect host name resolution."
```

### `nmap.scan` (subprocess, pentest)

```yaml
name: nmap.scan
version: 1
description: Run an nmap scan against a target.
category: pentest
risk: high
mode_required: pentest

inputs:
  target:
    type: string
    pattern: cidr_or_host
    required: true
  ports:
    type: string
    pattern: nmap_port_spec
    default: "top-1000"
  technique:
    type: enum
    values: [syn, connect, udp]
    default: syn
  rate_pps:
    type: integer
    min: 1
    max: 5000
    default: 500

guards:
  - if: "not target_in_local_lan(target)"
    confirm: "Target {target} is outside the LAN. Confirm?"
  - if: "rate_pps > 1000"
    confirm: "Rate {rate_pps} pps is high. Confirm?"

executor:
  type: subprocess
  cmd: ["nmap", "-s{technique[0]|upper}", "-p", "{ports}", "--max-rate", "{rate_pps}", "{target}"]
  timeout_s: 600
  output:
    capture: both
    max_bytes: 524288

audit:
  fields: [target, ports, technique, rate_pps]

examples:
  - args: { target: "192.168.1.0/24", ports: "22,80,443" }
    note: "Quick LAN sweep on common service ports."
```

### `shell.exec` (subprocess, escape hatch, gated)

```yaml
name: shell.exec
version: 1
description: |
  Run a shell command. Last resort. Prefer specific tools when one exists.
  The LLM is instructed to justify use of this tool and to propose the
  equivalent typed tool if any.
category: write
risk: critical

inputs:
  cmd:
    type: array
    items: { type: string }
    required: true
    description: argv array. The first element is the binary; subsequent are args.
  cwd:
    type: string
    pattern: path
    default: "."
  reason:
    type: string
    required: true
    description: Why a typed tool is not appropriate here.

guards:
  - if: "true"
    confirm: "Run `{cmd|join(' ')}` in {cwd}? Reason: {reason}"

executor:
  type: subprocess
  cmd: ["{cmd}"]                # spread, special-cased at validator
  cwd: "{cwd}"
  timeout_s: 60
  output:
    capture: both
    max_bytes: 262144

audit:
  fields: [cmd, cwd, reason]
```

`shell.exec` is the only tool that takes an argv array as a single
argument; the validator special-cases the spread. It is always
`critical` and always confirms. Disabling it in `wilai.toml` is
supported and recommended for users who want a hard guarantee that
Wilai cannot run arbitrary commands.
