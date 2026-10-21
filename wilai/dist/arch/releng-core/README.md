# WilOS-Core — headless-first Linux where Wilai is the interface

This profile builds the **WilOS-Core** ISO: a Linux distribution with no
X, no Wayland, no desktop. The entire user interface is Wilai, running
as a full-screen TUI (`wilai-shell`) on tty1, backed by `wilai-daemon`.
It is designed to democratise "server-grade" Linux: anything a sysadmin
would do with pacman, systemctl, nmcli, etc. is driven by asking Wilai in
natural language (voice or text), with the same `sys.*` tool family
every operation goes through — audit-logged and confirmed.

## Build

Requires an Arch host (or an Arch container) with `archiso` and `qemu`
if you want to test:

```bash
# From the workspace root (os_windows/wilai):
sudo pacman -S archiso qemu-base
sudo mkarchiso -v \
    -w /tmp/wilos-core-work \
    -o out \
    dist/arch/releng-core/
```

The resulting `out/wilos-core-*.iso` boots to a WilOS-Core login-less
tty1 that drops you directly into Wilai, which runs the first-boot
wizard (locale, keyboard, Wi-Fi, user, hostname).

## Try in a VM

```bash
qemu-system-x86_64 -enable-kvm -m 4G \
    -cdrom out/wilos-core-*.iso \
    -nic user,model=virtio
```

## Installing WilOS to disk

The ISO boots to the live Wilai shell. To install WilOS permanently on
a disk, type **`/install`** at the Wilai prompt. Wilai then walks you
through the sequence conversationally, confirming at each destructive
step:

1. Inventory block devices (`sys.disk.list`) and ask which target to use
2. Wipe (`sys.disk.wipe`), partition (`sys.disk.sgdisk_new`), format
   (`sys.disk.format`)
3. Mount (`sys.install.target_mount`, `sys.install.esp_mount`)
4. Pacstrap the base system (`sys.install.pacstrap`)
5. Generate fstab (`sys.install.genfstab`)
6. Timezone, locale, hostname, user (`sys.install.chroot_run`,
   `sys.install.user_create`)
7. Install the bootloader (`sys.install.bootloader_grub`)
8. Enable services on the new system (`sys.install.enable_service`)
9. Copy Wilai memory + facts (`sys.install.copy_wilai_state`) so what
   you taught it during the live session survives the reboot

Every mutating step prompts for confirmation, and the whole sequence is
hash-chain audited in `/var/lib/wilai/audit/` on the live system.

## Voice I/O

The ISO ships `whisper.cpp`, `piper` and `alsa-utils`. To enable voice:

1. Drop a whisper model (e.g. `ggml-base.en.bin`) under
   `/var/lib/wilai/models/`.
2. Drop a piper voice (`.onnx` + its `.json`) under the same folder.
3. In `~/.config/wilai/config.toml`, set `[voice] enabled = true` plus
   the model filenames (bare names resolve under the models dir).
4. Launch `wilai-shell`. F2 is push-to-talk, F3 toggles TTS mute.

For a hands-free wake-word, point `wake_cmd` at an external detector
that writes one line per detection (e.g. `openwakeword`). Each line
triggers a recording round. The whole stack is local — no network call.

## Layout

- `profiledef.sh` — archiso build knobs
- `packages.x86_64` — the ISO package set (no X/Wayland, has
  NetworkManager, pipewire, ollama)
- `pacman.conf` — build-time repos
- `airootfs/` — overlay copied into the live FS; owns the systemd units,
  skeletons, and the `/var/lib/wilai/wizard-pending` marker that tells
  `wilai-shell` to run the setup conversation on first boot
- `syslinux/`, `grub/`, `efiboot/` — bootloader configs

## Dual-ISO story

- **`releng-aurora/`** (ships Hyprland + `wilai-overlay` HUD on a full
  desktop) — targets daily-driver users who want a visual desktop
- **`releng-core/`** (this profile, no desktop) — targets anyone who
  wants a Linux box pilotable like an assistant, with no desktop in the
  way: homelab, server, kiosk, low-power machine, learning box

Both share the same Wilai workspace. The delta is the ISO profile and
the systemd units; the agent, tools, memory and audit are identical.
