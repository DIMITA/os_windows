# Wilai on Windows

Wilai is a Linux agent. On Windows, you don't *install* the agent —
you install a **bridge** that forwards your terminal to a Linux Wilai
instance. The real daemon still runs on Linux, where it belongs.

Two bridge modes, both using stock Windows components:

- **WSL** — Wilai runs inside a WSL2 distro you already use (Ubuntu,
  Arch, WilOS-in-WSL, …). Fast, no network, zero extra deps.
- **SSH** — Wilai runs on a remote Linux box (homelab, VPS, dev
  server). Uses OpenSSH client, built into Windows 10 1809+.

Pick what fits. You can switch any time by editing
`%APPDATA%\wilai\config.toml` or passing `--mode wsl` / `--mode ssh`
on the command line.

## Install

### Option A — the one-liner

```powershell
# From a PowerShell window in this folder:
PowerShell -ExecutionPolicy Bypass -File install.ps1
```

The installer:
1. Installs **`wilai.exe`** (native x64, ~1.3 MB, no runtime deps)
   into `%LOCALAPPDATA%\Programs\wilai\`. If no `.exe` is next to
   `install.ps1`, falls back to the `.ps1` launcher + a `wilai.cmd`
   stub.
2. Adds that folder to **your user PATH** (no admin needed).
3. Asks whether you want WSL or SSH, writes
   `%APPDATA%\wilai\config.toml` with sensible defaults.

Open a new terminal; `wilai` is now a command.

The native `wilai.exe` and the `.ps1` launcher behave identically —
same config file, same `--mode wsl|ssh`, same `--dump-config` and
`--help`. The `.exe` is just faster to start and does not need
PowerShell's ExecutionPolicy bypass.

### Option B — winget (once the manifest is published)

```powershell
winget install wilos.wilai
```

### Option C — manual

Drop `wilai.exe` wherever you keep binaries and call it directly.
No install step, no PATH change — the exe is self-contained.

```powershell
.\wilai.exe
.\wilai.exe --mode ssh
.\wilai.exe --dump-config
```

If you only have the `.ps1`:

```powershell
PowerShell -ExecutionPolicy Bypass -File .\wilai.ps1
```

## Bridge setup

### WSL mode

Make sure your WSL distro has Wilai installed. For a Debian/Ubuntu
WSL, install the `.deb` the Linux side ships:

```bash
# inside WSL:
./scripts/build-deb --release
sudo apt install ./dist/debian/out/wilai_*.deb
```

For an Arch WSL, build the PKGBUILD. Then on Windows:

```powershell
wilai --mode wsl
# or set it as default in %APPDATA%\wilai\config.toml
```

Default config:

```toml
[bridge]
mode = "wsl"

[bridge.wsl]
distro = "WilOS"        # or "Ubuntu", "Arch", your distro name
user   = ""             # empty = WSL's default user
shell  = "wilai-shell"  # command run inside WSL
```

### SSH mode

The target Linux box must have Wilai installed and must expose the
`wilai` system user with `wilai-shell` as login shell. Both the
`.deb` postinst and the Arch `wilai.install` hook set this up —
that's the point of v4.3's SSH drop-shell story.

Drop your Windows user's public key into the server's
`/var/lib/wilai/.ssh/authorized_keys` (same as any SSH setup).

```powershell
wilai --mode ssh
```

Default config:

```toml
[bridge]
mode = "ssh"

[bridge.ssh]
host = "wilai@my-box.lan"
port = 22
key  = ""     # path to a specific key, or empty for the OpenSSH default
```

## What the bridge does NOT do

- No service / tray icon. The launcher is a per-invocation script.
- No local Wilai daemon on Windows. The daemon stays on Linux.
- No port of the YAML tool catalogue to Windows. Every tool call
  the agent makes (systemctl, pacman, nmcli, …) runs on the Linux
  side where it has meaning.
- No admin privileges. Everything lives under your user profile.

If you want Wilai to feel more native on Windows (tray icon,
run-at-login, named-pipe transport), that's a separate project. The
bridge covers the 90% case: you have a Linux box, you want to use
it from Windows, you want it to feel like `wilai` is a Windows
command.

## Uninstall

```powershell
PowerShell -ExecutionPolicy Bypass -File install.ps1 -Uninstall
```

Removes the install folder and strips it from PATH. The config at
`%APPDATA%\wilai\` is left in place; delete it by hand if you want
a clean slate.

## Troubleshooting

**`wilai` is not recognized** — open a new terminal; the PATH change
takes a fresh shell. If still broken, confirm
`%LOCALAPPDATA%\Programs\wilai` is in `$env:Path`.

**`wsl.exe not found`** — install WSL2:
`wsl --install`. Reboot. Pick a distro, install Wilai inside it.

**`ssh.exe not found`** — Settings → Apps → Optional Features →
Add → OpenSSH Client.

**Permission denied (publickey)** — your key is not in the server's
`/var/lib/wilai/.ssh/authorized_keys`. See the Linux side's
INSTALL.md § SSH drop-shell.

**The TUI looks mangled** — use Windows Terminal (`wt.exe`),
not the legacy conhost. Set the font to something monospaced that
has box-drawing glyphs (Cascadia Code, JetBrains Mono).
