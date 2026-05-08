#ifndef WILOS_PAGING_H
#define WILOS_PAGING_H

#include <wilos/types.h>

#define PAGE_PRESENT  0x1
#define PAGE_RW       0x2
#define PAGE_USER     0x4

void paging_init(void);
void paging_map(uintptr_t virt, uintptr_t phys, uint32_t flags);

#endif
