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

/* ====================================================================
 * Write path
 * ==================================================================== */

static int write_sectors(fat_fs_t *fs, uint32_t rel_sector,
                         uint32_t count, const void *buf)
{
    return ata_write(fs->drive, fs->part_lba + rel_sector, count, buf);
}

static int set_fat_entry(fat_fs_t *fs, uint32_t cluster, uint32_t value)
{
    uint8_t buf[512];
    uint32_t entry_size = (fs->type == FAT_TYPE_32) ? 4 : 2;
    uint32_t fat_offset = cluster * entry_size;
    uint32_t sec_off    = fat_offset / 512;
    uint32_t in_sec     = fat_offset % 512;

    /* Write to every FAT copy. */
    for (uint8_t f = 0; f < fs->fat_count; f++) {
        uint32_t sector = fs->fat_start + f * fs->sectors_per_fat + sec_off;
        if (read_sectors(fs, sector, 1, buf) < 0) return -1;
        if (fs->type == FAT_TYPE_32) {
            uint32_t old = *(uint32_t *)(buf + in_sec);
            uint32_t mark = (old & 0xF0000000) | (value & 0x0FFFFFFF);
            *(uint32_t *)(buf + in_sec) = mark;
        } else {
            *(uint16_t *)(buf + in_sec) = (uint16_t)value;
        }
        if (write_sectors(fs, sector, 1, buf) < 0) return -1;
    }
    return 0;
}

static uint32_t alloc_cluster(fat_fs_t *fs)
{
    /* Linear scan from cluster 2. */
    for (uint32_t c = 2; c < fs->cluster_count + 2; c++) {
        uint32_t v = fat_next(fs, c);
        if (v == 0) {
            if (set_fat_entry(fs, c, 0x0FFFFFFF) < 0) return 0;
            /* Zero the cluster's data sectors. */
            uint8_t zero[512];
            memset(zero, 0, sizeof(zero));
            uint32_t s0 = cluster_to_sector(fs, c);
            for (uint8_t s = 0; s < fs->sectors_per_cluster; s++) {
                if (write_sectors(fs, s0 + s, 1, zero) < 0) return 0;
            }
            return c;
        }
    }
    return 0;
}

static int free_cluster_chain(fat_fs_t *fs, uint32_t start)
{
    uint32_t c = start;
    while (c >= 2 && c < 0x0FFFFFF8) {
        uint32_t next = fat_next(fs, c);
        if (set_fat_entry(fs, c, 0) < 0) return -1;
        c = next;
    }
    return 0;
}

/* Convert "test.txt" or "VERY-LONG-NAME.TXT" to FAT 8.3 (11 bytes,
 * space-padded, uppercase). Returns -1 if the source is too long. */
static int make_short_name(const char *src, uint8_t out[11])
{
    memset(out, ' ', 11);
    int i = 0, j = 0;
    while (src[i] && src[i] != '.') {
        if (j >= 8) return -1;
        char c = src[i++];
        if (c >= 'a' && c <= 'z') c -= 32;
        out[j++] = (uint8_t)c;
    }
    if (src[i] == '.') i++;
    j = 8;
    while (src[i]) {
        if (j >= 11) return -1;
        char c = src[i++];
        if (c >= 'a' && c <= 'z') c -= 32;
        out[j++] = (uint8_t)c;
    }
    if (out[0] == 0x00 || out[0] == 0xE5) return -1;
    return 0;
}

/* ----- directory mutation ----------------------------------------------- */

/* Locate a directory by path. Returns 0 and fills `out_cluster` with
 * the cluster of that directory (0 means "FAT16 root"). The root path
 * "" / "/" returns cluster 0 on FAT16 or fs->root_cluster on FAT32. */
static int dir_cluster_for_path(fat_fs_t *fs, const char *path,
                                uint32_t *out_cluster)
{
    while (*path == '/') path++;
    if (!*path) {
        *out_cluster = (fs->type == FAT_TYPE_32) ? fs->root_cluster : 0;
        return 0;
    }
    fat_entry_t e;
    if (fat_lookup(fs, path, &e) < 0) return -1;
    if (!e.is_dir) return -1;
    *out_cluster = e.cluster;
    return 0;
}

/* Split path into (parent_dir_path, leaf_name). The parent buffer
 * receives a possibly-empty string for top-level paths. */
static void split_path(const char *path, char *parent, size_t pcap, char *leaf, size_t lcap)
{
    while (*path == '/') path++;
    const char *slash = NULL;
    for (const char *p = path; *p; p++) if (*p == '/') slash = p;

    if (!slash) {
        parent[0] = '\0';
        size_t n  = strlen(path);
        if (n >= lcap) n = lcap - 1;
        memcpy(leaf, path, n); leaf[n] = '\0';
    } else {
        size_t n = slash - path;
        if (n >= pcap) n = pcap - 1;
        memcpy(parent, path, n); parent[n] = '\0';
        size_t m = strlen(slash + 1);
        if (m >= lcap) m = lcap - 1;
        memcpy(leaf, slash + 1, m); leaf[m] = '\0';
    }
}

/* Iterate over the dir entries of a directory cluster (or FAT16 root)
 * and either find an entry by short name (`needle` non-NULL) or find
 * a free slot (`needle` NULL). On success fills `*sector` with the
 * sector containing the entry and `*off` with the byte offset, plus
 * an output buffer holding that sector's contents. */
typedef struct {
    bool      found;
    bool      free_slot;
    uint32_t  sector;
    uint32_t  off;
    uint8_t   buf[512];
    /* tracking for extension when no free slot was found */
    uint32_t  last_cluster;
    uint32_t  last_sector;
} dir_locator_t;

static int dir_scan(fat_fs_t *fs, uint32_t dir_cluster,
                    const uint8_t *needle_83 /*[11] or NULL*/,
                    dir_locator_t *out)
{
    memset(out, 0, sizeof(*out));

    bool fat16_root = (dir_cluster == 0 && fs->type != FAT_TYPE_32);
    uint32_t cluster = dir_cluster;
    uint32_t sector  = fat16_root ? fs->root_dir_sector : 0;
    uint32_t scluster_remaining = fat16_root
        ? ((fs->root_dir_entries * 32 + 511) / 512)
        : fs->sectors_per_cluster;

    while (1) {
        uint32_t s0 = fat16_root ? sector : cluster_to_sector(fs, cluster);
        uint32_t scount = fat16_root ? scluster_remaining : fs->sectors_per_cluster;

        for (uint32_t k = 0; k < scount; k++) {
            uint32_t cur = s0 + k;
            if (read_sectors(fs, cur, 1, out->buf) < 0) return -1;

            for (uint32_t off = 0; off < 512; off += 32) {
                uint8_t *e = out->buf + off;
                uint8_t a = e[11];

                if (e[0] == 0x00) {
                    if (!needle_83) {
                        out->found     = true;
                        out->free_slot = true;
                        out->sector    = cur;
                        out->off       = off;
                        return 0;
                    }
                    /* End of directory and not found. */
                    out->last_cluster = fat16_root ? 0 : cluster;
                    out->last_sector  = cur;
                    return 0;
                }
                if (e[0] == 0xE5) {
                    if (!needle_83) {
                        out->found     = true;
                        out->free_slot = true;
                        out->sector    = cur;
                        out->off       = off;
                        return 0;
                    }
                    continue;
                }
                if (a == ATTR_LFN || (a & ATTR_VOLUME_ID)) continue;

                if (needle_83 && !memcmp(e, needle_83, 11)) {
                    out->found  = true;
                    out->sector = cur;
                    out->off    = off;
                    return 0;
                }
            }
        }

        if (fat16_root) {
            out->last_cluster = 0;
            out->last_sector  = s0 + scluster_remaining - 1;
            return 0;
        }
        uint32_t next = fat_next(fs, cluster);
        if (next >= 0x0FFFFFF8) {
            out->last_cluster = cluster;
            out->last_sector  = s0 + fs->sectors_per_cluster - 1;
            return 0;
        }
        cluster = next;
    }
}

static int dir_make_room(fat_fs_t *fs, dir_locator_t *loc)
{
    /* If we got here, the dir is full or its end-marker entry was at
     * the last 32-byte slot. For FAT32 we extend the dir by one
     * cluster. FAT16 root is fixed-size: refuse. */
    if (loc->last_cluster == 0) return -1;        /* FAT16 root full */

    uint32_t nc = alloc_cluster(fs);
    if (!nc) return -1;
    if (set_fat_entry(fs, loc->last_cluster, nc) < 0) return -1;

    loc->free_slot = true;
    loc->found     = true;
    loc->sector    = cluster_to_sector(fs, nc);
    loc->off       = 0;
    /* The new cluster is already zeroed by alloc_cluster. */
    if (read_sectors(fs, loc->sector, 1, loc->buf) < 0) return -1;
    return 0;
}

static void fill_dir_entry(uint8_t *e, const uint8_t name83[11],
                           uint8_t attr, uint32_t cluster, uint32_t size)
{
    memset(e, 0, 32);
    memcpy(e, name83, 11);
    e[11] = attr;
    *(uint16_t *)(e + 20) = (uint16_t)((cluster >> 16) & 0xFFFF);
    *(uint16_t *)(e + 26) = (uint16_t)(cluster & 0xFFFF);
    *(uint32_t *)(e + 28) = size;
}

static int dir_find(fat_fs_t *fs, uint32_t dir_cluster, const char *leaf,
                    dir_locator_t *loc)
{
    uint8_t needle[11];
    if (make_short_name(leaf, needle) < 0) return -1;
    if (dir_scan(fs, dir_cluster, needle, loc) < 0) return -1;
    return loc->found ? 0 : -1;
}

static int dir_create_entry(fat_fs_t *fs, uint32_t dir_cluster,
                            const uint8_t name83[11], uint8_t attr,
                            uint32_t cluster, uint32_t size)
{
    /* Refuse duplicates. */
    dir_locator_t check;
    if (dir_scan(fs, dir_cluster, name83, &check) < 0) return -1;
    if (check.found) return -1;

    dir_locator_t slot;
    if (dir_scan(fs, dir_cluster, NULL, &slot) < 0) return -1;
    if (!slot.found) {
        if (dir_make_room(fs, &slot) < 0) return -1;
    }

    fill_dir_entry(slot.buf + slot.off, name83, attr, cluster, size);
    return write_sectors(fs, slot.sector, 1, slot.buf);
}

/* ----- public write API ------------------------------------------------- */

int fat_create(fat_fs_t *fs, const char *path)
{
    char parent[FAT_NAME_MAX], leaf[FAT_NAME_MAX];
    split_path(path, parent, sizeof(parent), leaf, sizeof(leaf));
    if (!leaf[0]) return -1;

    uint32_t pcl;
    if (dir_cluster_for_path(fs, parent, &pcl) < 0) return -1;

    uint8_t n83[11];
    if (make_short_name(leaf, n83) < 0) return -1;
    return dir_create_entry(fs, pcl, n83, ATTR_ARCHIVE, 0, 0);
}

int fat_write_file(fat_fs_t *fs, const char *path,
                   const void *buf, uint32_t len)
{
    char parent[FAT_NAME_MAX], leaf[FAT_NAME_MAX];
    split_path(path, parent, sizeof(parent), leaf, sizeof(leaf));
    if (!leaf[0]) return -1;

    uint32_t pcl;
    if (dir_cluster_for_path(fs, parent, &pcl) < 0) return -1;

    uint8_t n83[11];
    if (make_short_name(leaf, n83) < 0) return -1;

    /* Find or create the dir entry. */
    dir_locator_t loc;
    if (dir_scan(fs, pcl, n83, &loc) < 0) return -1;

    bool created = false;
    if (!loc.found) {
        if (dir_create_entry(fs, pcl, n83, ATTR_ARCHIVE, 0, 0) < 0) return -1;
        if (dir_scan(fs, pcl, n83, &loc) < 0 || !loc.found) return -1;
        created = true;
    }
    (void)created;

    uint8_t *e = loc.buf + loc.off;
    if (e[11] & ATTR_DIRECTORY) return -1;

    /* Free any existing chain. */
    uint16_t lo  = *(uint16_t *)(e + 26);
    uint16_t hi  = *(uint16_t *)(e + 20);
    uint32_t old = ((uint32_t)hi << 16) | lo;
    if (old >= 2 && old < 0x0FFFFFF8) free_cluster_chain(fs, old);

    /* Allocate fresh chain. */
    uint32_t bpc        = fs->sectors_per_cluster * 512;
    uint32_t need_clus  = (len + bpc - 1) / bpc;
    uint32_t first      = 0, prev = 0;

    for (uint32_t i = 0; i < need_clus; i++) {
        uint32_t c = alloc_cluster(fs);
        if (!c) return -1;
        if (i == 0) first = c;
        else        set_fat_entry(fs, prev, c);
        prev = c;
    }

    /* Write data cluster by cluster. */
    static uint8_t cbuf[4096];
    const uint8_t *src = (const uint8_t *)buf;
    uint32_t written = 0;
    uint32_t cur = first;
    while (written < len && cur >= 2 && cur < 0x0FFFFFF8) {
        uint32_t take = len - written;
        if (take > bpc) take = bpc;
        memset(cbuf, 0, bpc);
        memcpy(cbuf, src + written, take);
        uint32_t s0 = cluster_to_sector(fs, cur);
        if (write_sectors(fs, s0, fs->sectors_per_cluster, cbuf) < 0) return -1;
        written += take;
        cur = fat_next(fs, cur);
    }

    /* Update dir entry: cluster + size. */
    *(uint16_t *)(e + 20) = (uint16_t)((first >> 16) & 0xFFFF);
    *(uint16_t *)(e + 26) = (uint16_t)(first & 0xFFFF);
    *(uint32_t *)(e + 28) = len;
    return write_sectors(fs, loc.sector, 1, loc.buf);
}

int fat_unlink(fat_fs_t *fs, const char *path)
{
    char parent[FAT_NAME_MAX], leaf[FAT_NAME_MAX];
    split_path(path, parent, sizeof(parent), leaf, sizeof(leaf));
    if (!leaf[0]) return -1;

    uint32_t pcl;
    if (dir_cluster_for_path(fs, parent, &pcl) < 0) return -1;

    dir_locator_t loc;
    if (dir_find(fs, pcl, leaf, &loc) < 0) return -1;

    uint8_t *e = loc.buf + loc.off;
    if (e[11] & ATTR_DIRECTORY) return -1;       /* use rmdir */

    uint16_t lo = *(uint16_t *)(e + 26);
    uint16_t hi = *(uint16_t *)(e + 20);
    uint32_t cluster = ((uint32_t)hi << 16) | lo;
    if (cluster >= 2 && cluster < 0x0FFFFFF8) free_cluster_chain(fs, cluster);

    e[0] = 0xE5;
    return write_sectors(fs, loc.sector, 1, loc.buf);
}

int fat_mkdir(fat_fs_t *fs, const char *path)
{
    char parent[FAT_NAME_MAX], leaf[FAT_NAME_MAX];
    split_path(path, parent, sizeof(parent), leaf, sizeof(leaf));
    if (!leaf[0]) return -1;

    uint32_t pcl;
    if (dir_cluster_for_path(fs, parent, &pcl) < 0) return -1;

    uint32_t nc = alloc_cluster(fs);
    if (!nc) return -1;

    /* Initialise "." and ".." entries in the new cluster. */
    static uint8_t cbuf[4096];
    memset(cbuf, 0, sizeof(cbuf));
    uint8_t dot[11];     memset(dot, ' ', 11);  dot[0] = '.';
    uint8_t dotdot[11];  memset(dotdot, ' ', 11); dotdot[0] = '.'; dotdot[1] = '.';
    fill_dir_entry(cbuf,      dot,    ATTR_DIRECTORY, nc,  0);
    /* ".." points at the parent dir cluster (or 0 for the root). */
    uint32_t parent_cluster = (pcl == fs->root_cluster && fs->type == FAT_TYPE_32) ? 0 : pcl;
    fill_dir_entry(cbuf + 32, dotdot, ATTR_DIRECTORY, parent_cluster, 0);

    uint32_t s0 = cluster_to_sector(fs, nc);
    if (write_sectors(fs, s0, fs->sectors_per_cluster, cbuf) < 0) return -1;

    uint8_t n83[11];
    if (make_short_name(leaf, n83) < 0) return -1;
    return dir_create_entry(fs, pcl, n83, ATTR_DIRECTORY, nc, 0);
}

int fat_rmdir(fat_fs_t *fs, const char *path)
{
    char parent[FAT_NAME_MAX], leaf[FAT_NAME_MAX];
    split_path(path, parent, sizeof(parent), leaf, sizeof(leaf));
    if (!leaf[0]) return -1;

    uint32_t pcl;
    if (dir_cluster_for_path(fs, parent, &pcl) < 0) return -1;

    dir_locator_t loc;
    if (dir_find(fs, pcl, leaf, &loc) < 0) return -1;
    uint8_t *e = loc.buf + loc.off;
    if (!(e[11] & ATTR_DIRECTORY)) return -1;

    uint16_t lo = *(uint16_t *)(e + 26);
    uint16_t hi = *(uint16_t *)(e + 20);
    uint32_t target = ((uint32_t)hi << 16) | lo;

    /* Verify it only contains "." and ".." */
    fat_dir_t d;
    d.fs = fs; d.cluster = target; d.offset = 0; d.fat16_root = false;
    fat_entry_t fe;
    while (fat_readdir(&d, &fe) > 0) {
        if (!strcmp(fe.name, ".") || !strcmp(fe.name, "..")) continue;
        return -1;     /* not empty */
    }

    if (target >= 2 && target < 0x0FFFFFF8) free_cluster_chain(fs, target);
    e[0] = 0xE5;
    return write_sectors(fs, loc.sector, 1, loc.buf);
}
