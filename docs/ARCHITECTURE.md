# WilOS Aurora — Architecture

WilOS is a Linux distribution: every layer below the shell is a
component the wider Linux ecosystem already maintains. The novelty
sits in the shell, the design system, and the install experience.

## Layers

```
+-------------------------------------------------------------------+
|  Shell — Aurora                                                   |
|    Hyprland (Wayland compositor, blur + animations)               |
|    Waybar × 2  (top menu bar, bottom dock)                        |
|    wofi (Spotlight launcher)   mako (notifications)               |
|    swww (wallpaper)            polkit-kde-agent (auth)            |
+-------------------------------------------------------------------+
|  Userland                                                         |
|    GNOME apps (Files, Text Editor, Calculator, Photos, ...)       |
|    Firefox, Kitty, mpv, htop, fastfetch                           |
|    NetworkManager, BlueZ, PipeWire, WirePlumber                   |
+-------------------------------------------------------------------+
|  Session manager                                                  |
|    greetd + tuigreet (login)                                      |
|    systemd (services, sockets, timers)                            |
+-------------------------------------------------------------------+
|  Base                                                             |
|    Arch Linux base + base-devel                                   |
|    Pacman (package manager)                                       |
|    Btrfs root with @ / @home / @log / @cache / @snapshots         |
+-------------------------------------------------------------------+
|  Kernel + drivers                                                 |
|    Linux (latest stable) + linux-firmware                         |
|    intel-ucode / amd-ucode                                        |
+-------------------------------------------------------------------+
|  Bootloader                                                       |
|    systemd-boot (UEFI) or GRUB                                    |
+-------------------------------------------------------------------+
```

## Build pipeline

```
  scripts/make-wallpaper.sh    →   distro/airootfs/usr/share/backgrounds/wilos/aurora.jpg
  scripts/build-iso.sh         →   sudo mkarchiso -v distro/   →   out/wilos-*.iso
  scripts/flash-usb.sh         →   sudo dd …
                                                              ↓
                                                  Live ISO booted on hardware
                                                              ↓
                                                  sudo wilos-install
                                                              ↓
                                                  Installed system on disk
```

## Conventions

- The live system is *exactly* what gets installed: the same shell,
  the same dotfiles in `/etc/skel`. There is no "live-only" theme.
- Anything user-visible lives under `distro/airootfs/etc/skel/.config/`
  so changes are picked up by both new live boots and freshly created
  installed users.
- The installer (`wilos-install`) **must** require a typed token
  (`WIPE <disk>`) before any destructive operation, and **must**
  refuse to write to the disk that backs the live ISO.
- Aurora design tokens (colours, radii, motion) live in `docs/DESIGN.md`
  and are mirrored in the relevant config files (Hyprland decoration,
  Waybar CSS, wofi CSS, Kitty palette). When a token changes, every
  surface changes in lock-step.

## Where to look for what

| You want to change…              | Edit…                                                      |
|----------------------------------|------------------------------------------------------------|
| Window radius / blur / shadows   | `distro/airootfs/etc/skel/.config/hypr/hyprland.conf`      |
| Top bar layout / modules         | `distro/airootfs/etc/skel/.config/waybar/config.jsonc`     |
| Top bar visual style             | `distro/airootfs/etc/skel/.config/waybar/style.css`        |
| Dock icons / actions             | `distro/airootfs/etc/skel/.config/waybar/dock.jsonc`       |
| Dock visual style                | `distro/airootfs/etc/skel/.config/waybar/dock.css`         |
| Launcher                         | `distro/airootfs/etc/skel/.config/wofi/`                   |
| Notifications                    | `distro/airootfs/etc/skel/.config/mako/config`             |
| Terminal palette                 | `distro/airootfs/etc/skel/.config/kitty/kitty.conf`        |
| Login screen prompt              | `distro/airootfs/etc/greetd/config.toml`                   |
| Live packages                    | `distro/packages.x86_64`                                   |
| Installed-system packages        | `distro/airootfs/etc/wilos/install-packages.list`          |
| Installer flow                   | `distro/airootfs/usr/local/bin/wilos-install`              |
| Wallpaper generator              | `scripts/make-wallpaper.sh`                                |
| Boot menu (UEFI)                 | `distro/efiboot/loader/`                                   |
| Boot menu (GRUB)                 | `distro/grub/grub.cfg`                                     |
| Boot menu (BIOS / syslinux)      | `distro/syslinux/syslinux.cfg`                             |
| Plymouth boot splash             | `distro/airootfs/usr/share/plymouth/themes/wilos/`         |
| GTK4 / libadwaita theme          | `distro/airootfs/etc/skel/.config/gtk-4.0/`                |
| GTK3 fallback theme              | `distro/airootfs/etc/skel/.config/gtk-3.0/`                |
| WilOS logo / branding            | `distro/airootfs/usr/share/wilos-branding/`                |
| Graphical installer (GTK4)       | `distro/airootfs/usr/local/bin/wilos-installer`            |
| Welcome tour                     | `distro/airootfs/usr/local/bin/wilos-tour`                 |
| Time Machine snapshot UI         | `distro/airootfs/usr/local/bin/wilos-timemachine`          |
| Snapshot rollback helper         | `distro/airootfs/usr/local/bin/wilos-snapshot-rollback`    |
| Custom Aurora icon set           | `distro/airootfs/usr/share/icons/wilos-aurora/`            |
| Avatar variants                  | `distro/airootfs/usr/share/wilos-branding/avatars/`        |
| System sounds                    | `distro/airootfs/usr/share/sounds/wilos/`                  |
| Snapper root config              | `distro/airootfs/etc/snapper/configs/root`                 |
| Snapper pacman hooks             | `distro/airootfs/etc/pacman.d/hooks/*.hook`                |
| PolicyKit policy for installer   | `distro/airootfs/usr/share/polkit-1/actions/`              |
| Installer desktop entry          | `distro/airootfs/usr/share/applications/`                  |
| Wilbar (native top bar, Rust)    | `wilbar/` (Cargo crate)                                    |
| Wildock (native dock, Rust)      | `wildock/` (Cargo crate)                                   |
| Wildock pinned items config      | `distro/airootfs/etc/wilos/dock.toml`                      |
| Wilcenter (command palette)      | `distro/airootfs/usr/local/bin/wilcenter`                  |
| Snap layouts overlay             | `distro/airootfs/usr/local/bin/wilos-snap`                 |
| Notes app                        | `distro/airootfs/usr/local/bin/wilos-notes`                |
| Calendar / Mail / Camera shells  | `distro/airootfs/usr/share/applications/wilos-{calendar,mail,camera}.desktop` |
