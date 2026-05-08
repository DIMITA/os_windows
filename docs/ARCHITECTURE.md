# WilOS Architecture

This document describes the structure of the WilOS kernel as it stands
in phase 0 and the architectural choices that shape every later phase.

## Layering

```
+-------------------------------------------------------------+
|                     applications (future)                   |
+-------------------------------------------------------------+
|     glassmorphic shell  |  app frameworks  |  package mgr   |
+-------------------------------------------------------------+
|              compositor (Wil-Comp, Wayland-like)            |
+-------------------------------------------------------------+
|           userland services (init, login, audio, net)       |
+-------------------------------------------------------------+
|                           libwilos                          |
+-------------------------------------------------------------+
|                       syscall boundary                      |
+=============================================================+
|                     WilOS kernel (this repo)                |
|   shell  |  vfs  |  scheduler  |  ipc  |  drivers  |  mm    |
+-------------------------------------------------------------+
|     arch (i386 today, x86_64 + aarch64 in later phases)     |
+-------------------------------------------------------------+
|                 firmware / bootloader (GRUB)                |
+-------------------------------------------------------------+
```

The split between kernel space and user space follows a **hybrid**
model — long-running drivers (graphics, network) will eventually move
to userland in phase 2, while latency-critical paths (scheduler, VFS
core, IPC) stay in the kernel.

## Boot path

1. GRUB loads `/boot/wilos.elf` per the multiboot 1 spec at 1 MiB.
2. `boot/multiboot.S` sets up a 16 KiB boot stack and calls `kmain`
   with the multiboot magic and info pointer on the stack.
3. `kmain` (in `kernel/kernel.c`) initialises subsystems in order:
   serial → VGA → GDT → IDT/PIC → PIT → keyboard → PMM → heap →
   paging, then jumps into `shell_run`.

## Subsystems (phase 0)

| Subsystem | Files | Purpose |
|-----------|-------|---------|
| arch/i386 | `kernel/arch/i386/*` | CPU bring-up: GDT, IDT, ISR/IRQ stubs, PIC remap, identity paging |
| drivers   | `kernel/drivers/*`   | VGA text, COM1 serial, PS/2 keyboard, 8253 PIT |
| mm        | `kernel/mm/*`        | Bitmap PMM, first-fit kernel heap |
| lib       | `kernel/lib/*`       | freestanding libc subset, kprintf, panic |
| shell     | `kernel/shell/*`     | in-kernel debug shell |

## Memory map (phase 0)

```
0x00000000 - 0x000FFFFF   reserved (BIOS, low memory, VGA buffer)
0x00100000 - kernel_end   kernel image (text/rodata/data/bss)
kernel_end - +bitmap      PMM bitmap
+heap_base - +1 MiB       kernel heap (kmalloc / kfree)
above                     free pages, allocated by pmm_alloc_page
```

## Interrupt model

- Exceptions 0..31 are handled by `isr_dispatch` and call into
  registered C handlers via `isr_register`. Unhandled exceptions
  panic the kernel.
- IRQs 0..15 are remapped to vectors 32..47 by reprogramming the dual
  8259 PIC. Drivers register handlers with `irq_register`.
- The PIT (IRQ0) drives `pit_ticks` and the future scheduler.
- The PS/2 keyboard (IRQ1) feeds a small ring buffer consumed by
  `keyboard_getc`.

## Conventions

- Kernel source uses C11 (`-std=gnu11`), no libc, `-ffreestanding`.
- Public headers live under `kernel/include/wilos/` and are the only
  surface other subsystems are allowed to depend on.
- Anything that allocates memory must own the matching free path. The
  PMM and the heap track usage so the `mem` shell command stays
  truthful.
- Asm stubs are paired with C dispatchers — never put logic in asm
  unless the C compiler cannot express it.
