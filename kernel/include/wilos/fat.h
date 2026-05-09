#ifndef WILOS_FAT_H
#define WILOS_FAT_H

#include <wilos/types.h>

#define FAT_NAME_MAX 256

typedef enum { FAT_TYPE_NONE = 0, FAT_TYPE_16, FAT_TYPE_32 } fat_type_t;

typedef struct {
    bool        mounted;
    size_t      drive;
    uint64_t    part_lba;          /* LBA of partition start              */
    uint64_t    part_count;
    fat_type_t  type;
    uint16_t    bytes_per_sector;
    uint8_t     sectors_per_cluster;
    uint16_t    reserved_sectors;
    uint8_t     fat_count;
    uint32_t    sectors_per_fat;
    uint32_t    root_cluster;      /* FAT32 only                          */
    uint32_t    fat_start;         /* sector relative to part_lba         */
    uint32_t    data_start;        /* sector relative to part_lba         */
    uint32_t    cluster_count;
    /* FAT16 root directory block: */
    uint32_t    root_dir_sector;
    uint16_t    root_dir_entries;
} fat_fs_t;

typedef struct {
    char     name[FAT_NAME_MAX];
    bool     is_dir;
    uint32_t size;
    uint32_t cluster;
} fat_entry_t;

typedef struct {
    fat_fs_t *fs;
    uint32_t  cluster;       /* current cluster (0 = FAT16 root)         */
    uint32_t  offset;        /* byte offset inside the current cluster   */
    bool      fat16_root;
} fat_dir_t;

int  fat_mount(fat_fs_t *fs, size_t drive, uint64_t part_lba, uint64_t part_count);
int  fat_open_root(fat_fs_t *fs, fat_dir_t *dir);
int  fat_readdir(fat_dir_t *dir, fat_entry_t *out);
int  fat_lookup(fat_fs_t *fs, const char *path, fat_entry_t *out);
int  fat_read_file(fat_fs_t *fs, const fat_entry_t *e,
                   uint32_t offset, void *buf, uint32_t len);

#endif
