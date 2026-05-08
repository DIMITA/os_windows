#ifndef WILOS_PANIC_H
#define WILOS_PANIC_H

__attribute__((noreturn))
void panic(const char *fmt, ...);

#endif
