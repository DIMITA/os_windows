# WilOS + Wilai

**WilOS** is a Linux distribution where an on-device AI agent is a first-class
citizen. **Wilai** is that agent — a typed-tool LLM driver with a
hash-chained audit log, a local-only lock, and three distinct ways to meet it:
a boot-to-face live ISO, a `.deb`/pacman package for the Linux box you already
run, or an SSH drop-shell that lands remote users straight inside the TUI.

Everything stays local by default. The LLM runs on-device (Ollama). STT
(whisper.cpp), TTS (piper), embeddings (nomic), and the llava vision model
all run on-device. The `security.local_only = true` lock refuses non-local
providers unconditionally.

## Install

**Ubuntu / Debian / Mint / Pop!_OS** — one line, nothing to pre-install:

```bash
curl -fsSL https://raw.githubusercontent.com/dimita/os_windows/main/install.sh | sudo sh
```

The script installs Ollama if missing, drops the prebuilt `.deb`,
queues the LLM model pulls as a background systemd unit, and
exits. No Rust toolchain on your machine. ~90 seconds to a
working `wilai-shell`.

**Other paths:**

```bash
# Boot the live ISO (fresh machine / VM / kiosk) — build it from Ubuntu:
sudo ./scripts/build-iso-docker.sh   # output: ./out/wilos-*.iso

# Arch / Manjaro / EndeavourOS:
makepkg -si wilai/dist/arch/PKGBUILD

# From Windows (bridges to a Linux Wilai over WSL or SSH):
#   PowerShell -ExecutionPolicy Bypass -File wilai\dist\windows\install.ps1

# SSH into a Wilai machine from any OS with an ssh client:
ssh wilai@my-server    # the `wilai` user's login shell IS wilai-shell
```

Full matrix with options, config and troubleshooting: [INSTALL.md](INSTALL.md).

## What's in the box

### Agent

- **Daemon** (`wilai-daemon`) — Unix-socket agent with multi-session
  support, a tamper-evident hash-chained audit log, and per-tool
  bubblewrap sandboxing. Default provider is a local Ollama; the
  cloud kill-switch triggers in Pentest mode.
- **Memory** — SQLite + local embeddings. Facts, notes, session
  summaries. Compacted periodically so the DB never grows into a
  Windows-like problem.
- **MCP** — load any Model Context Protocol server as a tool source
  via `[mcp.<name>]`. Backfill picks up late-starting servers.

### Interfaces

- **`wilai-shell`** — full-screen TUI with the animated robot face
  on the left, chat on the right. Keybinds, voice I/O (F2 push-to-
  talk), slash commands (`/help`, `/install`, `/update`, `/remotes`,
  `/wizard`), history recall (Up/Down).
- **`wilai-hud`** — Wayland layer-shell permanent HUD overlay for
  Hyprland. Shows the face state (idle, listening, thinking,
  speaking, acting, alert, offline).
- **`wilai`** — simple CLI client for one-shot prompts.
- **`wilai-voice`** — standalone voice bridge (whisper + piper +
  wake-word).
- **`wilai-overlay`** — libnotify + zenity bridge for desktops that
  prefer toasts over a full HUD.

### Tool catalogue (YAML, confirmed, audited)

- **`sys.*`** — systemd, pacman, NetworkManager, hostname, locale,
  user, disk, timedate.
- **`sys.install.*`** — disk layout, pacstrap, genfstab, chroot,
  bootloader, state migration. The `/install` flow uses these.
- **`memory.*`** — notes with embedding recall, facts (key/value),
  session summaries.
- **`remote.wilai.exec/read`** — SSH-based federation to other Wilai
  instances. Each remote stays sovereign (its own daemon, audit,
  confirms).
- **`screen.capture` / `vision.describe`** — local llava screen
  description. Forbidden in Pentest mode.
- **`sys.wilai.update`** — self-update via pacman + unit restart.
- **`fs.*`, `git.*`, `hyprland.dispatch`** — the day-to-day basics.
- **Pentest tools** (`nmap`, `nuclei`, `whatweb`, …) in `tools/pentest/`,
  only loaded in Pentest mode.

### Proactivity

Opt-in `[proactivity]` turns on three watchers:

- **systemd** — reports newly-failed units.
- **disk** — reports mounts crossing a free-space threshold.
- **process** — reports a named watchlist process disappearing /
  returning.

Each nudge is rate-limited by `quota_per_hour` and reflected in the
HUD (face flips Alert) and the shell (red nudge line).

## Design values

- **Local-only by default**. The daemon refuses non-local providers
  when `security.local_only = true`, which it is.
- **Typed tools, not prompt engineering**. Every action a human
  takes crosses a YAML spec with a confirm guard and an audit entry.
- **Hash-chained audit**. The log is append-only + SHA-256-chained
  + Ed25519-signed, with chattr +a applied automatically when the
  filesystem supports it.
- **Keep the agent small**. If a feature pulls GTK, Electron, or
  anything cloud, it does not go in the base. Wilai should not slowly
  turn into another operating system.

## Status

Current release: **1.0.0**. Snapshot of what landed, in order:
foundation → audit → tools → MCP → memory → WilOS-Core ISO → voice →
HUD → installer → proactivity → self-update → federation → vision →
three install paths (ISO, apt, SSH drop-shell).

See [INSTALL.md](INSTALL.md) and [`wilai/docs/`](wilai/docs/) for
architecture, tool format, and audit format.

## Repository layout

```
wilai/                 — the agent and all its clients (Rust workspace)
  crates/              — wilai-core, wilai-audit, wilai-tools,
                         wilai-providers, wilai-mcp, wilai-memory,
                         wilai-daemon, wilai-cli, wilai-shell,
                         wilai-hud, wilai-voice, wilai-overlay
  tools/core/          — YAML tool catalogue (default-loaded)
  tools/pentest/       — Pentest-only tool catalogue
  dist/arch/           — PKGBUILD + archiso profile (WilOS-Core)
  dist/debian/         — control.in + maintainer scripts for apt
  dist/systemd/        — user systemd units
  dist/hyprland/       — layerrule snippet for the HUD
  docs/                — architecture, tool format, audit format
  scripts/             — build-deb, wilai-install, wilai-remote,
                         wilai-vision, wilai-screencap, wilai-selfupdate
```

Hyprland desktop configuration, wallpaper generator, and the macOS-
leaning Aurora shell are still shipped — see `aurora/`, `hyprland/`,
`system/`.
