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

**Ubuntu / Debian / Mint / Pop!_OS — one line, nothing to pre-install:**

```bash
curl -fsSL https://raw.githubusercontent.com/dimita/os_windows/main/install.sh | sudo sh
```

The script installs Ollama if missing, pulls the default LLM
(`qwen2.5:3b`, ~2 GB) and the embedding model (`nomic-embed-text`,
250 MB), drops the prebuilt `.deb`, auto-adds you to the `wilai`
group, and seeds the default config. No Rust toolchain on your
machine. ~5 minutes total on a decent connection, mostly model
download.

Then **one** command to pick up the new group in your current
shell (Linux refreshes groups only at login):

```bash
newgrp wilai        # or just log out and back in
wilai-shell         # launch the TUI
```

### System requirements

- **8 GB RAM** is enough with the default `qwen2.5:3b` model
  (~2 GB resident when loaded, freed on shell exit).
- **12+ GB RAM** if you want Mistral 7B instead:
  `curl … | sudo WILAI_WITH_MISTRAL=1 sh`
- **x86_64 or aarch64** CPU, 10 GB disk (mostly Ollama models).
- Any distro with systemd + apt. GNOME/KDE/Xfce/Hyprland all fine
  for the TUI; the floating `wilai-hud` face only works on wlroots
  compositors (Hyprland, Sway, river, Wayfire).

### Opt-in extras

| Flag | What it adds | Extra disk | Extra RAM |
|---|---|---|---|
| `WILAI_WITH_MISTRAL=1` | mistral:7b-instruct as a second LLM | +4 GB | +4.5 GB |
| `WILAI_WITH_VISION=1` | llava:7b for `screen.capture` + `vision.describe` | +4.5 GB | +5 GB |
| `WILAI_WITH_VOICE=1` | whisper.cpp (STT) + piper (TTS) + models, enables `[voice]` | +400 MB | +500 MB |
| `WILAI_FROM_SOURCE=1` | build from `main` instead of using the prebuilt `.deb` | +2 GB build cache | needs 6+ GB RAM to compile |

Combine freely: `sudo WILAI_WITH_VOICE=1 WILAI_WITH_VISION=1 sh install.sh`

### Uninstall

```bash
curl -fsSL https://raw.githubusercontent.com/dimita/os_windows/main/scripts/uninstall.sh | sudo WILAI_PURGE_OLLAMA=1 WILAI_YES=1 sh
```

Removes Wilai + Ollama + models + state + sshd drop-in + the `wilai`
user. Keep models/config via `WILAI_KEEP_STATE=1` / `WILAI_KEEP_CONFIG=1`.

### Other install paths

```bash
# Boot the live ISO (fresh machine / VM / kiosk) — build from any Linux host:
sudo ./scripts/build-iso-docker.sh   # output: ./out/wilos-*.iso

# Arch / Manjaro / EndeavourOS:
makepkg -si wilai/dist/arch/PKGBUILD

# From Windows (bridges to a Linux Wilai over WSL or SSH):
#   PowerShell -ExecutionPolicy Bypass -File wilai\dist\windows\install.ps1

# SSH into a Wilai machine from any OS with an ssh client:
ssh wilai@my-server    # the `wilai` user's login shell IS wilai-shell
```

Full matrix with options and troubleshooting: [INSTALL.md](INSTALL.md).
Step-by-step walkthrough in a VirtualBox Ubuntu VM, including the full
pentest flow: [wilai/docs/TESTING.md](wilai/docs/TESTING.md).

## What's in the box

### Agent

- **Daemon** (`wilai-daemon`) — Unix-socket agent with multi-session
  support, a tamper-evident hash-chained audit log, and per-tool
  bubblewrap sandboxing. Default provider is a local Ollama; the
  cloud kill-switch triggers in Pentest mode. Models get unloaded
  from Ollama RAM when a shell session ends (no more 5-minute wait).
- **Memory** — SQLite (SQLCipher-encrypted) + local embeddings.
  Facts, notes, session summaries. Compacted periodically so the DB
  never grows into a Windows-like problem.
- **MCP** — load any Model Context Protocol server as a tool source
  via `[mcp.<name>]`. Backfill picks up late-starting servers.

### Interfaces

- **`wilai-shell`** — full-screen TUI with the ASCII robot face on
  the left, chat on the right. Keybinds, voice I/O (F2 push-to-
  talk), slash commands (`/help`, `/install`, `/update`, `/rollback`,
  `/remotes`, `/wizard`), history recall (Up/Down).
- **`wilai-hud`** — Wayland layer-shell permanent HUD overlay for
  wlroots compositors (Hyprland, Sway). Shows the face state (idle,
  listening, thinking, speaking, acting, alert, offline).
- **`wilai`** — simple CLI client for one-shot prompts.
- **`wilai-voice`** — standalone voice bridge (whisper + piper +
  wake-word).
- **`wilai-overlay`** — libnotify + zenity bridge for desktops that
  prefer toasts over a full HUD.
- **`wilai-bridge.exe`** — Windows native launcher that proxies to a
  Linux Wilai over WSL or SSH.

### Tool catalogue (YAML, confirmed, audited)

- **`sys.*`** — systemd, pacman/apt, NetworkManager, hostname,
  locale, user, disk, timedate.
- **`sys.install.*`** — disk layout, pacstrap, genfstab, chroot,
  bootloader, state migration. The `/install` flow uses these.
- **`memory.*`** — notes with embedding recall, facts (key/value),
  session summaries, compaction.
- **`remote.wilai.exec/read`** — SSH-based federation to other Wilai
  instances. Each remote stays sovereign.
- **`screen.capture` / `vision.describe`** — local llava screen
  description. Forbidden in Pentest mode.
- **`sys.wilai.update` / `sys.wilai.rollback`** — self-update with
  snapshot-based rollback.
- **`fs.*`, `git.*`, `hyprland.dispatch`** — the day-to-day basics.
- **Pentest project** (`pt.*`) + network tools (`nmap`, `nuclei`,
  `whatweb`) only loaded in Pentest mode. Scope guards cross-cut
  every network tool.

### Pentest project

A full engagement spine. Enter pentest mode, then:
`pt.project.start` → `pt.authz.register` (SHA-256-pinned RoE) →
`pt.scope.add` (CIDR, domain, URL globs with allow/require_ok/deny)
→ scope-guarded `nmap.scan` → `pt.finding.new` with a CVSS 3.1
vector → 6-state machine → `pt.report.render` (Markdown).
Walkthrough: [wilai/docs/TESTING.md §7](wilai/docs/TESTING.md).

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
- **Free RAM when you're not using it**. Models unload from Ollama
  when the shell closes; 5-minute keep-alive is a bad default on a
  small VM.
- **Keep the agent small**. If a feature pulls GTK, Electron, or
  anything cloud, it does not go in the base. Wilai should not slowly
  turn into another operating system.

## Status

Current release: **1.0.1** (bugfix of 1.0.0 — see the release
notes for the fixed list). Snapshot of what has landed:
foundation → audit → tools → MCP → memory → WilOS-Core ISO → voice
→ HUD → installer → proactivity → self-update + rollback →
federation → vision → three install paths → Windows bridge →
pentest project spine (M1).

Architecture, tool format, audit format, and the roadmap for the
remaining pentest milestones live under [`wilai/docs/`](wilai/docs/).

## Repository layout

```
install.sh             — the one-line installer
scripts/
  uninstall.sh         — nukes everything the installer put down
  build-iso.sh         — build the WilOS live ISO on an Arch host
  build-iso-docker.sh  — same, from Ubuntu/Debian via a podman container
  flash-usb.sh         — dd an ISO to a USB stick

wilai/                 — the agent and all its clients (Rust workspace)
  crates/              — wilai-core, wilai-audit, wilai-tools,
                         wilai-providers, wilai-mcp, wilai-memory,
                         wilai-pentest, wilai-daemon, wilai-cli,
                         wilai-shell, wilai-hud, wilai-voice,
                         wilai-overlay, wilai-bridge
  tools/core/          — YAML tool catalogue (default-loaded)
  tools/pentest/       — Pentest-only tool catalogue
  dist/arch/           — PKGBUILD + archiso profile (WilOS-Core)
  dist/debian/         — control.in + maintainer scripts for apt
  dist/windows/        — PowerShell launcher + install.ps1 for wilai.exe
  dist/systemd/        — user systemd units
  dist/hyprland/       — layerrule snippet for the HUD
  docs/                — architecture, tool format, audit format,
                         TESTING.md walkthrough
  scripts/             — build-deb, wilai-install, wilai-remote,
                         wilai-vision, wilai-screencap, wilai-selfupdate
  assets/branding/     — the Wilai avatar reference art
```

Hyprland desktop configuration, wallpaper generator, and the macOS-
leaning Aurora shell are still shipped — see `aurora/`, `hyprland/`,
`system/`.
