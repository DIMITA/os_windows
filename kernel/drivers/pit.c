#include <wilos/pit.h>
#include <wilos/idt.h>
#include <wilos/ports.h>
#include <wilos/types.h>

#define PIT_CHANNEL0 0x40
#define PIT_COMMAND  0x43
#define PIT_BASE_HZ  1193182U

static volatile uint64_t ticks;
static uint32_t          frequency_hz;

static void on_irq0(registers_t *r)
{
    (void)r;
    ticks++;
}

void pit_init(uint32_t hz)
{
    frequency_hz = hz ? hz : 100;
    uint32_t divisor = PIT_BASE_HZ / frequency_hz;

    outb(PIT_COMMAND, 0x36);
    outb(PIT_CHANNEL0, divisor & 0xFF);
    outb(PIT_CHANNEL0, (divisor >> 8) & 0xFF);

    irq_register(0, on_irq0);
}

uint64_t pit_ticks(void)
{
    return ticks;
}

void pit_sleep_ms(uint32_t ms)
{
    uint64_t target = ticks + ((uint64_t)ms * frequency_hz) / 1000;
    while (ticks < target) {
        __asm__ volatile ("hlt");
    }
}
