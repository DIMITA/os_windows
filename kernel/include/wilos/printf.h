#ifndef WILOS_PRINTF_H
#define WILOS_PRINTF_H

#include <wilos/types.h>
#include <stdarg.h>

int kprintf(const char *fmt, ...);
int kvprintf(const char *fmt, va_list ap);

#endif
