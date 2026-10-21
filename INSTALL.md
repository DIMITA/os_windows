# Installing Wilai

Wilai ships three ways. Pick the one that fits what you already run.

## 1. Boot the WilOS ISO (fresh machines, VMs, kiosks)

Two flavors, one agent:

- **WilOS-Core** — headless, Wilai *is* the shell on tty1. For
  servers, homelabs, low-power boxes and anyone who wants Linux
  without a desktop in the way.
- **WilOS-Aurora** — Hyprland desktop with a permanent robot-face
  HUD overlay (`wilai-hud.service`).

Build on an Arch host:

```bash
# Core (headless)
sudo pacman -S archiso
sudo mkarchiso -v -w /tmp/wilos-core-work -o out wilai/dist/arch/releng-core/
qemu-system-x86_64 -enable-kvm -m 4G -cdrom out/wilos-core-*.iso
```

Boot it, Wilai greets you, type `/install` when ready to go on disk.
Everything from locale picking to GRUB is conversational and
hash-chain audited.

Full guide: `wilai/dist/arch/releng-core/README.md`.

## 2. Install from a package (existing Linux boxes)

### Arch / Manjaro / EndeavourOS

```bash
cd wilai/
makepkg -si dist/arch/PKGBUILD
```

Puts the binaries under `/usr/bin`, the systemd units under
`/usr/lib/systemd`, enables `wilai-daemon.service`, creates the
`wilai` system user, and lays down an sshd drop-in snippet (see
below).

### Debian / Ubuntu / Mint (apt)

```bash
# Build the .deb on a Debian host:
cd wilai/
./scripts/build-deb --release
# → dist/debian/out/wilai_<version>_amd64.deb

sudo apt install ./dist/debian/out/wilai_*.deb
```

The .deb carries the same payload as the Arch package and the same
postinst behavior: creates the `wilai` user, enables the daemon,
drops `/etc/ssh/sshd_config.d/50-wilai.conf`.

**Recommended apt deps** are listed in the control file: `ollama` (or
`ollama-cuda`), `networkmanager`, `pipewire`, `openssh-server`,
`socat`, `python3`, `curl`. Vision + voice + pentest tools are
`Suggests:` so they do not pull in GUI stacks you did not ask for.

After install:

```bash
# Local use
wilai                              # one-shot chat
wilai-shell                        # full-screen TUI (any tty)
systemctl status wilai-daemon      # the agent is running system-wide

# Local Ollama
ollama pull mistral:7b-instruct    # or whatever you prefer
ollama pull nomic-embed-text       # needed by memory.*
```

### Config

```bash
cp /usr/share/wilai/wilai.toml.example ~/.config/wilai/config.toml
# Edit [persona], [voice], [proactivity], [remotes] to taste.
```

## 3. SSH into a Wilai machine (drop-shell)

Once installed, every Wilai box exposes an SSH drop-shell. The
`wilai` system user has `/usr/bin/wilai-shell` as its login shell, so
a plain `ssh` session lands you directly in the TUI — no login
prompt escape to a bash shell, by design.

```bash
# On the server: drop your client public key into
#   /var/lib/wilai/.ssh/authorized_keys
sudo install -d -m 0700 -o wilai -g wilai /var/lib/wilai/.ssh
sudo install -m 0600 -o wilai -g wilai ~your_user/.ssh/id_ed25519.pub \
    /var/lib/wilai/.ssh/authorized_keys

# From your laptop:
ssh wilai@my-server.example.com
# → Wilai's full-screen face + chat, over the SSH tty.
```

The `50-wilai.conf` sshd snippet both installers lay down tightens
the `wilai` user's session: no agent forwarding, no TCP forwarding,
no X11, TTY only. Password auth is left at whatever your global
`sshd_config` says — keep it `no` on anything exposed to the
internet.

### Federation vs SSH drop-shell

- **SSH drop-shell** (this section): one user *is* Wilai from
  wherever they log in. One agent, many entry terminals.
- **Federation** (`remote.wilai.exec/read`): one Wilai instance
  delegates a sub-task to another Wilai on another box. Each remote
  has its own daemon, its own audit log, its own confirms. Use
  federation when you want machines to collaborate; use SSH when
  you just want to use your own Wilai from somewhere else.

Both can coexist on the same host.

## What you get in all three

- **Local-only lock** (`security.local_only = true` by default). The
  LLM runs on-device. Memory embeddings run on-device. Voice (STT +
  TTS) runs on-device. Vision (llava) runs on-device.
- **Hash-chained audit** of every tool call, every confirm, every
  nudge, under `/var/lib/wilai/audit/`.
- **Confirmed tool catalogue**: `sys.*`, `memory.*`, `remote.*`,
  `vision.*`, `screen.capture`, plus whatever MCP servers you wire
  in via `[mcp.<name>]`.
- **Proactivity** (opt-in): `[proactivity] enabled = true` turns on
  watchers (systemd failed units, disk free, process watchlist) that
  emit rate-limited nudges.
