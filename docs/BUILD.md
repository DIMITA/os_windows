# Building WilOS Aurora

WilOS is built with `mkarchiso` from the `archiso` package. You need
to build on an **Arch Linux host** (real machine or VM) — `mkarchiso`
will not run correctly on other distributions.

## Host setup

```sh
sudo pacman -Syu
sudo pacman -S archiso imagemagick git
```

Optional (for testing the resulting ISO without writing it to a USB
stick):

```sh
sudo pacman -S qemu-desktop edk2-ovmf
```

## Build

```sh
git clone <this repo> wilos
cd wilos
sudo ./scripts/build-iso.sh
```

`mkarchiso` will:
1. Create a temporary work directory under `out/work/`
2. Bootstrap a base system per `distro/packages.x86_64`
3. Overlay everything in `distro/airootfs/` on top of it
4. Build the squashfs image
5. Wrap it in an ISO with the BIOS + UEFI boot configurations from
   `distro/syslinux/`, `distro/efiboot/`, `distro/grub/`

The resulting ISO is dropped in `out/wilos-YYYY.MM.DD-x86_64.iso`.

A full first build downloads ~1.5 GiB of packages and produces a
~2.5 GiB ISO. Subsequent builds reuse the pacman cache and are much
faster.

## Test in QEMU

UEFI:

```sh
qemu-system-x86_64 -enable-kvm -m 4G -smp 4 \
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/edk2/x64/OVMF_CODE.4m.fd \
    -drive if=pflash,format=raw,file=/var/lib/wilos/OVMF_VARS.4m.fd \
    -cdrom out/wilos-*.iso \
    -boot d
```

BIOS:

```sh
qemu-system-x86_64 -enable-kvm -m 4G -smp 4 \
    -cdrom out/wilos-*.iso -boot d
```

## Flash to USB

```sh
sudo ./scripts/flash-usb.sh out/wilos-*.iso /dev/sdX
```

The helper refuses to write to anything that looks like an internal
disk and requires you to type `FLASH sdX` to proceed. If you really
want to flash an internal disk (rare), use `dd` directly.

## Customisation

Most changes happen by editing files under
`distro/airootfs/etc/skel/.config/`. The whole directory is mirrored
into both the live session and the installed system, so edits are
applied uniformly.

### Adding a package

- For the **live ISO** (available the moment you boot the USB):
  add to `distro/packages.x86_64`.
- For the **installed system** (pacstrapped by `wilos-install`):
  add to `distro/airootfs/etc/wilos/install-packages.list`.
- For both: add to both files.

### Changing the wallpaper

Either:
- Drop your own `aurora.jpg` into
  `distro/airootfs/usr/share/backgrounds/wilos/`
- Or edit `scripts/make-wallpaper.sh` and rerun it.

Then change the path in the `swww img …` line of
`distro/airootfs/etc/skel/.config/hypr/hyprland.conf` if you renamed
the file.

### Tuning the look

The Aurora glassmorphism is driven by three places:

1. `hyprland.conf` `decoration { … }` — blur radius + passes,
   rounding, shadow, opacities.
2. `waybar/style.css` and `waybar/dock.css` — top bar and dock glass
   tints, accent gradients.
3. `wofi/style.css` — launcher glass card.

The colour palette and motion curves are documented in
[`docs/DESIGN.md`](DESIGN.md). Keep these three files in sync with
the design tokens — that's how the system stays visually coherent.
