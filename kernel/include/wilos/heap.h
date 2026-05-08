#ifndef WILOS_HEAP_H
#define WILOS_HEAP_H

#include <wilos/types.h>

void   heap_init(uintptr_t start, size_t size);
void  *kmalloc(size_t size);
void   kfree(void *p);
size_t heap_used(void);
size_t heap_total(void);

#endif
