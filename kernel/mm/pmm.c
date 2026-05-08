#include <wilos/pmm.h>
#include <wilos/string.h>
#include <wilos/printf.h>
#include <wilos/types.h>

/* A bitmap-based physical memory manager.
 *
 * Each bit tracks one 4 KiB page: 0 = free, 1 = used. The bitmap is
 * placed immediately after the kernel image so it does not collide
 * with anything reserved. Pages owned by the kernel and the bitmap
 * itself are pre-marked as used. */

static uint32_t *bitmap;
static size_t    bitmap_pages;
static size_t    total_pages;
static size_t    used_pages;

static inline void bit_set(size_t i)   { bitmap[i / 32] |=  (1u << (i % 32)); }
static inline void bit_clear(size_t i) { bitmap[i / 32] &= ~(1u << (i % 32)); }
static inline int  bit_get(size_t i)   { return (bitmap[i / 32] >> (i % 32)) & 1; }

void pmm_init(const multiboot_info_t *mbi, uintptr_t kernel_end)
{
    /* Determine total memory: prefer the multiboot mmap, fall back to
     * mem_lower/mem_upper. mem_upper is in KiB starting at 1 MiB. */
    uint64_t highest = 0;
    if (mbi && (mbi->flags & MULTIBOOT_INFO_MEM_MAP) && mbi->mmap_length) {
        const uint8_t *cur = (const uint8_t *)mbi->mmap_addr;
        const uint8_t *end = cur + mbi->mmap_length;
        while (cur < end) {
            const multiboot_mmap_entry_t *e = (const multiboot_mmap_entry_t *)cur;
            if (e->type == MULTIBOOT_MEMORY_AVAILABLE) {
                uint64_t top = e->addr + e->len;
                if (top > highest) highest = top;
            }
            cur += e->size + 4;
        }
    } else if (mbi && (mbi->flags & MULTIBOOT_INFO_MEMORY)) {
        highest = (uint64_t)0x100000 + (uint64_t)mbi->mem_upper * 1024ULL;
    } else {
        highest = 32 * 1024 * 1024;     /* sane fallback: 32 MiB */
    }

    if (highest > 0xFFFFFFFFULL) highest = 0xFFFFFFFFULL;

    total_pages   = (size_t)(highest / PAGE_SIZE);
    bitmap        = (uint32_t *)((kernel_end + 0xFFF) & ~0xFFF);
    bitmap_pages  = (total_pages / 8 + PAGE_SIZE - 1) / PAGE_SIZE;
    size_t bytes  = bitmap_pages * PAGE_SIZE;

    memset(bitmap, 0xFF, bytes);    /* mark everything used initially */
    used_pages = total_pages;

    /* Free pages reported as available by the multiboot mmap. */
    if (mbi && (mbi->flags & MULTIBOOT_INFO_MEM_MAP) && mbi->mmap_length) {
        const uint8_t *cur = (const uint8_t *)mbi->mmap_addr;
        const uint8_t *end = cur + mbi->mmap_length;
        while (cur < end) {
            const multiboot_mmap_entry_t *e = (const multiboot_mmap_entry_t *)cur;
            if (e->type == MULTIBOOT_MEMORY_AVAILABLE) {
                uint64_t a = e->addr;
                uint64_t b = e->addr + e->len;
                if (a < 0x100000ULL) a = 0x100000ULL;   /* keep low memory reserved */
                size_t pa = (size_t)(a / PAGE_SIZE);
                size_t pb = (size_t)(b / PAGE_SIZE);
                if (pb > total_pages) pb = total_pages;
                for (size_t p = pa; p < pb; p++) {
                    if (bit_get(p)) {
                        bit_clear(p);
                        used_pages--;
                    }
                }
            }
            cur += e->size + 4;
        }
    }

    /* Reserve the kernel and the bitmap itself. */
    uintptr_t reserved_end = (uintptr_t)bitmap + bytes;
    size_t kpages = (reserved_end + PAGE_SIZE - 1) / PAGE_SIZE;
    for (size_t p = 0; p < kpages && p < total_pages; p++) {
        if (!bit_get(p)) {
            bit_set(p);
            used_pages++;
        }
    }
}

void *pmm_alloc_page(void)
{
    for (size_t i = 0; i < total_pages; i++) {
        if (!bit_get(i)) {
            bit_set(i);
            used_pages++;
            return (void *)(i * PAGE_SIZE);
        }
    }
    return NULL;
}

void pmm_free_page(void *p)
{
    if (!p) return;
    size_t i = ((uintptr_t)p) / PAGE_SIZE;
    if (i < total_pages && bit_get(i)) {
        bit_clear(i);
        used_pages--;
    }
}

size_t pmm_total_pages(void) { return total_pages; }
size_t pmm_used_pages(void)  { return used_pages; }
