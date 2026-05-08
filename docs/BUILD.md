# Building WilOS

## Toolchain

A dedicated `i686-elf` cross compiler is the recommended way to build
WilOS. On Linux you can use a host toolchain with `gcc-multilib`
installed, which is what the default `Makefile` falls back to.

### Debian / Ubuntu

```sh
sudo apt-get install build-essential nasm gcc-multilib \
                     grub-pc-bin grub-common xorriso \
                     qemu-system-x86
```

### macOS (Homebrew)

```sh
brew install x86_64-elf-gcc nasm xorriso qemu i686-elf-gcc
```

If `i686-elf-gcc` is not in your `PATH`, override the prefix:

```sh
make CROSS=i686-elf-
```

## Targets

| Command           | Result                                              |
|-------------------|-----------------------------------------------------|
| `make`            | builds `build/wilos.elf` (multiboot kernel)         |
| `make iso`        | packages `wilos.iso` via `grub-mkrescue`            |
| `make run`        | boots the ISO in QEMU with serial-on-stdio          |
| `make run-kernel` | boots the kernel directly with `qemu -kernel`       |
| `make clean`      | wipes `build/` and `wilos.iso`                      |

## Verifying a build

```sh
grub-file --is-x86-multiboot build/wilos.elf && echo OK
```

A correctly built kernel will print `OK`. Booting it should land on
the WilOS banner and the kernel shell prompt.

## Running on real hardware

`wilos.iso` is bootable on any BIOS/UEFI x86 machine that supports
GRUB legacy boot. Burn it to a USB stick:

```sh
sudo dd if=wilos.iso of=/dev/sdX bs=4M status=progress conv=fsync
```

There are no graphics or USB drivers yet, so you will need a serial
console or an old PS/2 keyboard to interact.
