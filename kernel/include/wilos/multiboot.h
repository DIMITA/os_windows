#ifndef WILOS_MULTIBOOT_H
#define WILOS_MULTIBOOT_H

#include <wilos/types.h>

#define MULTIBOOT_BOOTLOADER_MAGIC 0x2BADB002

#define MULTIBOOT_INFO_MEMORY     0x00000001
#define MULTIBOOT_INFO_BOOTDEV    0x00000002
#define MULTIBOOT_INFO_CMDLINE    0x00000004
#define MULTIBOOT_INFO_MODS       0x00000008
#define MULTIBOOT_INFO_MEM_MAP    0x00000040

typedef struct multiboot_mmap_entry {
    uint32_t size;
    uint64_t addr;
    uint64_t len;
    uint32_t type;
} __attribute__((packed)) multiboot_mmap_entry_t;

#define MULTIBOOT_MEMORY_AVAILABLE        1
#define MULTIBOOT_MEMORY_RESERVED         2
#define MULTIBOOT_MEMORY_ACPI_RECLAIMABLE 3
#define MULTIBOOT_MEMORY_NVS              4
#define MULTIBOOT_MEMORY_BADRAM           5

typedef struct multiboot_info {
    uint32_t flags;
    uint32_t mem_lower;
    uint32_t mem_upper;
    uint32_t boot_device;
    uint32_t cmdline;
    uint32_t mods_count;
    uint32_t mods_addr;
    uint32_t syms[4];
    uint32_t mmap_length;
    uint32_t mmap_addr;
} __attribute__((packed)) multiboot_info_t;

#endif
