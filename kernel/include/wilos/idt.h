#ifndef WILOS_IDT_H
#define WILOS_IDT_H

#include <wilos/types.h>

typedef struct {
    uint32_t ds;
    uint32_t edi, esi, ebp, esp, ebx, edx, ecx, eax;
    uint32_t int_no, err_code;
    uint32_t eip, cs, eflags, useresp, ss;
} __attribute__((packed)) registers_t;

typedef void (*isr_handler_t)(registers_t *);

void idt_init(void);
void isr_register(uint8_t n, isr_handler_t h);
void irq_register(uint8_t irq, isr_handler_t h);

#endif
