#include <wilos/vga.h>
#include <wilos/ports.h>
#include <wilos/types.h>

#define VGA_WIDTH  80
#define VGA_HEIGHT 25
#define VGA_MEM    ((volatile uint16_t *)0xB8000)

static size_t  cursor_x;
static size_t  cursor_y;
static uint8_t color;

static uint16_t make_cell(char c, uint8_t color)
{
    return (uint16_t)c | ((uint16_t)color << 8);
}

static void update_hw_cursor(void)
{
    uint16_t pos = cursor_y * VGA_WIDTH + cursor_x;
    outb(0x3D4, 14); outb(0x3D5, (pos >> 8) & 0xFF);
    outb(0x3D4, 15); outb(0x3D5, pos & 0xFF);
}

static void scroll_if_needed(void)
{
    if (cursor_y < VGA_HEIGHT) return;

    for (size_t y = 1; y < VGA_HEIGHT; y++)
        for (size_t x = 0; x < VGA_WIDTH; x++)
            VGA_MEM[(y - 1) * VGA_WIDTH + x] = VGA_MEM[y * VGA_WIDTH + x];

    for (size_t x = 0; x < VGA_WIDTH; x++)
        VGA_MEM[(VGA_HEIGHT - 1) * VGA_WIDTH + x] = make_cell(' ', color);

    cursor_y = VGA_HEIGHT - 1;
}

void vga_set_color(uint8_t fg, uint8_t bg)
{
    color = (bg << 4) | (fg & 0x0F);
}

void vga_clear(void)
{
    for (size_t y = 0; y < VGA_HEIGHT; y++)
        for (size_t x = 0; x < VGA_WIDTH; x++)
            VGA_MEM[y * VGA_WIDTH + x] = make_cell(' ', color);

    cursor_x = cursor_y = 0;
    update_hw_cursor();
}

void vga_init(void)
{
    vga_set_color(VGA_LIGHT_CYAN, VGA_BLACK);
    vga_clear();
}

void vga_putc(char c)
{
    if (c == '\n') {
        cursor_x = 0;
        cursor_y++;
    } else if (c == '\r') {
        cursor_x = 0;
    } else if (c == '\b') {
        if (cursor_x > 0) {
            cursor_x--;
            VGA_MEM[cursor_y * VGA_WIDTH + cursor_x] = make_cell(' ', color);
        }
    } else if (c == '\t') {
        cursor_x = (cursor_x + 8) & ~7;
    } else {
        VGA_MEM[cursor_y * VGA_WIDTH + cursor_x] = make_cell(c, color);
        cursor_x++;
    }

    if (cursor_x >= VGA_WIDTH) {
        cursor_x = 0;
        cursor_y++;
    }
    scroll_if_needed();
    update_hw_cursor();
}

void vga_write(const char *s)
{
    while (*s) vga_putc(*s++);
}
