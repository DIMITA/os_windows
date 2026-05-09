#include <wilos/fat.h>
#include <wilos/ata.h>
#include <wilos/string.h>
#include <wilos/heap.h>
#include <wilos/printf.h>
#include <wilos/types.h>

/* Read-only FAT16 / FAT32 driver. Long file names (LFN) are decoded
 * from VFAT entries when present, otherwise we fall back to the 8.3
 * short name. Writing comes in phase 1.1 along with the installer. */

#define DIR_ENTRY_SIZE 32
#define ATTR_READ_ONLY 0x01
#define ATTR_HIDDEN    0x02
#define ATTR_SYSTEM    0x04
#define ATTR_VOLUME_ID 0x08
#define ATTR_DIRECTORY 0x10
#define ATTR_ARCHIVE   0x20
#define ATTR_LFN       0x0F

typedef struct __attribute__((packed)) {
    uint8_t  jmp[3];
    uint8_t  oem[8];
    uint16_t bytes_per_sector;
    uint8_t  sectors_per_cluster;
    uint16_t reserved_sectors;
    uint8_t  fat_count;
    uint16_t root_entries;
    uint16_t total_sectors_16;
    uint8_t  media;
    uint16_t sectors_per_fat_16;
    uint16_t sectors_per_track;
    uint16_t heads;
    uint32_t hidden_sectors;
    uint32_t total_sectors_32;

    /* FAT32 extension */
    uint32_t sectors_per_fat_32;
    uint16_t flags;
    uint16_t fs_version;
    uint32_t root_cluster;
    uint16_t fs_info_sector;
    uint16_t backup_boot_sector;
    uint8_t  reserved[12];
    uint8_t  drive_num;
    uint8_t  reserved2;
    uint8_t  ext_signature;
    uint32_t volume_id;
    uint8_t  volume_label[11];
    uint8_t  fs_type[8];
} bpb_t;

static int read_sectors(fat_fs_t *fs, uint32_t rel_sector, uint32_t count, void *buf)
{
    return ata_read(fs->drive, fs->part_lba + rel_sector, count, buf);
}

int fat_mount(fat_fs_t *fs, size_t drive, uint64_t part_lba, uint64_t part_count)
{
    memset(fs, 0, sizeof(*fs));
    fs->drive      = drive;
    fs->part_lba   = part_lba;
    fs->part_count = part_count;

    uint8_t boot[512];
    if (ata_read(drive, part_lba, 1, boot) < 0) return -1;
    if (boot[510] != 0x55 || boot[511] != 0xAA) return -1;

    bpb_t bpb;
    memcpy(&bpb, boot, sizeof(bpb));

    if (bpb.bytes_per_sector != 512 || bpb.sectors_per_cluster == 0) return -1;

    fs->bytes_per_sector    = bpb.bytes_per_sector;
    fs->sectors_per_cluster = bpb.sectors_per_cluster;
    fs->reserved_sectors    = bpb.reserved_sectors;
    fs->fat_count           = bpb.fat_count;
    fs->root_dir_entries    = bpb.root_entries;

    uint32_t spf = bpb.sectors_per_fat_16
                   ? bpb.sectors_per_fat_16
                   : bpb.sectors_per_fat_32;
    if (spf == 0) return -1;
    fs->sectors_per_fat = spf;

    uint32_t total_sectors = bpb.total_sectors_16
                             ? bpb.total_sectors_16
                             : bpb.total_sectors_32;
    if (total_sectors == 0) return -1;

    uint32_t root_dir_sectors =
        ((bpb.root_entries * 32) + (bpb.bytes_per_sector - 1)) / bpb.bytes_per_sector;

    fs->fat_start       = bpb.reserved_sectors;
    fs->root_dir_sector = fs->fat_start + bpb.fat_count * spf;
    fs->data_start      = fs->root_dir_sector + root_dir_sectors;

    uint32_t data_sectors = total_sectors - fs->data_start;
    fs->cluster_count    = data_sectors / bpb.sectors_per_cluster;

    if (fs->cluster_count < 4085) {
        /* Treat FAT12 as unsupported. */
        return -1;
    } else if (fs->cluster_count < 65525) {
        fs->type         = FAT_TYPE_16;
        fs->root_cluster = 0;
    } else {
        fs->type         = FAT_TYPE_32;
        fs->root_cluster = bpb.root_cluster;
    }

    fs->mounted = true;
    return 0;
}

static uint32_t cluster_to_sector(fat_fs_t *fs, uint32_t cluster)
{
    return fs->data_start + (cluster - 2) * fs->sectors_per_cluster;
}

static uint32_t fat_next(fat_fs_t *fs, uint32_t cluster)
{
    uint8_t buf[512];
    uint32_t entry_size = (fs->type == FAT_TYPE_32) ? 4 : 2;
    uint32_t fat_offset = cluster * entry_size;
    uint32_t sector = fs->fat_start + fat_offset / 512;
    uint32_t off    = fat_offset % 512;

    if (read_sectors(fs, sector, 1, buf) < 0) return 0x0FFFFFFF;

    uint32_t v;
    if (fs->type == FAT_TYPE_32) {
        v = (*(uint32_t *)(buf + off)) & 0x0FFFFFFF;
        if (v >= 0x0FFFFFF8) v = 0x0FFFFFFF;
    } else {
        v = *(uint16_t *)(buf + off);
        if (v >= 0xFFF8) v = 0x0FFFFFFF;
    }
    return v;
}

/* --- directory iteration ------------------------------------------------ */

int fat_open_root(fat_fs_t *fs, fat_dir_t *dir)
{
    dir->fs       = fs;
    dir->offset   = 0;
    if (fs->type == FAT_TYPE_32) {
        dir->cluster    = fs->root_cluster;
        dir->fat16_root = false;
    } else {
        dir->cluster    = 0;
        dir->fat16_root = true;
    }
    return 0;
}

static int read_dir_chunk(fat_dir_t *d, uint8_t *buf, uint32_t *bytes_read)
{
    fat_fs_t *fs = d->fs;
    if (d->fat16_root) {
        if (d->cluster >= fs->root_dir_entries) { *bytes_read = 0; return 0; }
        uint32_t sector = fs->root_dir_sector + d->cluster / 16;
        if (read_sectors(fs, sector, 1, buf) < 0) return -1;
        *bytes_read = 512;
        return 0;
    }

    if (d->cluster >= 0x0FFFFFF8 || d->cluster < 2) { *bytes_read = 0; return 0; }
    uint32_t s0 = cluster_to_sector(fs, d->cluster);
    uint32_t to_read = fs->sectors_per_cluster;
    if (read_sectors(fs, s0, to_read, buf) < 0) return -1;
    *bytes_read = to_read * 512;
    return 0;
}

static void advance_dir(fat_dir_t *d)
{
    fat_fs_t *fs = d->fs;
    if (d->fat16_root) {
        d->cluster += 16;        /* 16 entries per sector */
        d->offset = 0;
    } else {
        d->cluster = fat_next(fs, d->cluster);
        d->offset = 0;
    }
}

static void short_name_to_string(const uint8_t *e, char *out)
{
    int i, j = 0;
    for (i = 0; i < 8 && e[i] != ' '; i++) out[j++] = e[i];
    if (e[8] != ' ') {
        out[j++] = '.';
        for (i = 8; i < 11 && e[i] != ' '; i++) out[j++] = e[i];
    }
    out[j] = '\0';
}

int fat_readdir(fat_dir_t *d, fat_entry_t *out)
{
    fat_fs_t *fs = d->fs;
    static uint8_t buf[4096];
    uint32_t got = 0;
    char lfn[FAT_NAME_MAX]; lfn[0] = '\0';

    while (1) {
        if (d->offset == 0 || d->offset >= got) {
            if (d->offset >= got && got > 0) advance_dir(d);
            if (read_dir_chunk(d, buf, &got) < 0) return -1;
            if (got == 0) return 0;       /* end */
            d->offset = 0;
        }

        for (; d->offset + DIR_ENTRY_SIZE <= got; d->offset += DIR_ENTRY_SIZE) {
            uint8_t *e = buf + d->offset;
            if (e[0] == 0x00) return 0;   /* end of directory */
            if (e[0] == 0xE5) { lfn[0] = '\0'; continue; }   /* deleted */

            uint8_t attr = e[11];
            if (attr == ATTR_LFN) {
                /* Build LFN piece by piece. Order field is in e[0]. */
                uint8_t seq = e[0] & 0x1F;
                if (seq == 0 || seq > 20) { lfn[0] = '\0'; continue; }
                char piece[14];
                int pi = 0;
                static const int off[] = { 1,3,5,7,9, 14,16,18,20,22,24, 28,30 };
                for (int k = 0; k < 13; k++) {
                    uint16_t c = e[off[k]] | (e[off[k] + 1] << 8);
                    if (c == 0 || c == 0xFFFF) break;
                    piece[pi++] = (c < 0x80) ? (char)c : '?';
                }
                piece[pi] = '\0';

                /* Prepend this piece (entries arrive in reverse order). */
                size_t pl = strlen(piece);
                size_t cl = strlen(lfn);
                if (pl + cl < FAT_NAME_MAX) {
                    char tmp[FAT_NAME_MAX];
                    memcpy(tmp, piece, pl);
                    memcpy(tmp + pl, lfn, cl + 1);
                    memcpy(lfn, tmp, pl + cl + 1);
                }
                continue;
            }

            if (attr & ATTR_VOLUME_ID) { lfn[0] = '\0'; continue; }

            if (lfn[0]) {
                size_t n = strlen(lfn);
                if (n >= FAT_NAME_MAX) n = FAT_NAME_MAX - 1;
                memcpy(out->name, lfn, n);
                out->name[n] = '\0';
            } else {
                short_name_to_string(e, out->name);
            }
            lfn[0] = '\0';

            out->is_dir  = (attr & ATTR_DIRECTORY) != 0;
            out->size    = *(uint32_t *)(e + 28);
            uint16_t lo  = *(uint16_t *)(e + 26);
            uint16_t hi  = *(uint16_t *)(e + 20);
            out->cluster = ((uint32_t)hi << 16) | lo;

            d->offset += DIR_ENTRY_SIZE;
            return 1;
        }

        advance_dir(d);
        if ((d->fat16_root && d->cluster >= fs->root_dir_entries) ||
            (!d->fat16_root && (d->cluster >= 0x0FFFFFF8 || d->cluster < 2))) {
            return 0;
        }
        d->offset = 0;
        got = 0;
    }
}

int fat_read_file(fat_fs_t *fs, const fat_entry_t *e,
                  uint32_t offset, void *buf, uint32_t len)
{
    if (e->is_dir || offset >= e->size) return 0;
    if (offset + len > e->size) len = e->size - offset;

    uint32_t bytes_per_cluster = fs->sectors_per_cluster * 512;
    uint32_t cluster = e->cluster;
    uint32_t skip    = offset / bytes_per_cluster;
    uint32_t inner   = offset % bytes_per_cluster;

    while (skip-- && cluster < 0x0FFFFFF8 && cluster >= 2) {
        cluster = fat_next(fs, cluster);
    }

    uint8_t  tmp[4096];
    uint8_t *out = (uint8_t *)buf;
    uint32_t copied = 0;

    while (copied < len && cluster >= 2 && cluster < 0x0FFFFFF8) {
        uint32_t s0 = cluster_to_sector(fs, cluster);
        if (ata_read(fs->drive, fs->part_lba + s0, fs->sectors_per_cluster, tmp) < 0)
            return copied;

        uint32_t avail = bytes_per_cluster - inner;
        uint32_t take  = len - copied;
        if (take > avail) take = avail;
        memcpy(out + copied, tmp + inner, take);
        copied += take;
        inner   = 0;
        cluster = fat_next(fs, cluster);
    }
    return copied;
}

static int icmp(const char *a, const char *b)
{
    while (*a && *b) {
        char ca = *a, cb = *b;
        if (ca >= 'A' && ca <= 'Z') ca += 32;
        if (cb >= 'A' && cb <= 'Z') cb += 32;
        if (ca != cb) return ca - cb;
        a++; b++;
    }
    return *a - *b;
}

int fat_lookup(fat_fs_t *fs, const char *path, fat_entry_t *out)
{
    while (*path == '/') path++;

    fat_dir_t dir;
    fat_open_root(fs, &dir);

    char comp[FAT_NAME_MAX];
    while (*path) {
        size_t i = 0;
        while (path[i] && path[i] != '/' && i + 1 < sizeof(comp)) {
            comp[i] = path[i]; i++;
        }
        comp[i] = '\0';

        fat_entry_t e;
        bool found = false;
        while (fat_readdir(&dir, &e) > 0) {
            if (!icmp(e.name, comp)) {
                *out  = e;
                found = true;
                break;
            }
        }
        if (!found) return -1;

        path += i;
        while (*path == '/') path++;
        if (!*path) return 0;
        if (!e.is_dir) return -1;

        dir.fs         = fs;
        dir.cluster    = e.cluster;
        dir.offset     = 0;
        dir.fat16_root = false;
    }
    return 0;
}
