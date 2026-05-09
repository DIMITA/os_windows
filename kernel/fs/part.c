#include <wilos/part.h>
#include <wilos/ata.h>
#include <wilos/string.h>
#include <wilos/types.h>

/* Partition table parser. Supports MBR (4 primary entries) and GPT
 * (LBA 1 header + entry array). When the MBR holds a single 0xEE
 * "protective" entry, we automatically switch to GPT. */

const char *part_scheme_name(part_scheme_t s)
{
    switch (s) {
    case PART_SCHEME_MBR: return "MBR";
    case PART_SCHEME_GPT: return "GPT";
    default:              return "none";
    }
}

static const char *mbr_type_name(uint8_t t)
{
    switch (t) {
    case 0x00: return "empty";
    case 0x01: return "FAT12";
    case 0x04: return "FAT16<32M";
    case 0x05: return "extended";
    case 0x06: return "FAT16";
    case 0x07: return "NTFS/exFAT";
    case 0x0B: return "FAT32";
    case 0x0C: return "FAT32-LBA";
    case 0x0E: return "FAT16-LBA";
    case 0x0F: return "extended-LBA";
    case 0x82: return "Linux swap";
    case 0x83: return "Linux";
    case 0x8E: return "Linux LVM";
    case 0xA5: return "FreeBSD";
    case 0xAF: return "HFS+";
    case 0xEE: return "GPT-protective";
    case 0xEF: return "EFI System";
    case 0xFD: return "Linux RAID";
    default:   return "unknown";
    }
}

/* Common GPT type GUIDs: first 16 bytes (mixed-endian). */
static const struct {
    uint8_t guid[16];
    const char *name;
} gpt_types[] = {
    /* EFI System Partition C12A7328-F81F-11D2-BA4B-00A0C93EC93B */
    {{ 0x28,0x73,0x2A,0xC1, 0x1F,0xF8, 0xD2,0x11,
       0xBA,0x4B,0x00,0xA0,0xC9,0x3E,0xC9,0x3B }, "EFI System"},
    /* Microsoft Basic Data EBD0A0A2-B9E5-4433-87C0-68B6B72699C7 */
    {{ 0xA2,0xA0,0xD0,0xEB, 0xE5,0xB9, 0x33,0x44,
       0x87,0xC0,0x68,0xB6,0xB7,0x26,0x99,0xC7 }, "MS Basic Data"},
    /* Microsoft Reserved E3C9E316-0B5C-4DB8-817D-F92DF00215AE */
    {{ 0x16,0xE3,0xC9,0xE3, 0x5C,0x0B, 0xB8,0x4D,
       0x81,0x7D,0xF9,0x2D,0xF0,0x02,0x15,0xAE }, "MS Reserved"},
    /* Linux filesystem 0FC63DAF-8483-4772-8E79-3D69D8477DE4 */
    {{ 0xAF,0x3D,0xC6,0x0F, 0x83,0x84, 0x72,0x47,
       0x8E,0x79,0x3D,0x69,0xD8,0x47,0x7D,0xE4 }, "Linux FS"},
    /* Linux swap 0657FD6D-A4AB-43C4-84E5-0933C84B4F4F */
    {{ 0x6D,0xFD,0x57,0x06, 0xAB,0xA4, 0xC4,0x43,
       0x84,0xE5,0x09,0x33,0xC8,0x4B,0x4F,0x4F }, "Linux swap"},
    /* Apple HFS+ 48465300-0000-11AA-AA11-00306543ECAC */
    {{ 0x00,0x53,0x46,0x48, 0x00,0x00, 0xAA,0x11,
       0xAA,0x11,0x00,0x30,0x65,0x43,0xEC,0xAC }, "Apple HFS+"},
};

static const char *gpt_type_name(const uint8_t *guid)
{
    for (size_t i = 0; i < sizeof(gpt_types) / sizeof(gpt_types[0]); i++) {
        if (!memcmp(gpt_types[i].guid, guid, 16))
            return gpt_types[i].name;
    }
    return "unknown";
}

static void utf16le_to_ascii(const uint8_t *src, size_t pairs, char *dst, size_t cap)
{
    size_t j = 0;
    for (size_t i = 0; i < pairs && j + 1 < cap; i++) {
        uint16_t c = src[i * 2] | (src[i * 2 + 1] << 8);
        if (c == 0) break;
        dst[j++] = (c < 0x80) ? (char)c : '?';
    }
    dst[j] = '\0';
}

static int scan_gpt(size_t drive, part_table_t *out)
{
    uint8_t header[ATA_SECTOR_SIZE];
    if (ata_read(drive, 1, 1, header) < 0) return -1;
    if (memcmp(header, "EFI PART", 8) != 0) return -1;

    uint64_t entry_lba   = *(uint64_t *)(header + 72);
    uint32_t num_entries = *(uint32_t *)(header + 80);
    uint32_t entry_size  = *(uint32_t *)(header + 84);

    out->scheme = PART_SCHEME_GPT;
    out->count  = 0;

    /* Read entries one sector at a time. */
    if (entry_size == 0 || entry_size > 512) return -1;
    uint32_t per_sector = ATA_SECTOR_SIZE / entry_size;
    if (per_sector == 0) return -1;

    uint8_t sector[ATA_SECTOR_SIZE];
    for (uint32_t i = 0; i < num_entries && out->count < PART_MAX; i += per_sector) {
        if (ata_read(drive, entry_lba + i / per_sector, 1, sector) < 0) break;

        for (uint32_t k = 0; k < per_sector && i + k < num_entries; k++) {
            const uint8_t *e = sector + k * entry_size;
            bool empty = true;
            for (int b = 0; b < 16; b++) if (e[b]) { empty = false; break; }
            if (empty) continue;

            partition_t *p = &out->parts[out->count++];
            p->used      = true;
            p->lba_start = *(uint64_t *)(e + 32);
            uint64_t end = *(uint64_t *)(e + 40);
            p->lba_count = end - p->lba_start + 1;
            p->mbr_type  = 0;

            const char *tn = gpt_type_name(e);
            size_t n = strlen(tn);
            if (n > sizeof(p->type_name) - 1) n = sizeof(p->type_name) - 1;
            memcpy(p->type_name, tn, n);
            p->type_name[n] = '\0';

            utf16le_to_ascii(e + 56, 36, p->gpt_name, sizeof(p->gpt_name));
        }
    }
    return 0;
}

void part_scan(size_t drive, part_table_t *out)
{
    memset(out, 0, sizeof(*out));
    out->drive  = drive;
    out->scheme = PART_SCHEME_NONE;

    uint8_t mbr[ATA_SECTOR_SIZE];
    if (ata_read(drive, 0, 1, mbr) < 0) return;
    if (mbr[510] != 0x55 || mbr[511] != 0xAA) return;

    /* Detect protective MBR -> GPT. */
    bool is_gpt = false;
    for (int i = 0; i < 4; i++) {
        if (mbr[0x1BE + i * 16 + 4] == 0xEE) { is_gpt = true; break; }
    }
    if (is_gpt) {
        if (scan_gpt(drive, out) == 0) return;
    }

    /* Plain MBR. */
    out->scheme = PART_SCHEME_MBR;
    for (int i = 0; i < 4; i++) {
        const uint8_t *e = mbr + 0x1BE + i * 16;
        uint8_t  type    = e[4];
        uint32_t start   = *(uint32_t *)(e + 8);
        uint32_t count   = *(uint32_t *)(e + 12);
        if (type == 0 || count == 0) continue;

        partition_t *p = &out->parts[out->count++];
        p->used      = true;
        p->lba_start = start;
        p->lba_count = count;
        p->mbr_type  = type;
        const char *tn = mbr_type_name(type);
        size_t n = strlen(tn);
        if (n > sizeof(p->type_name) - 1) n = sizeof(p->type_name) - 1;
        memcpy(p->type_name, tn, n);
        p->type_name[n] = '\0';
        p->gpt_name[0]  = '\0';
    }
}
