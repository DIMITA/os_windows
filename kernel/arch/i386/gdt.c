#include <wilos/gdt.h>
#include <wilos/types.h>

typedef struct {
    uint16_t limit_low;
    uint16_t base_low;
    uint8_t  base_mid;
    uint8_t  access;
    uint8_t  granularity;
    uint8_t  base_high;
} __attribute__((packed)) gdt_entry_t;

typedef struct {
    uint16_t limit;
    uint32_t base;
} __attribute__((packed)) gdt_ptr_t;

#define GDT_ENTRIES 5

static gdt_entry_t gdt[GDT_ENTRIES];
static gdt_ptr_t   gdtp;

extern void gdt_flush(uint32_t);

static void gdt_set(int n, uint32_t base, uint32_t limit, uint8_t access, uint8_t gran)
{
    gdt[n].base_low    = base & 0xFFFF;
    gdt[n].base_mid    = (base >> 16) & 0xFF;
    gdt[n].base_high   = (base >> 24) & 0xFF;
    gdt[n].limit_low   = limit & 0xFFFF;
    gdt[n].granularity = ((limit >> 16) & 0x0F) | (gran & 0xF0);
    gdt[n].access      = access;
}

void gdt_init(void)
{
    gdtp.limit = sizeof(gdt) - 1;
    gdtp.base  = (uint32_t)&gdt;

    gdt_set(0, 0, 0, 0, 0);                       /* null */
    gdt_set(1, 0, 0xFFFFF, 0x9A, 0xCF);           /* kernel code */
    gdt_set(2, 0, 0xFFFFF, 0x92, 0xCF);           /* kernel data */
    gdt_set(3, 0, 0xFFFFF, 0xFA, 0xCF);           /* user code   */
    gdt_set(4, 0, 0xFFFFF, 0xF2, 0xCF);           /* user data   */

    gdt_flush((uint32_t)&gdtp);
}
