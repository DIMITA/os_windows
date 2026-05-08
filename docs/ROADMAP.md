# WilOS Roadmap

WilOS aims for full feature parity with Microsoft Windows — including
its announced future evolutions — under a fresh brand and a modern
glassmorphic identity. This is a multi-decade undertaking; the
roadmap below sequences the work into phases that each deliver
something useful on their own.

## Phase 0 — Kernel foundation (this commit)

- [x] Multiboot 1 boot, GRUB ISO packaging
- [x] GDT, IDT, ISR/IRQ stubs, 8259 PIC remap
- [x] PIT timer, PS/2 keyboard, COM1 serial logging, VGA text driver
- [x] Bitmap physical memory manager driven by the multiboot mmap
- [x] First-fit kernel heap (`kmalloc` / `kfree`)
- [x] Identity-mapped paging for the low 16 MiB
- [x] Freestanding libk: `string.h`, `printf`, `panic`
- [x] In-kernel debug shell (`help`, `mem`, `uptime`, `reboot`, …)

## Phase 1 — Real OS kernel

- [ ] Higher-half kernel at `0xC0000000`, demand-paged kernel heap
- [ ] Buddy/PMM rework + slab allocator
- [ ] Per-process page directories, copy-on-write
- [ ] Preemptive round-robin scheduler, kernel threads
- [ ] System call interface (`int 0x80` then `syscall`)
- [ ] ELF32/ELF64 user binary loader
- [ ] VFS layer with `ramfs`, `devfs`, `tmpfs`
- [ ] FAT32 driver (read+write), then a journaled native FS
- [ ] x86_64 port (long mode trampoline, new linker layout)
- [ ] ACPI bring-up, APIC instead of PIC, HPET instead of PIT

## Phase 2 — Userland & graphics

- [ ] `init` process, runlevel-style service manager
- [ ] `libwilos` (libc-equivalent) and a port of `musl`
- [ ] Shell (`wsh`) with pipes, redirection, job control
- [ ] Framebuffer driver via VBE / GOP
- [ ] **Wil-Comp** compositor: Wayland-like protocol, GPU-accelerated
      via Mesa once a GPU driver lands
- [ ] Input stack (libinput-equivalent)
- [ ] Glassmorphic shell (taskbar, start menu, action centre, widgets)
      following [`docs/DESIGN.md`](DESIGN.md)
- [ ] Sound stack (PulseAudio-style mixer)

## Phase 3 — Networking

- [ ] e1000 / virtio-net drivers
- [ ] Native TCP/IP stack
- [ ] DNS, DHCP client, NTP client
- [ ] TLS via BoringSSL or rustls
- [ ] HTTP/2/3 client and a Chromium-derived browser ("WilEdge")

## Phase 4 — Application platform

- [ ] **WilNT API**: source-compatible re-implementation of the Win32
      surface that matters most (file I/O, GDI/Direct2D-equivalent,
      window management). The goal is a clear migration path for
      Windows software, **not** binary compatibility on day one.
- [ ] **WilStore**: signed package manager with sandbox manifests,
      delta updates, and per-app permissions.
- [ ] **WilML** runtime: on-device AI assistant integrated into the
      shell (search, dictation, code actions).
- [ ] First-party apps: Files, Edge-like browser, Mail, Calendar,
      Terminal, Settings, Photos, Media Player, Notepad, Paint,
      Calculator, Snipping Tool, Office-style suite.

## Phase 5 — Compatibility & ecosystem

- [ ] **WilWow**: PE/COFF loader + Win32 ABI shim able to run a
      curated set of unmodified Windows applications (think Wine, but
      first-party).
- [ ] WSL-equivalent: run a Linux user space inside WilOS.
- [ ] Hyper-V-style virtualisation via KVM/HVF-equivalent.
- [ ] Hardware partner program: signed drivers, WHQL-equivalent lab.

## Phase 6 — Future evolutions tracked from Windows

These mirror the public Windows roadmap so the WilOS surface stays
on par with what users expect from a modern Windows release:

- [ ] AI-first shell: contextual Copilot-equivalent on every surface
- [ ] Cloud PC: stream a remote WilOS session to any device
- [ ] Passwordless-by-default authentication (WebAuthn, passkeys)
- [ ] Sudo-style elevation flow built into the desktop
- [ ] Pluton-style hardware-rooted security
- [ ] Recall-equivalent semantic timeline (opt-in, on-device only)
- [ ] Native ARM64 support and translation layer for x86_64 apps
- [ ] Tabbed shell everywhere (Files, Terminal, Notepad, Settings)
- [ ] Modern Notepad with autosave + AI rewrite
- [ ] Snap layouts / FancyZones built in
- [ ] Widgets dashboard with developer SDK
- [ ] Phone Link-equivalent across Android and iOS
- [ ] DirectStorage-equivalent fast-path for games
- [ ] HDR + auto-HDR pipeline in the compositor
- [ ] Native Linux GUI app support inside the desktop session

## Non-goals

- Bug-for-bug compatibility with Windows internals (NT kernel,
  registry hive layout, undocumented APIs).
- Re-using any Microsoft source code or proprietary assets.
- Shipping closed-source by default — the base system is MIT, third-
  party apps may use any license.
