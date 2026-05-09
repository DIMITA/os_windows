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
#include <wilos/ata.h>
#include <wilos/part.h>
#include <wilos/fat.h>
#include <wilos/types.h>

#define LINE_MAX 256

static fat_fs_t mounted_fs;
static bool     have_mount;
static size_t   mounted_drive;
static size_t   mounted_part;

static void prompt(void)
{
    vga_set_color(VGA_LIGHT_CYAN, VGA_BLACK);
    kprintf("WilOS");
    vga_set_color(VGA_LIGHT_GREY, VGA_BLACK);
    if (have_mount) kprintf("[%u:%u]", (unsigned)mounted_drive, (unsigned)mounted_part);
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

static int parse_uint(const char *s, unsigned *out)
{
    if (!*s) return -1;
    unsigned v = 0;
    while (*s >= '0' && *s <= '9') { v = v * 10 + (*s - '0'); s++; }
    if (*s && *s != ' ') return -1;
    *out = v;
    return 0;
}

static void cmd_help(void)
{
    kprintf("Built-in commands:\n");
    kprintf("  help            show this list\n");
    kprintf("  about           about WilOS\n");
    kprintf("  clear           clear the screen\n");
    kprintf("  mem             memory statistics\n");
    kprintf("  uptime          system uptime\n");
    kprintf("  echo X          print X\n");
    kprintf("  disks           list ATA drives\n");
    kprintf("  parts D         list partitions on drive D\n");
    kprintf("  mount D P       mount FAT partition P of drive D\n");
    kprintf("  umount          unmount current FAT volume\n");
    kprintf("  ls [PATH]       list directory on the mounted volume\n");
    kprintf("  cat PATH        print a text file\n");
    kprintf("  panic           trigger a kernel panic (debug)\n");
    kprintf("  reboot          reboot the machine\n");
}

static void cmd_about(void)
{
    vga_set_color(VGA_LIGHT_MAGENTA, VGA_BLACK);
    kprintf("\n  WilOS\n");
    vga_set_color(VGA_LIGHT_GREY, VGA_BLACK);
    kprintf("  A modern, glassmorphic operating system. Phase 1.0.\n");
    kprintf("  Kernel: i686 multiboot, monolithic with planned hybrid split.\n");
    kprintf("  Disk: ATA PIO + MBR/GPT + FAT16/FAT32 read-only.\n");
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
    uint32_t s  = (uint32_t)(t / 100);
    uint32_t ms = (uint32_t)((t % 100) * 10);
    kprintf("up %u.%03u s (%u ticks)\n", s, ms, (unsigned)t);
}

static void cmd_reboot(void)
{
    while (inb(0x64) & 0x02) { }
    outb(0x64, 0xFE);
    for (;;) __asm__ volatile ("hlt");
}

static void cmd_disks(void)
{
    size_t n = ata_drive_count();
    if (!n) { kprintf("no ATA drives detected\n"); return; }
    kprintf(" #  type   sectors          size       model\n");
    for (size_t i = 0; i < n; i++) {
        const ata_drive_t *d = ata_drive(i);
        if (!d) continue;
        uint32_t mib = (uint32_t)((d->sectors * 512ULL) / (1024 * 1024));
        kprintf(" %u  %s  %u  %u MiB  %s\n",
                (unsigned)i,
                d->atapi ? "ATAPI" : "ATA  ",
                (unsigned)d->sectors,
                (unsigned)mib,
                d->model);
    }
}

static void cmd_parts(const char *arg)
{
    unsigned drive;
    if (parse_uint(arg, &drive) < 0) { kprintf("usage: parts <drive>\n"); return; }
    if (drive >= ata_drive_count()) { kprintf("no such drive\n"); return; }

    part_table_t pt;
    part_scan(drive, &pt);
    kprintf("scheme: %s, %u partitions\n", part_scheme_name(pt.scheme), (unsigned)pt.count);
    for (size_t i = 0; i < pt.count; i++) {
        const partition_t *p = &pt.parts[i];
        if (!p->used) continue;
        uint32_t mib = (uint32_t)((p->lba_count * 512ULL) / (1024 * 1024));
        kprintf("  %u  start=%u  count=%u (%u MiB)  type=%s",
                (unsigned)i,
                (unsigned)p->lba_start,
                (unsigned)p->lba_count,
                (unsigned)mib,
                p->type_name);
        if (p->gpt_name[0]) kprintf("  name=\"%s\"", p->gpt_name);
        kprintf("\n");
    }
}

static void cmd_mount(const char *arg)
{
    unsigned drive, part;
    char *sp = (char *)arg;
    if (parse_uint(sp, &drive) < 0) { kprintf("usage: mount <drive> <part>\n"); return; }
    while (*sp && *sp != ' ') sp++;
    while (*sp == ' ') sp++;
    if (parse_uint(sp, &part) < 0)  { kprintf("usage: mount <drive> <part>\n"); return; }

    if (drive >= ata_drive_count()) { kprintf("no such drive\n"); return; }
    part_table_t pt;
    part_scan(drive, &pt);
    if (part >= pt.count || !pt.parts[part].used) { kprintf("no such partition\n"); return; }

    if (fat_mount(&mounted_fs, drive, pt.parts[part].lba_start,
                  pt.parts[part].lba_count) < 0) {
        kprintf("mount failed: not a FAT16/FAT32 filesystem\n");
        have_mount = false;
        return;
    }
    have_mount    = true;
    mounted_drive = drive;
    mounted_part  = part;
    kprintf("mounted FAT%s on %u:%u\n",
            mounted_fs.type == FAT_TYPE_32 ? "32" : "16",
            (unsigned)drive, (unsigned)part);
}

static void cmd_umount(void)
{
    if (!have_mount) { kprintf("nothing mounted\n"); return; }
    have_mount = false;
    memset(&mounted_fs, 0, sizeof(mounted_fs));
}

static void cmd_ls(const char *path)
{
    if (!have_mount) { kprintf("nothing mounted; use `mount D P`\n"); return; }

    fat_dir_t dir;
    fat_entry_t e;

    if (path && *path) {
        if (fat_lookup(&mounted_fs, path, &e) < 0) {
            kprintf("not found: %s\n", path);
            return;
        }
        if (!e.is_dir) {
            kprintf("%-40s %10u\n", e.name, (unsigned)e.size);
            return;
        }
        dir.fs         = &mounted_fs;
        dir.cluster    = e.cluster;
        dir.offset     = 0;
        dir.fat16_root = false;
    } else {
        fat_open_root(&mounted_fs, &dir);
    }

    while (fat_readdir(&dir, &e) > 0) {
        if (e.is_dir) kprintf("  <DIR>  %s\n", e.name);
        else          kprintf("  %7u %s\n", (unsigned)e.size, e.name);
    }
}

static void cmd_cat(const char *path)
{
    if (!have_mount) { kprintf("nothing mounted\n"); return; }
    if (!path || !*path) { kprintf("usage: cat <path>\n"); return; }

    fat_entry_t e;
    if (fat_lookup(&mounted_fs, path, &e) < 0) { kprintf("not found\n"); return; }
    if (e.is_dir) { kprintf("is a directory\n"); return; }

    static uint8_t buf[2048];
    uint32_t off = 0;
    while (off < e.size) {
        uint32_t want = e.size - off;
        if (want > sizeof(buf)) want = sizeof(buf);
        int got = fat_read_file(&mounted_fs, &e, off, buf, want);
        if (got <= 0) break;
        for (int i = 0; i < got; i++) {
            char c = (char)buf[i];
            if (c == '\r') continue;
            if (c == '\n' || c == '\t' || (c >= 0x20 && c < 0x7F)) vga_putc(c);
            else vga_putc('.');
        }
        off += got;
    }
    kprintf("\n");
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
    else if (!strcmp(line, "disks"))   cmd_disks();
    else if (!strcmp(line, "parts"))   cmd_parts(arg);
    else if (!strcmp(line, "mount"))   cmd_mount(arg);
    else if (!strcmp(line, "umount"))  cmd_umount();
    else if (!strcmp(line, "ls"))      cmd_ls(arg);
    else if (!strcmp(line, "cat"))     cmd_cat(arg);
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
