#include <wilos/printf.h>
#include <wilos/vga.h>
#include <wilos/serial.h>
#include <wilos/string.h>

static void emit(char c)
{
    vga_putc(c);
    serial_putc(c);
    if (c == '\n') serial_putc('\r');
}

static void emit_str(const char *s)
{
    while (*s) emit(*s++);
}

static void emit_uint(unsigned long long v, unsigned base, int upper, int width, int zero)
{
    char buf[32];
    int  n = 0;
    const char *digits = upper ? "0123456789ABCDEF" : "0123456789abcdef";

    if (v == 0) buf[n++] = '0';
    while (v) { buf[n++] = digits[v % base]; v /= base; }

    while (n < width) buf[n++] = zero ? '0' : ' ';
    while (n--) emit(buf[n]);
}

static void emit_int(long long v, int width, int zero)
{
    if (v < 0) { emit('-'); v = -v; if (width > 0) width--; }
    emit_uint((unsigned long long)v, 10, 0, width, zero);
}

int kvprintf(const char *fmt, va_list ap)
{
    int written = 0;
    while (*fmt) {
        if (*fmt != '%') { emit(*fmt++); written++; continue; }
        fmt++;

        int zero = 0, width = 0;
        if (*fmt == '0') { zero = 1; fmt++; }
        while (*fmt >= '0' && *fmt <= '9') { width = width * 10 + (*fmt - '0'); fmt++; }

        switch (*fmt) {
        case 'c': emit((char)va_arg(ap, int)); written++; break;
        case 's': {
            const char *s = va_arg(ap, const char *);
            if (!s) s = "(null)";
            while (*s) { emit(*s++); written++; }
            break;
        }
        case 'd': case 'i':
            emit_int(va_arg(ap, int), width, zero); break;
        case 'u':
            emit_uint(va_arg(ap, unsigned int), 10, 0, width, zero); break;
        case 'x':
            emit_uint(va_arg(ap, unsigned int), 16, 0, width, zero); break;
        case 'X':
            emit_uint(va_arg(ap, unsigned int), 16, 1, width, zero); break;
        case 'p':
            emit_str("0x");
            emit_uint((uintptr_t)va_arg(ap, void *), 16, 0, 8, 1); break;
        case '%': emit('%'); written++; break;
        default:  emit('%'); emit(*fmt); written += 2; break;
        }
        fmt++;
    }
    return written;
}

int kprintf(const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    int n = kvprintf(fmt, ap);
    va_end(ap);
    return n;
}
