#include <wilos/serial.h>
#include <wilos/ports.h>

void serial_init(void)
{
    outb(SERIAL_COM1 + 1, 0x00);   /* disable interrupts */
    outb(SERIAL_COM1 + 3, 0x80);   /* enable DLAB        */
    outb(SERIAL_COM1 + 0, 0x03);   /* divisor low (38400 baud) */
    outb(SERIAL_COM1 + 1, 0x00);   /* divisor high */
    outb(SERIAL_COM1 + 3, 0x03);   /* 8N1 */
    outb(SERIAL_COM1 + 2, 0xC7);   /* enable + clear FIFO */
    outb(SERIAL_COM1 + 4, 0x0B);   /* IRQs, RTS/DSR */
}

static int can_send(void)
{
    return inb(SERIAL_COM1 + 5) & 0x20;
}

void serial_putc(char c)
{
    while (!can_send()) { }
    outb(SERIAL_COM1, (uint8_t)c);
}

void serial_write(const char *s)
{
    while (*s) {
        if (*s == '\n') serial_putc('\r');
        serial_putc(*s++);
    }
}
