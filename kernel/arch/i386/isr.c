#include <wilos/idt.h>
#include <wilos/printf.h>
#include <wilos/panic.h>
#include <wilos/types.h>

static isr_handler_t isr_handlers[32];

static const char *exception_names[] = {
    "Divide-by-zero",
    "Debug",
    "Non-maskable interrupt",
    "Breakpoint",
    "Overflow",
    "Bound range exceeded",
    "Invalid opcode",
    "Device not available",
    "Double fault",
    "Coprocessor segment overrun",
    "Invalid TSS",
    "Segment not present",
    "Stack-segment fault",
    "General protection fault",
    "Page fault",
    "Reserved",
    "x87 FPU error",
    "Alignment check",
    "Machine check",
    "SIMD FP exception",
    "Virtualization",
    "Control protection",
    "Reserved", "Reserved", "Reserved", "Reserved", "Reserved", "Reserved",
    "Hypervisor injection",
    "VMM communication",
    "Security",
    "Reserved",
};

void isr_register(uint8_t n, isr_handler_t h)
{
    if (n < 32) isr_handlers[n] = h;
}

void isr_dispatch(registers_t *r)
{
    if (r->int_no < 32 && isr_handlers[r->int_no]) {
        isr_handlers[r->int_no](r);
        return;
    }

    panic("CPU exception #%u (%s) err=0x%x at eip=0x%x",
          r->int_no,
          r->int_no < 32 ? exception_names[r->int_no] : "unknown",
          r->err_code, r->eip);
}
