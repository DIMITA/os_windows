#include <wilos/idt.h>
#include <wilos/ports.h>
#include <wilos/types.h>

static isr_handler_t irq_handlers[16];

void irq_register(uint8_t irq, isr_handler_t h)
{
    if (irq < 16) irq_handlers[irq] = h;
}

void irq_dispatch(registers_t *r)
{
    uint32_t irq = r->int_no - 32;

    if (irq < 16 && irq_handlers[irq])
        irq_handlers[irq](r);

    /* End-of-interrupt to PIC. */
    if (irq >= 8) outb(0xA0, 0x20);
    outb(0x20, 0x20);
}
