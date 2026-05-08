#ifndef WILOS_PIT_H
#define WILOS_PIT_H

#include <wilos/types.h>

void     pit_init(uint32_t hz);
uint64_t pit_ticks(void);
void     pit_sleep_ms(uint32_t ms);

#endif
