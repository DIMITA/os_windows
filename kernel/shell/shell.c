#include <wilos/shell.h>
#include <wilos/printf.h>
#include <wilos/keyboard.h>
#include <wilos/vga.h>
#include <wilos/string.h>
#include <wilos/pmm.h>
#include <wilos/heap.h>
#include <wilos/pit.h>
#include <wilos/ports.h>
#include <wilos/panic.h>
#include <wilos/types.h>

#define LINE_MAX 128

static void prompt(void)
{
    vga_set_color(VGA_LIGHT_CYAN, VGA_BLACK);
    kprintf("WilOS");
    vga_set_color(VGA_LIGHT_GREY, VGA_BLACK);
    kprintf(":~$ ");
}

static void read_line(char *buf, size_t cap)
{
    size_t n = 0;
    for (;;) {
        char c = keyboard_getc();
        if (!c) continue;
        if (c == '\n') {
            buf[n] = '\0';
            kprintf("\n");
            return;
        }
        if (c == '\b') {
            if (n) { n--; kprintf("\b"); }
            continue;
        }
        if (n + 1 < cap) {
            buf[n++] = c;
            char s[2] = { c, 0 };
            kprintf("%s", s);
        }
    }
}

static void cmd_help(void)
{
    kprintf("Built-in commands:\n");
    kprintf("  help      show this list\n");
    kprintf("  about     about WilOS\n");
    kprintf("  clear     clear the screen\n");
    kprintf("  mem       print memory statistics\n");
    kprintf("  uptime    print system uptime\n");
    kprintf("  echo X    print X\n");
    kprintf("  panic     trigger a kernel panic (debug)\n");
    kprintf("  reboot    reboot the machine\n");
}

static void cmd_about(void)
{
    vga_set_color(VGA_LIGHT_MAGENTA, VGA_BLACK);
    kprintf("\n  WilOS\n");
    vga_set_color(VGA_LIGHT_GREY, VGA_BLACK);
    kprintf("  A modern, glassmorphic operating system. Phase 0.\n");
    kprintf("  Kernel: i686 multiboot, monolithic with planned hybrid split.\n");
    kprintf("  See docs/ROADMAP.md for the path to feature parity.\n\n");
}

static void cmd_mem(void)
{
    size_t total = pmm_total_pages();
    size_t used  = pmm_used_pages();
    size_t free  = total - used;
    kprintf("Physical memory:\n");
    kprintf("  total: %u pages (%u KiB)\n", (unsigned)total, (unsigned)(total * 4));
    kprintf("  used:  %u pages (%u KiB)\n", (unsigned)used,  (unsigned)(used  * 4));
    kprintf("  free:  %u pages (%u KiB)\n", (unsigned)free,  (unsigned)(free  * 4));
    kprintf("Heap:\n");
    kprintf("  used:  %u / %u bytes\n", (unsigned)heap_used(), (unsigned)heap_total());
}

static void cmd_uptime(void)
{
    uint64_t t  = pit_ticks();
    uint32_t s  = (uint32_t)(t / 100);    /* PIT runs at 100 Hz */
    uint32_t ms = (uint32_t)((t % 100) * 10);
    kprintf("up %u.%03u s (%u ticks)\n", s, ms, (unsigned)t);
}

static void cmd_reboot(void)
{
    /* Triple-fault via the 8042 reset line. */
    while (inb(0x64) & 0x02) { }
    outb(0x64, 0xFE);
    for (;;) __asm__ volatile ("hlt");
}

static void execute(char *line)
{
    while (*line == ' ') line++;
    if (!*line) return;

    char *arg = line;
    while (*arg && *arg != ' ') arg++;
    if (*arg) { *arg++ = '\0'; while (*arg == ' ') arg++; }

    if      (!strcmp(line, "help"))    cmd_help();
    else if (!strcmp(line, "about"))   cmd_about();
    else if (!strcmp(line, "clear"))   vga_clear();
    else if (!strcmp(line, "mem"))     cmd_mem();
    else if (!strcmp(line, "uptime"))  cmd_uptime();
    else if (!strcmp(line, "echo"))    kprintf("%s\n", arg);
    else if (!strcmp(line, "reboot"))  cmd_reboot();
    else if (!strcmp(line, "panic"))   panic("user-requested panic");
    else                               kprintf("unknown command: %s\n", line);
}

__attribute__((noreturn))
void shell_run(void)
{
    char line[LINE_MAX];
    cmd_about();
    for (;;) {
        prompt();
        read_line(line, sizeof(line));
        execute(line);
    }
}
