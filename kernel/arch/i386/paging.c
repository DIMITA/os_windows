#include <wilos/paging.h>
#include <wilos/pmm.h>
#include <wilos/string.h>
#include <wilos/types.h>

/* A small identity-mapping pager. The kernel runs with a single page
 * directory that identity-maps the first 16 MiB of physical memory so
 * the rest of the kernel sees the same addresses with or without
 * paging enabled. Higher-half mapping and per-process address spaces
 * are part of phase 1 (see docs/ROADMAP.md). */

#define ENTRIES 1024

static uint32_t page_directory[ENTRIES] __attribute__((aligned(4096)));
static uint32_t first_tables[4][ENTRIES] __attribute__((aligned(4096)));

static void load_directory(uint32_t *pd)
{
    __asm__ volatile ("mov %0, %%cr3" : : "r"(pd));
}

static void enable_paging(void)
{
    uint32_t cr0;
    __asm__ volatile ("mov %%cr0, %0" : "=r"(cr0));
    cr0 |= 0x80000000;
    __asm__ volatile ("mov %0, %%cr0" : : "r"(cr0));
}

void paging_init(void)
{
    memset(page_directory, 0, sizeof(page_directory));

    /* Identity map the first 16 MiB (4 page tables). */
    for (int t = 0; t < 4; t++) {
        for (int i = 0; i < ENTRIES; i++) {
            uint32_t phys = (t * ENTRIES + i) * 4096;
            first_tables[t][i] = phys | PAGE_PRESENT | PAGE_RW;
        }
        page_directory[t] = ((uint32_t)first_tables[t]) | PAGE_PRESENT | PAGE_RW;
    }

    load_directory(page_directory);
    enable_paging();
}

void paging_map(uintptr_t virt, uintptr_t phys, uint32_t flags)
{
    uint32_t pd_idx = virt >> 22;
    uint32_t pt_idx = (virt >> 12) & 0x3FF;

    if (pd_idx < 4) {
        first_tables[pd_idx][pt_idx] = (phys & ~0xFFF) | (flags & 0xFFF) | PAGE_PRESENT;
        __asm__ volatile ("invlpg (%0)" : : "r"(virt) : "memory");
    }
    /* Beyond 16 MiB: phase 1 will allocate page tables on demand. */
}
