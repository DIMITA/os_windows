#include <wilos/types.h>
#include <wilos/multiboot.h>
#include <wilos/vga.h>
#include <wilos/serial.h>
#include <wilos/printf.h>
#include <wilos/gdt.h>
#include <wilos/idt.h>
#include <wilos/keyboard.h>
#include <wilos/pit.h>
#include <wilos/pmm.h>
#include <wilos/heap.h>
#include <wilos/paging.h>
#include <wilos/shell.h>
#include <wilos/panic.h>

extern uint8_t __kernel_end[];

static void banner(void)
{
    vga_set_color(VGA_LIGHT_CYAN, VGA_BLACK);
    kprintf("\n");
    kprintf("  __        ___ _  ___  ____\n");
    kprintf("  \\ \\      / (_) |/ _ \\/ ___|\n");
    kprintf("   \\ \\ /\\ / /| | | | | \\___ \\\n");
    kprintf("    \\ V  V / | | | |_| |___) |\n");
    kprintf("     \\_/\\_/  |_|_|\\___/|____/\n\n");
    vga_set_color(VGA_LIGHT_GREY, VGA_BLACK);
    kprintf("  WilOS phase 0 kernel booted.\n\n");
}

void kmain(uint32_t magic, multiboot_info_t *mbi)
{
    serial_init();
    vga_init();

    kprintf("WilOS booting...\n");

    if (magic != MULTIBOOT_BOOTLOADER_MAGIC) {
        kprintf("warning: unexpected multiboot magic 0x%x\n", magic);
    }

    gdt_init();        kprintf("[ok] gdt\n");
    idt_init();        kprintf("[ok] idt + pic remap\n");
    pit_init(100);     kprintf("[ok] pit @ 100 Hz\n");
    keyboard_init();   kprintf("[ok] ps/2 keyboard\n");

    pmm_init(mbi, (uintptr_t)__kernel_end);
    kprintf("[ok] pmm: %u pages total\n", (unsigned)pmm_total_pages());

    /* Carve a 1 MiB kernel heap from the PMM. */
    void *heap_base = NULL;
    size_t heap_pages = 256;        /* 1 MiB */
    for (size_t i = 0; i < heap_pages; i++) {
        void *p = pmm_alloc_page();
        if (!p) panic("out of memory while building kernel heap");
        if (i == 0) heap_base = p;
    }
    heap_init((uintptr_t)heap_base, heap_pages * 4096);
    kprintf("[ok] kernel heap: %u KiB at %p\n",
            (unsigned)(heap_pages * 4), heap_base);

    paging_init();
    kprintf("[ok] paging (identity-mapped low 16 MiB)\n");

    banner();
    shell_run();
}
