#ifndef REFRESH_PARK_H
#define REFRESH_PARK_H
#include <stdint.h>
extern uint64_t unregistered_threads;
int park_init(void);
int park(void);
void release(void);
#endif
