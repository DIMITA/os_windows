; WilOS multiboot 1 header + early entry.
;
; GRUB loads the kernel, jumps into protected mode, and transfers
; control to `_start`. We set up a small stack and call `kmain` in C.

MBALIGN     equ 1 << 0
MEMINFO     equ 1 << 1
FLAGS       equ MBALIGN | MEMINFO
MAGIC       equ 0x1BADB002
CHECKSUM    equ -(MAGIC + FLAGS)

section .multiboot
align 4
    dd MAGIC
    dd FLAGS
    dd CHECKSUM

section .bss
align 16
stack_bottom:
    resb 16384          ; 16 KiB boot stack
stack_top:

section .text
global _start
extern kmain

_start:
    mov esp, stack_top
    push ebx            ; multiboot info pointer
    push eax            ; multiboot magic
    cli
    call kmain

.hang:
    hlt
    jmp .hang
