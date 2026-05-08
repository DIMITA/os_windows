#include <wilos/panic.h>
#include <wilos/printf.h>
#include <wilos/vga.h>

__attribute__((noreturn))
void panic(const char *fmt, ...)
{
    vga_set_color(VGA_WHITE, VGA_RED);
    kprintf("\n[WilOS PANIC] ");
    va_list ap;
    va_start(ap, fmt);
    kvprintf(fmt, ap);
    va_end(ap);
    kprintf("\nSystem halted.\n");

    __asm__ volatile ("cli");
    for (;;) __asm__ volatile ("hlt");
}
