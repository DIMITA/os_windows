#include <wilos/keyboard.h>
#include <wilos/idt.h>
#include <wilos/ports.h>
#include <wilos/types.h>

#define BUF_SIZE 256

static volatile char buf[BUF_SIZE];
static volatile size_t head, tail;
static bool shift_down;
static bool caps_lock;

static const char map_lower[128] = {
    0,  27, '1','2','3','4','5','6','7','8','9','0','-','=','\b',
    '\t','q','w','e','r','t','y','u','i','o','p','[',']','\n',
    0,  'a','s','d','f','g','h','j','k','l',';','\'','`',
    0,  '\\','z','x','c','v','b','n','m',',','.','/',
    0,  '*', 0, ' ',
};

static const char map_upper[128] = {
    0,  27, '!','@','#','$','%','^','&','*','(',')','_','+','\b',
    '\t','Q','W','E','R','T','Y','U','I','O','P','{','}','\n',
    0,  'A','S','D','F','G','H','J','K','L',':','"','~',
    0,  '|','Z','X','C','V','B','N','M','<','>','?',
    0,  '*', 0, ' ',
};

static void buf_push(char c)
{
    size_t next = (head + 1) % BUF_SIZE;
    if (next != tail) {
        buf[head] = c;
        head = next;
    }
}

static void on_irq1(registers_t *r)
{
    (void)r;
    uint8_t sc = inb(0x60);

    /* Modifier handling. */
    if (sc == 0x2A || sc == 0x36) { shift_down = true; return; }
    if (sc == 0xAA || sc == 0xB6) { shift_down = false; return; }
    if (sc == 0x3A) { caps_lock = !caps_lock; return; }

    if (sc & 0x80) return;        /* key release */
    if (sc >= 128) return;

    bool upper = shift_down ^ caps_lock;
    char c = upper ? map_upper[sc] : map_lower[sc];
    if (c) buf_push(c);
}

void keyboard_init(void)
{
    head = tail = 0;
    irq_register(1, on_irq1);
}

char keyboard_getc(void)
{
    while (head == tail) {
        __asm__ volatile ("hlt");
    }
    char c = buf[tail];
    tail = (tail + 1) % BUF_SIZE;
    return c;
}
