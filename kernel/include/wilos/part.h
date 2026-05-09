#ifndef WILOS_PART_H
#define WILOS_PART_H

#include <wilos/types.h>

#define PART_MAX 16

typedef enum {
    PART_SCHEME_NONE = 0,
    PART_SCHEME_MBR,
    PART_SCHEME_GPT,
} part_scheme_t;

typedef struct {
    bool         used;
    uint64_t     lba_start;
    uint64_t     lba_count;
    uint8_t      mbr_type;        /* MBR partition type byte (when MBR)  */
    char         gpt_name[37];    /* UTF-8 name, only for GPT entries    */
    char         type_name[32];   /* human label for the partition type  */
} partition_t;

typedef struct {
    size_t        drive;
    part_scheme_t scheme;
    size_t        count;
    partition_t   parts[PART_MAX];
} part_table_t;

void part_scan(size_t drive, part_table_t *out);
const char *part_scheme_name(part_scheme_t s);

#endif
