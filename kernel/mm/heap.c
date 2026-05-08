#include <wilos/heap.h>
#include <wilos/string.h>
#include <wilos/types.h>

/* Tiny first-fit free-list allocator. Block layout:
 *
 *   [ block_t header ][ user data ... ]
 *
 * Adjacent free blocks are coalesced on free. This is intentionally
 * simple — phase 1 will replace it with a slab allocator. */

typedef struct block {
    size_t        size;        /* size of the user payload */
    bool          free;
    struct block *next;
} block_t;

static block_t *head;
static size_t   total_size;
static size_t   used_size;

#define ALIGN_UP(x, a) (((x) + (a) - 1) & ~((a) - 1))

void heap_init(uintptr_t start, size_t size)
{
    head           = (block_t *)start;
    head->size     = size - sizeof(block_t);
    head->free     = true;
    head->next     = NULL;
    total_size     = size;
    used_size      = sizeof(block_t);
}

static void split(block_t *b, size_t size)
{
    if (b->size >= size + sizeof(block_t) + 16) {
        block_t *nb = (block_t *)((uint8_t *)b + sizeof(block_t) + size);
        nb->size    = b->size - size - sizeof(block_t);
        nb->free    = true;
        nb->next    = b->next;
        b->size     = size;
        b->next     = nb;
        used_size  += sizeof(block_t);
    }
}

void *kmalloc(size_t size)
{
    size = ALIGN_UP(size, 8);
    if (!size) return NULL;

    for (block_t *b = head; b; b = b->next) {
        if (b->free && b->size >= size) {
            split(b, size);
            b->free   = false;
            used_size += b->size;
            return (void *)((uint8_t *)b + sizeof(block_t));
        }
    }
    return NULL;
}

static void coalesce(void)
{
    for (block_t *b = head; b && b->next; ) {
        if (b->free && b->next->free) {
            b->size += sizeof(block_t) + b->next->size;
            b->next  = b->next->next;
            used_size -= sizeof(block_t);
        } else {
            b = b->next;
        }
    }
}

void kfree(void *p)
{
    if (!p) return;
    block_t *b = (block_t *)((uint8_t *)p - sizeof(block_t));
    if (!b->free) {
        b->free = true;
        used_size -= b->size;
        coalesce();
    }
}

size_t heap_used(void)  { return used_size; }
size_t heap_total(void) { return total_size; }
