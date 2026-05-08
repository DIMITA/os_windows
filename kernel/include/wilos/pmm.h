#ifndef WILOS_PMM_H
#define WILOS_PMM_H

#include <wilos/types.h>
#include <wilos/multiboot.h>

#define PAGE_SIZE 4096

void     pmm_init(const multiboot_info_t *mbi, uintptr_t kernel_end);
void    *pmm_alloc_page(void);
void     pmm_free_page(void *p);
size_t   pmm_total_pages(void);
size_t   pmm_used_pages(void);

#endif
