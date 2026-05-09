#include <wilos/ata.h>
#include <wilos/ports.h>
#include <wilos/string.h>
#include <wilos/printf.h>
#include <wilos/types.h>

/* ATA PIO driver for the legacy IDE interface (primary + secondary
 * controllers, master + slave devices). This is enough to talk to:
 *   - real IDE / SATA disks exposed in legacy mode by the BIOS
 *   - QEMU's `-drive if=ide` and the default CD-ROM
 *
 * AHCI / NVMe come in phase 1.1. */

#define ATA_REG_DATA       0
#define ATA_REG_ERROR      1
#define ATA_REG_FEATURES   1
#define ATA_REG_SECCOUNT0  2
#define ATA_REG_LBA0       3
#define ATA_REG_LBA1       4
#define ATA_REG_LBA2       5
#define ATA_REG_HDDEVSEL   6
#define ATA_REG_COMMAND    7
#define ATA_REG_STATUS     7

#define ATA_SR_BSY  0x80
#define ATA_SR_DRDY 0x40
#define ATA_SR_DRQ  0x08
#define ATA_SR_ERR  0x01

#define ATA_CMD_READ_PIO        0x20
#define ATA_CMD_READ_PIO_EXT    0x24
#define ATA_CMD_WRITE_PIO       0x30
#define ATA_CMD_WRITE_PIO_EXT   0x34
#define ATA_CMD_CACHE_FLUSH     0xE7
#define ATA_CMD_CACHE_FLUSH_EXT 0xEA
#define ATA_CMD_IDENTIFY        0xEC
#define ATA_CMD_IDENTIFY_PACKET 0xA1

static ata_drive_t drives[ATA_MAX_DRIVES];
static size_t      drive_count;

static void ata_io_wait(uint16_t ctrl)
{
    inb(ctrl); inb(ctrl); inb(ctrl); inb(ctrl);
}

static int wait_not_busy(uint16_t io)
{
    for (int i = 0; i < 100000; i++) {
        uint8_t s = inb(io + ATA_REG_STATUS);
        if (!(s & ATA_SR_BSY)) return 0;
    }
    return -1;
}

static int wait_drq(uint16_t io)
{
    for (int i = 0; i < 100000; i++) {
        uint8_t s = inb(io + ATA_REG_STATUS);
        if (s & ATA_SR_ERR) return -1;
        if (!(s & ATA_SR_BSY) && (s & ATA_SR_DRQ)) return 0;
    }
    return -1;
}

static void copy_id_string(char *dst, const uint16_t *id, int word_off, int words)
{
    for (int i = 0; i < words; i++) {
        uint16_t w = id[word_off + i];
        dst[i * 2 + 0] = (char)(w >> 8);
        dst[i * 2 + 1] = (char)(w & 0xFF);
    }
    dst[words * 2] = '\0';
    /* Trim trailing spaces. */
    for (int i = words * 2 - 1; i >= 0 && dst[i] == ' '; i--) dst[i] = '\0';
}

static void try_identify(uint16_t io, uint16_t ctrl, bool slave)
{
    if (drive_count >= ATA_MAX_DRIVES) return;
    ata_drive_t *d = &drives[drive_count];

    outb(io + ATA_REG_HDDEVSEL, slave ? 0xB0 : 0xA0);
    ata_io_wait(ctrl);

    outb(io + ATA_REG_SECCOUNT0, 0);
    outb(io + ATA_REG_LBA0, 0);
    outb(io + ATA_REG_LBA1, 0);
    outb(io + ATA_REG_LBA2, 0);
    outb(io + ATA_REG_COMMAND, ATA_CMD_IDENTIFY);
    ata_io_wait(ctrl);

    uint8_t status = inb(io + ATA_REG_STATUS);
    if (status == 0) return;            /* nothing on this slot */

    /* Wait for BSY clear, watching for ATAPI signature. */
    bool atapi = false;
    for (int i = 0; i < 100000; i++) {
        status = inb(io + ATA_REG_STATUS);
        if (!(status & ATA_SR_BSY)) break;
        if (i == 99999) return;
    }

    uint8_t lba1 = inb(io + ATA_REG_LBA1);
    uint8_t lba2 = inb(io + ATA_REG_LBA2);
    if (lba1 == 0x14 && lba2 == 0xEB) {
        atapi = true;
        outb(io + ATA_REG_COMMAND, ATA_CMD_IDENTIFY_PACKET);
        ata_io_wait(ctrl);
    } else if (lba1 != 0 || lba2 != 0) {
        /* Not a standard ATA device, skip. */
        return;
    }

    if (wait_drq(io) < 0) return;

    uint16_t id[256];
    for (int i = 0; i < 256; i++) id[i] = inw(io + ATA_REG_DATA);

    d->present   = true;
    d->atapi     = atapi;
    d->io_base   = io;
    d->ctrl_base = ctrl;
    d->slave     = slave;
    d->lba48     = (id[83] & (1 << 10)) != 0;

    if (d->lba48) {
        d->sectors = ((uint64_t)id[100])
                   | ((uint64_t)id[101] << 16)
                   | ((uint64_t)id[102] << 32)
                   | ((uint64_t)id[103] << 48);
    } else {
        d->sectors = ((uint64_t)id[60]) | ((uint64_t)id[61] << 16);
    }

    copy_id_string(d->model, id, 27, 20);
    drive_count++;
}

void ata_init(void)
{
    drive_count = 0;
    memset(drives, 0, sizeof(drives));

    try_identify(0x1F0, 0x3F6, false);     /* primary master */
    try_identify(0x1F0, 0x3F6, true);      /* primary slave  */
    try_identify(0x170, 0x376, false);     /* secondary master */
    try_identify(0x170, 0x376, true);      /* secondary slave  */
}

size_t ata_drive_count(void) { return drive_count; }

const ata_drive_t *ata_drive(size_t i)
{
    if (i >= drive_count) return NULL;
    return &drives[i];
}

static int read_one_28(const ata_drive_t *d, uint32_t lba, void *buf)
{
    uint16_t io = d->io_base;

    if (wait_not_busy(io) < 0) return -1;
    outb(io + ATA_REG_HDDEVSEL,
         (d->slave ? 0xF0 : 0xE0) | ((lba >> 24) & 0x0F));
    ata_io_wait(d->ctrl_base);
    outb(io + ATA_REG_SECCOUNT0, 1);
    outb(io + ATA_REG_LBA0, lba & 0xFF);
    outb(io + ATA_REG_LBA1, (lba >> 8) & 0xFF);
    outb(io + ATA_REG_LBA2, (lba >> 16) & 0xFF);
    outb(io + ATA_REG_COMMAND, ATA_CMD_READ_PIO);

    if (wait_drq(io) < 0) return -1;
    uint16_t *p = (uint16_t *)buf;
    for (int i = 0; i < 256; i++) p[i] = inw(io + ATA_REG_DATA);
    return 0;
}

static int read_one_48(const ata_drive_t *d, uint64_t lba, void *buf)
{
    uint16_t io = d->io_base;

    if (wait_not_busy(io) < 0) return -1;
    outb(io + ATA_REG_HDDEVSEL, d->slave ? 0x50 : 0x40);
    ata_io_wait(d->ctrl_base);

    outb(io + ATA_REG_SECCOUNT0, 0);                       /* high byte */
    outb(io + ATA_REG_LBA0, (lba >> 24) & 0xFF);
    outb(io + ATA_REG_LBA1, (lba >> 32) & 0xFF);
    outb(io + ATA_REG_LBA2, (lba >> 40) & 0xFF);
    outb(io + ATA_REG_SECCOUNT0, 1);                       /* low byte  */
    outb(io + ATA_REG_LBA0, lba & 0xFF);
    outb(io + ATA_REG_LBA1, (lba >> 8) & 0xFF);
    outb(io + ATA_REG_LBA2, (lba >> 16) & 0xFF);
    outb(io + ATA_REG_COMMAND, ATA_CMD_READ_PIO_EXT);

    if (wait_drq(io) < 0) return -1;
    uint16_t *p = (uint16_t *)buf;
    for (int i = 0; i < 256; i++) p[i] = inw(io + ATA_REG_DATA);
    return 0;
}

int ata_read(size_t i, uint64_t lba, size_t count, void *buf)
{
    const ata_drive_t *d = ata_drive(i);
    if (!d || d->atapi) return -1;
    if (lba + count > d->sectors) return -1;

    uint8_t *out = (uint8_t *)buf;
    for (size_t s = 0; s < count; s++) {
        int rc = d->lba48
            ? read_one_48(d, lba + s, out + s * ATA_SECTOR_SIZE)
            : read_one_28(d, (uint32_t)(lba + s), out + s * ATA_SECTOR_SIZE);
        if (rc) return -1;
    }
    return 0;
}

static int write_one_28(const ata_drive_t *d, uint32_t lba, const void *buf)
{
    uint16_t io = d->io_base;

    if (wait_not_busy(io) < 0) return -1;
    outb(io + ATA_REG_HDDEVSEL,
         (d->slave ? 0xF0 : 0xE0) | ((lba >> 24) & 0x0F));
    ata_io_wait(d->ctrl_base);
    outb(io + ATA_REG_SECCOUNT0, 1);
    outb(io + ATA_REG_LBA0, lba & 0xFF);
    outb(io + ATA_REG_LBA1, (lba >> 8) & 0xFF);
    outb(io + ATA_REG_LBA2, (lba >> 16) & 0xFF);
    outb(io + ATA_REG_COMMAND, ATA_CMD_WRITE_PIO);

    if (wait_drq(io) < 0) return -1;
    const uint16_t *p = (const uint16_t *)buf;
    for (int i = 0; i < 256; i++) outw(io + ATA_REG_DATA, p[i]);
    return 0;
}

static int write_one_48(const ata_drive_t *d, uint64_t lba, const void *buf)
{
    uint16_t io = d->io_base;

    if (wait_not_busy(io) < 0) return -1;
    outb(io + ATA_REG_HDDEVSEL, d->slave ? 0x50 : 0x40);
    ata_io_wait(d->ctrl_base);

    outb(io + ATA_REG_SECCOUNT0, 0);
    outb(io + ATA_REG_LBA0, (lba >> 24) & 0xFF);
    outb(io + ATA_REG_LBA1, (lba >> 32) & 0xFF);
    outb(io + ATA_REG_LBA2, (lba >> 40) & 0xFF);
    outb(io + ATA_REG_SECCOUNT0, 1);
    outb(io + ATA_REG_LBA0, lba & 0xFF);
    outb(io + ATA_REG_LBA1, (lba >> 8) & 0xFF);
    outb(io + ATA_REG_LBA2, (lba >> 16) & 0xFF);
    outb(io + ATA_REG_COMMAND, ATA_CMD_WRITE_PIO_EXT);

    if (wait_drq(io) < 0) return -1;
    const uint16_t *p = (const uint16_t *)buf;
    for (int i = 0; i < 256; i++) outw(io + ATA_REG_DATA, p[i]);
    return 0;
}

static int cache_flush(const ata_drive_t *d)
{
    uint16_t io = d->io_base;
    outb(io + ATA_REG_COMMAND,
         d->lba48 ? ATA_CMD_CACHE_FLUSH_EXT : ATA_CMD_CACHE_FLUSH);
    return wait_not_busy(io);
}

int ata_write(size_t i, uint64_t lba, size_t count, const void *buf)
{
    const ata_drive_t *d = ata_drive(i);
    if (!d || d->atapi) return -1;
    if (lba + count > d->sectors) return -1;

    const uint8_t *in = (const uint8_t *)buf;
    for (size_t s = 0; s < count; s++) {
        int rc = d->lba48
            ? write_one_48(d, lba + s, in + s * ATA_SECTOR_SIZE)
            : write_one_28(d, (uint32_t)(lba + s), in + s * ATA_SECTOR_SIZE);
        if (rc) return -1;
    }
    return cache_flush(d);
}
