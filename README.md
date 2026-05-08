# WilOS

WilOS is a from-scratch operating system written in C and assembly.
The long-term goal is feature parity with Microsoft Windows (and its
announced future evolutions) under a fresh brand and a modern
glassmorphism / acrylic visual identity.

This repository contains the **kernel foundation** (phase 0). It boots
on i686 hardware and inside QEMU through GRUB (multiboot 1), brings up
the CPU, memory management, interrupts, a few drivers, and an in-kernel
debug shell. It is the substrate on which every later phase
(filesystems, userland, GUI compositor, application suite) is built.

## Quick start

Requirements on the build host:

- `i686-elf-gcc` / `i686-elf-ld` cross compiler (or a host toolchain
  capable of producing 32-bit freestanding ELF — see `docs/BUILD.md`)
- `nasm`
- `grub-mkrescue` and `xorriso` (for the bootable ISO)
- `qemu-system-i386` (to run)

```sh
make            # build kernel/wilos.elf
make iso        # build wilos.iso
make run        # boot the ISO in QEMU
```

On boot you land in the WilOS kernel shell. Type `help` to list the
built-in commands.

## Repository layout

```
boot/              multiboot header + early entry (asm)
kernel/
  arch/i386/       CPU bring-up: GDT, IDT, ISR stubs, paging, ports
  drivers/         VGA text, serial (COM1), PS/2 keyboard, PIT
  mm/              physical memory manager, kernel heap
  lib/             freestanding libc subset (string, printf, panic)
  shell/           in-kernel debug shell
  include/wilos/   public kernel headers
  kernel.c         kmain entry point
  linker.ld        kernel link script
grub/              grub.cfg used when packaging the ISO
docs/              architecture, roadmap, design system, build notes
scripts/           helper scripts (ISO packaging, run helpers)
```

## Status

Phase 0 — kernel foundation — is what ships in this commit. Everything
beyond (VFS, FAT/NTFS-like FS, ELF userland loader, syscalls,
networking stack, compositor, glassmorphism shell, application suite,
package manager, AI integration) is described in
[`docs/ROADMAP.md`](docs/ROADMAP.md). The visual identity that the
future GUI will implement is captured in
[`docs/DESIGN.md`](docs/DESIGN.md).

## License

Original code in this repository is released under the MIT license.
WilOS is an independent project and is **not** affiliated with, endorsed
by, or derived from Microsoft Windows.
