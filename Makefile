# WilOS top-level build
#
# Targets:
#   make            - build kernel ELF
#   make iso        - produce bootable wilos.iso (requires grub-mkrescue)
#   make run        - boot the ISO in QEMU
#   make run-kernel - boot the kernel directly with qemu -kernel
#   make clean      - remove build artifacts

CROSS    ?= i686-elf-
CC       := $(CROSS)gcc
LD       := $(CROSS)ld
AS       := nasm

# Fall back to the host compiler if no cross compiler is available.
# This still produces a freestanding 32-bit ELF on most Linux hosts.
ifeq ($(shell command -v $(CC) 2>/dev/null),)
  CC := gcc
  LD := ld
endif

BUILD    := build
KERNEL   := $(BUILD)/wilos.elf
ISO      := wilos.iso

INCLUDES := -Ikernel/include

CFLAGS   := -std=gnu11 -ffreestanding -fno-stack-protector -fno-pic \
            -fno-builtin -fno-omit-frame-pointer -m32 \
            -Wall -Wextra -Wno-unused-parameter -O2 -g \
            $(INCLUDES)
ASFLAGS  := -f elf32
LDFLAGS  := -m elf_i386 -nostdlib -T kernel/linker.ld
LIBGCC   := $(shell $(CC) -m32 -print-libgcc-file-name 2>/dev/null)

C_SOURCES := \
  kernel/kernel.c \
  kernel/arch/i386/gdt.c \
  kernel/arch/i386/idt.c \
  kernel/arch/i386/isr.c \
  kernel/arch/i386/irq.c \
  kernel/arch/i386/paging.c \
  kernel/drivers/vga.c \
  kernel/drivers/serial.c \
  kernel/drivers/keyboard.c \
  kernel/drivers/pit.c \
  kernel/drivers/ata.c \
  kernel/fs/part.c \
  kernel/fs/fat.c \
  kernel/mm/pmm.c \
  kernel/mm/heap.c \
  kernel/lib/string.c \
  kernel/lib/printf.c \
  kernel/lib/panic.c \
  kernel/shell/shell.c

ASM_SOURCES := \
  boot/multiboot.S \
  kernel/arch/i386/gdt_flush.S \
  kernel/arch/i386/idt_load.S \
  kernel/arch/i386/isr_stubs.S \
  kernel/arch/i386/irq_stubs.S

C_OBJECTS   := $(patsubst %.c,$(BUILD)/%.o,$(C_SOURCES))
ASM_OBJECTS := $(patsubst %.S,$(BUILD)/%.o,$(ASM_SOURCES))
OBJECTS     := $(ASM_OBJECTS) $(C_OBJECTS)

.PHONY: all iso run run-kernel clean

all: $(KERNEL)

$(KERNEL): $(OBJECTS) kernel/linker.ld
	@mkdir -p $(dir $@)
	$(LD) $(LDFLAGS) -o $@ $(OBJECTS) $(LIBGCC)

$(BUILD)/%.o: %.c
	@mkdir -p $(dir $@)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/%.o: %.S
	@mkdir -p $(dir $@)
	$(AS) $(ASFLAGS) $< -o $@

iso: $(ISO)

$(ISO): $(KERNEL) grub/grub.cfg
	@mkdir -p $(BUILD)/iso/boot/grub
	cp $(KERNEL) $(BUILD)/iso/boot/wilos.elf
	cp grub/grub.cfg $(BUILD)/iso/boot/grub/grub.cfg
	grub-mkrescue -o $@ $(BUILD)/iso 2>/dev/null

run: $(ISO)
	qemu-system-i386 -cdrom $(ISO) -serial stdio -m 128

run-kernel: $(KERNEL)
	qemu-system-i386 -kernel $(KERNEL) -serial stdio -m 128

clean:
	rm -rf $(BUILD) $(ISO)
