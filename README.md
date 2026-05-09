# WilOS Aurora

WilOS is a desktop operating system built on the Linux kernel, an
Arch base, and a custom **Aurora** shell — glassmorphic, animated,
GPU-blurred, with macOS-leaning ergonomics (top menu bar, centered
floating dock, Spotlight launcher, trackpad gestures) and the breadth
of the Linux ecosystem underneath.

This repository contains the **distribution sources**: the archiso
profile, the shell configuration tree (Hyprland, Waybar, wofi, mako,
Kitty), the installer, the wallpaper generator, and the helper
scripts that turn it all into a bootable, installable ISO.

## What ships

- **Linux kernel** (latest stable) with full hardware support: AHCI,
  NVMe, USB, Wi-Fi, Bluetooth, GPU drivers, audio.
- **Aurora compositor**: Hyprland with vibrant blur, 16 px rounded
  corners, ambient shadows, spring-curve animations, gesture-driven
  workspace switching, tearing-free.
- **macOS-leaning shell**:
  - **Top bar** (Waybar) — app menu trigger, workspace dots, current
    window title, centered live clock, status icons (network, BT,
    sound, brightness, battery, power).
  - **Bottom dock** (Waybar instance) — floating glass slab,
    centered, hover-lift animation, launcher icons + open-app
    taskbar.
  - **Spotlight launcher** (wofi) — Super+Space, fuzzy search of
    apps and files.
  - **Notifications** (mako) — top-right glass cards.
- **Apps**: Files (Nautilus), Terminal (Kitty themed), Browser
  (Firefox), Mail/Calendar/Music/Photos/Calculator, Settings panel.
- **Login**: greetd + tuigreet, "Welcome to WilOS — Light, made
  personal."
- **Installer** (`wilos-install`): guided text installer. Refuses to
  touch the live disk. Requires the user to type
  `WIPE <disk>` exactly before any write.
- **Btrfs root** with `@ / @home / @log / @cache / @snapshots`
  subvolumes ready for snapper-style snapshots later.

## Quick start

### Build the ISO (on an Arch host)

```sh
sudo pacman -S archiso imagemagick
sudo ./scripts/build-iso.sh
# → ISO appears in ./out/wilos-YYYY.MM.DD-x86_64.iso
```

### Flash to a USB stick

```sh
sudo ./scripts/flash-usb.sh out/wilos-*.iso /dev/sdX
```

The script refuses anything that looks like an internal disk
(`sda`, `nvme0n1`, `mmcblk0`) and asks for `FLASH sdX` to be typed.

### Install onto a machine

Boot the USB. Open a terminal (Super + Return). Run:

```sh
sudo wilos-install
```

You will be asked for a target disk, hostname, user, password,
locale, timezone, then to type `WIPE <disk>` to authorise the wipe.

## Repository layout

```
distro/
  profiledef.sh                archiso profile metadata
  packages.x86_64              packages installed in the ISO
  pacman.conf                  build-time pacman config
  syslinux/                    BIOS boot menu
  efiboot/                     systemd-boot menu (UEFI)
  grub/                        GRUB menu
  airootfs/                    overlay rooted at / in the live system
    etc/
      greetd/config.toml         autologin into Hyprland
      os-release / hostname / motd
      skel/.config/              user defaults
        hypr/hyprland.conf       Aurora compositor config
        waybar/{config,style,dock} top bar + bottom dock
        wofi/{config,style.css}  Spotlight launcher
        kitty/kitty.conf         terminal theme
        mako/config              notifications theme
      wilos/install-packages.list packages pacstrap installs
    usr/
      local/bin/
        wilos-install            installer (typed-confirmation gate)
        wilos-firstrun           first login welcome
      share/backgrounds/wilos/   wallpaper(s)

scripts/
  build-iso.sh        sudo wrapper around mkarchiso
  flash-usb.sh        guarded dd-to-USB helper
  make-wallpaper.sh   generates the Aurora wallpaper

docs/
  ARCHITECTURE.md     where each component lives, how it fits
  DESIGN.md           the Aurora visual identity (tokens, motion…)
  ROADMAP.md          phased plan of features and apps
  BUILD.md            full build and customisation guide
```

## License

Original code, configurations, and assets in this repository are
released under the MIT license (see `LICENSE`). All Arch Linux
packages keep their respective upstream licenses. WilOS is an
independent project and is **not** affiliated with Arch Linux,
Apple, or Microsoft.
