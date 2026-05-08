#ifndef WILOS_KEYBOARD_H
#define WILOS_KEYBOARD_H

#include <wilos/types.h>

void keyboard_init(void);

/* Blocking read of one ASCII character from the keyboard. Returns 0 on
 * a key event that does not map to a printable character. */
char keyboard_getc(void);

#endif
