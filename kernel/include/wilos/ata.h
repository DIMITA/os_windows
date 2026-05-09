#ifndef WILOS_ATA_H
#define WILOS_ATA_H

#include <wilos/types.h>

#define ATA_SECTOR_SIZE 512
#define ATA_MAX_DRIVES  4

typedef struct {
    bool     present;
    bool     atapi;          /* CD/DVD vs HDD */
    uint16_t io_base;        /* 0x1F0 / 0x170 */
    uint16_t ctrl_base;      /* 0x3F6 / 0x376 */
    bool     slave;
    uint64_t sectors;        /* total sector count */
    bool     lba48;
    char     model[41];
} ata_drive_t;

void               ata_init(void);
size_t             ata_drive_count(void);
const ata_drive_t *ata_drive(size_t i);

/* Read `count` sectors starting at `lba` from drive `i` into `buf`
 * (which must be at least count * 512 bytes). Returns 0 on success,
 * -1 on error. */
int ata_read(size_t i, uint64_t lba, size_t count, void *buf);

/* Write `count` sectors starting at `lba` to drive `i` from `buf`.
 * Returns 0 on success, -1 on error. Issues a CACHE FLUSH after the
 * last sector. Refuses to touch ATAPI devices. */
int ata_write(size_t i, uint64_t lba, size_t count, const void *buf);

#endif
