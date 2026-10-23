#ifndef REFRESH_REGION_H
#define REFRESH_REGION_H
#include <stdint.h>
extern uintptr_t region_start, region_end;
extern int region_ok;
void region_init(void);
int region_contains(uintptr_t page);
uint64_t region_used(void);
#endif
