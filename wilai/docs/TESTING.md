# Testing Wilai in a VirtualBox VM — step-by-step

Goal: boot a fresh Ubuntu VM, install Wilai, exercise every feature
family. Expected total time: ~45 min once the base install is done.

Everything below is reproducible. When something fails, the mode op
says what the symptom looks like and where to look.

---

## 0. What you need on the host

- VirtualBox ≥ 7.0 (any OS: Linux / macOS / Windows)
- An Ubuntu 24.04 LTS ISO (desktop or server — server is lighter)
- 8+ GB free RAM, 30+ GB free disk, 2+ CPU cores to allocate
- A working internet connection (Wilai downloads an Ollama model
  on first use; ~4 GB for `mistral:7b-instruct`, ~250 MB for
  `nomic-embed-text`).

---

## 1. VirtualBox VM setup

- **Name:** `wilos-test`
- **Type / Version:** Linux / Ubuntu (64-bit)
- **RAM:** 6144 MB (minimum 4096; Ollama needs ~4 GB headroom)
- **CPUs:** 2
- **Disk:** 30 GB VDI, dynamically allocated
- **Network:** NAT (default) + optionally a Host-only adapter so
  you can SSH into the VM from the host at a stable IP
- **Audio:** leave default — voice I/O is optional in this mode op
- **Clipboard sharing:** Host → Guest (helps copy prompts in)

Boot the ISO, install Ubuntu with:
- user: `nora` (any name)
- disk: whole-disk, no LVM, no encryption (keep it simple for the
  test; LUKS works but adds noise)
- openssh-server: **yes** (we test the SSH drop-shell later)

After first reboot, from the VM:

```bash
sudo apt update
sudo apt install -y git curl build-essential pkg-config \
                     libxkbcommon-dev networkmanager pipewire \
                     openssh-server socat python3
```

---

## 2. Install Rust + Ollama

```bash
# Rust (as the normal user, not root)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustc --version   # expect >= 1.75

# Ollama (local LLM runtime, Wilai's default provider)
curl -fsSL https://ollama.com/install.sh | sh
sudo systemctl enable --now ollama

# Pull the two models Wilai expects by default.
ollama pull mistral:7b-instruct     # ~4 GB — this takes a while
ollama pull nomic-embed-text        # ~250 MB — for memory embeddings

# Sanity check
ollama list
ollama run mistral:7b-instruct "hello"
```

If `ollama run` streams back text, the local LLM half works.

---

## 3. Build and install Wilai

```bash
# Clone
git clone https://github.com/dimita/os_windows.git ~/wilai-src
cd ~/wilai-src/wilai

# Build + .deb (release, ~5 min on 2 cores)
sudo apt install -y dpkg-dev fakeroot
./scripts/build-deb --release
ls -lh dist/debian/out/wilai_*.deb
# → wilai_1.0.0_amd64.deb

# Install
sudo apt install -y ./dist/debian/out/wilai_1.0.0_amd64.deb

# The postinst created the `wilai` system user and enabled
# wilai-daemon.service. Confirm:
systemctl status wilai-daemon.service   # expect "active (running)"
sudo -u wilai ls /var/lib/wilai         # expect audit/ (created lazy)
```

Add your user to the `wilai` group so you can read audit files:

```bash
sudo usermod -aG wilai nora   # replace nora with your login
newgrp wilai                   # pick up the group in this shell
```

---

## 4. First prompt — core smoke test

```bash
wilai-shell
```

You should see the full-screen TUI with the ASCII robot face on the
left, chat area on the right, input at the bottom. If it looks
mangled, your terminal font is missing box-drawing glyphs — set the
terminal (gnome-terminal, konsole, kitty, …) to `Cascadia Code`,
`JetBrains Mono` or similar.

Type:

```
/help
```

→ The help card prints the four main sections (slash commands,
keybinds, natural-language workflows, pentest project).

Try a basic prompt:

```
Hi. Tell me which services are running on this machine.
```

Wilai will call `sys.service.list`, Ask you to **confirm** (press
`y`), then stream the result. Face flips `Acting` during the tool
run, back to `Idle` on `TurnDone`.

If no confirm appears, your tool catalogue is empty — check
`sudo ls /usr/share/wilai/tools/core/` has YAML files.

Exit with `Ctrl+C`.

---

## 5. Memory

```
Remember that this machine is my test VM and my preferred editor is nvim.
```

Wilai calls `memory.fact_set` (two keys usually: one for the VM
fact, one for the editor). Confirm each.

```
What do you know about me?
```

Wilai calls `memory.fact_list` and `memory.recall`, surfaces the
facts.

```
Note: nmap scan of 10.0.2.0/24 took 42s on 2026-10-21.
```

Wilai calls `memory.note` — the content gets embedded via Ollama's
`nomic-embed-text`.

```
Recall what you know about nmap scans.
```

→ The embedding search returns the note.

Check the SQLite is growing:

```bash
sudo ls -lh ~/.local/share/wilai/memory/wilai.sqlite
# or if running as system user, under /var/lib/wilai/
```

---

## 6. /update → /rollback round-trip

The point: upgrade is *reversible*. The daemon takes a snapshot of
every binary + the YAML tool catalogue + the systemd units before
running `apt install --only-upgrade wilai`.

In the shell:

```
/update
```

Wilai calls `sys.wilai.update` with `dry_run=true`. Because you just
installed `1.0.0` and there is no newer version in the repo,
output is "already on the latest version". That's fine — the point
is to exercise the snapshot path. Force a snapshot anyway:

```bash
# From a side terminal (the user who's in the wilai group)
sudo /usr/bin/wilai-selfupdate snapshot
sudo /usr/bin/wilai-selfupdate list
# → [{"version":"1.0.0","current":true,"manifest_path":"..."}]
```

Then back in the shell:

```
/rollback
```

Wilai calls `sys.wilai.rollback_list` and reads back the one
snapshot. If you ask to restore `1.0.0` back onto itself, it's a
no-op, which is still useful to prove the plumbing works:

```
Yes, restore 1.0.0.
```

Daemon restarts (brief "connecting…" in the TUI), shell reconnects,
you're back.

To actually see a real rollback, you need two versions. Easiest:
drop the Cargo workspace version to `1.0.1` in `Cargo.toml`,
`./scripts/build-deb --release`, `sudo apt install ./dist/.../deb`,
then `/rollback` back to `1.0.0`.

---

## 7. Pentest M1 — the real test

The goal: run a full engagement spine against your own VM.

### 7.1 Enter pentest mode

Pentest mode activates the dedicated `sys.wilai.update` ↔ scope
tools and locks out non-local LLM providers. Say in the shell:

```
Enter pentest mode.
```

Wilai calls the mode switch tool. The HUD/face flips to the
reddish Alert tint (if HUD is running). The status line shows
`mode: pentest`.

### 7.2 Start a project

Prepare a fake authz doc:

```bash
# From a side terminal
echo "Rules of Engagement — self-test, 2026-10-21, signed CTO." \
    > /tmp/roe.txt
```

In the shell:

```
Start a pentest project named selftest, engagement webapp.
```

Wilai calls `pt.project.start`. Expect:
```
project `selftest` (webapp) active · 0 scope rule(s) · root /var/lib/wilai/projects/selftest
```

### 7.3 Pin the authz

```
Pin the authz document at /tmp/roe.txt. Summary: self-test engagement. Jurisdiction: eu-west.
```

Wilai calls `pt.authz.register`. Returns the SHA-256 of
`/tmp/roe.txt`.

Verify drift detection:

```bash
# tamper with the file from outside
echo "mutated" >> /tmp/roe.txt
```

Back in the shell:

```
Verify the authz.
```

Wilai calls `pt.authz.show` → `verify: FAIL (hash drift)`. Good.
Rewrite the file to its original content to continue:

```bash
echo "Rules of Engagement — self-test, 2026-10-21, signed CTO." \
    > /tmp/roe.txt
```

### 7.4 Scope — the heart of M1

Add rules. Your VM NAT is typically `10.0.2.0/24`; your VM gateway
is `10.0.2.2`. We'll scan `10.0.2.0/24` but **deny** the gateway
to prove scope enforcement.

```
Add scope: allow cidr 10.0.2.0/24.
Add scope: deny ip 10.0.2.2 with note "VirtualBox gateway, not a target".
Add scope: require_ok cidr 10.0.2.3/32 with note "sensitive host, confirm each run".
```

Three `pt.scope.add` calls. Then:

```
Show me the current scope.
```

→ `pt.scope.list` prints three rules in evaluation order.

```
Is 10.0.2.5 in scope?
Is 10.0.2.2 in scope?
Is 1.1.1.1 in scope?
```

→ `pt.scope.check` returns, respectively: `allow`, `deny · ... VirtualBox gateway ...`, `deny · ... not in scope (no rule matched)`.

### 7.5 Scope-guard integration — try to scan OOS

**This is the headline M1 proof.** Attempt a nmap on the gateway:

```
Run nmap on 10.0.2.2.
```

Wilai calls `nmap.scan` → the `scope_denies(target)` guard fires
BEFORE the subprocess runs → the tool is refused with
`scope: target 10.0.2.2 is out of scope for this engagement`.

Check the audit log picked up the deny:

```bash
sudo tail -2 ~/.local/share/wilai/audit/current.jsonl
# → should include a tool.deny with guard: scope
```

Try `10.0.2.3` (require_ok):

```
Run nmap on 10.0.2.3.
```

The guard fires with a `require_ok` → surfaces a one-shot confirm
asking you if that host is in-scope right now. Answer `n` → tool
refused. Answer `y` → nmap runs.

Try `10.0.2.5` (allow):

```
Run a quick nmap on 10.0.2.5.
```

No scope prompt → nmap runs normally.

### 7.6 Findings + CVSS

Compute a CVSS vector first:

```
Compute the CVSS 3.1 score for AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H.
```

→ `pt.finding.cvss_calc` returns `10.0 · critical` (Log4Shell).

Record a finding using that vector:

```
Add a critical finding titled "Open telnet on the gateway",
CVSS vector CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H,
affected ["10.0.2.5:23"],
impact "cleartext administrative access to the host",
steps "nc 10.0.2.5 23 reveals a telnet banner",
remediation "disable telnetd, use ssh".
```

Then:

```
List findings.
Move finding <paste-id-from-above> to confirmed.
Render the report.
```

Three calls: `pt.finding.list`, `pt.finding.state`,
`pt.report.render`. The last writes
`/var/lib/wilai/projects/selftest/report-technical.md`.

Open it from the host:

```bash
cat /var/lib/wilai/projects/selftest/report-technical.md
```

→ Markdown report with RoE, scope table, findings grouped by
severity. Each severity bucket shows title, score, CVSS vector,
state, affected, impact, steps, remediation.

### 7.7 Close + audit chain integrity

```
Close the pentest project.
```

→ `pt.project.stop` → the scope checker is uninstalled; a
`pt.scope.check` now says "not configured".

Verify the audit log is intact:

```bash
# wilai audit verify is the chain checker (if installed as a subcommand)
wilai audit verify 2>&1 | tail -5
# → "chain OK, N entries, Y signatures verified"
```

If `wilai audit verify` doesn't exist in your build, read the
JSONL by hand — every entry has `prev_hash` chaining to the
previous one.

---

## 8. SSH drop-shell from the host

From your laptop (the host OS running VirtualBox), reach the VM
over SSH. First copy your public key:

```bash
# On the HOST, in a terminal
ssh-copy-id -p <vm-port> nora@<vm-ip>    # with NAT, you may need
                                          # port forwarding
```

Then on the VM, drop your pubkey into the `wilai` user's
authorized_keys (this is where the drop-shell convention lives):

```bash
sudo install -d -m 0700 -o wilai -g wilai /var/lib/wilai/.ssh
sudo install -m 0600 -o wilai -g wilai \
    /home/nora/.ssh/authorized_keys /var/lib/wilai/.ssh/authorized_keys
```

Back on the host:

```bash
ssh wilai@<vm-ip>
```

→ You should land **directly in the Wilai TUI** — not a bash shell.
The `wilai` user's login shell is `/usr/bin/wilai-shell`.

Hit `Ctrl+C` to exit; the SSH session closes.

---

## 9. (Optional) Windows bridge, from a Windows host

If your VM has a stable IP (host-only adapter) and you want to
test the Windows bridge:

On Windows (PowerShell as regular user):

```powershell
# Copy the release zip from the VM
scp -r nora@<vm-ip>:~/wilai-src/wilai/dist/windows .

# Or: build the Windows .exe directly on your VM:
#   rustup target add x86_64-pc-windows-gnu
#   sudo apt install -y mingw-w64
#   cargo build -p wilai-bridge --release --target x86_64-pc-windows-gnu
#   scp target/x86_64-pc-windows-gnu/release/wilai-bridge.exe \
#       nora@<vm-ip>:~  (then from Windows: scp back here)

cd windows
PowerShell -ExecutionPolicy Bypass -File install.ps1 -Mode ssh -Host wilai@<vm-ip>

# New PowerShell window:
wilai
```

→ Same TUI, this time proxied over SSH from Windows.

---

## 10. Teardown + where to find artifacts

- **Audit log:** `/var/lib/wilai/audit/current.jsonl` (hot) +
  `YYYY-MM-DD.jsonl` (rotated). Encrypted `*.enc` siblings if you
  enabled `[security] audit_encrypt_rotated`.
- **Memory DB:** `/var/lib/wilai/memory/wilai.sqlite` for the
  system daemon, or `~/.local/share/wilai/memory/wilai.sqlite` if
  running as your user.
- **Pentest project workspaces:** `/var/lib/wilai/projects/<name>/`
  with `project.toml`, `findings.sqlite`, `report-technical.md`.
- **Rollback snapshots:** `/var/lib/wilai/rollback/<version>/` with
  binaries, YAMLs, systemd units, `manifest.json`.
- **Daemon logs:** `journalctl -u wilai-daemon.service -b`.
- **Shell log:** `~/.local/state/wilai/shell.log` (only populated
  when `--log debug` or above).

To wipe and restart from scratch:

```bash
sudo systemctl stop wilai-daemon.service
sudo rm -rf /var/lib/wilai
sudo systemctl start wilai-daemon.service
```

To uninstall:

```bash
sudo apt purge wilai
# → removes binaries, config, state, the `wilai` user, and the
#   sshd drop-in. `apt remove` (without purge) keeps the state.
```

---

## 11. If something fails

| Symptom | Where to look |
|---|---|
| `wilai-shell` says "daemon unreachable" | `systemctl status wilai-daemon.service` + `journalctl -u wilai-daemon.service -n 50` |
| First prompt hangs forever | Ollama not reachable: `curl 127.0.0.1:11434/api/tags`. Pull the model again. |
| Memory tools fail with "embedder init failed" | `ollama pull nomic-embed-text` was skipped. |
| `pt.*` tools say "mode: forbidden" | You're not in pentest mode. "Enter pentest mode." first. |
| scope guard doesn't trigger | Did you call `pt.project.start`? The guard checks the global checker; no project = no checker = pass-through. |
| TUI is mangled | Font without box-drawing glyphs. Switch to Cascadia Code / JetBrains Mono. |
| `/update` says "cannot determine candidate version" | The repo your `.deb` was installed from isn't an apt-trusted source. That's fine for a self-test; `wilai-selfupdate snapshot` still works. |

---

## 12. What this mode op does NOT cover

- The ISO install (requires a USB stick or a second VM to receive
  the install). The build command is in INSTALL.md.
- The HUD (requires Hyprland or another wlroots compositor — Ubuntu
  default is GNOME, which does not run `wilai-hud` well out of the
  box). Doable with sway in a side session, but out of scope here.
- Voice (needs a working mic + audio stack + the whisper/piper
  models dropped under `/var/lib/wilai/models/`). The code paths
  are tested; the user-facing experience is best verified on a
  laptop, not a headless VM.
- Federation across machines (needs two VMs; the single-VM dry run
  above exercises `remote.wilai.*` only in error paths).

If any step above doesn't match reality, that's a bug worth filing.
