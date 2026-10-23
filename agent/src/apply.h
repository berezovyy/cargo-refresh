#ifndef REFRESH_APPLY_H
#define REFRESH_APPLY_H
#include "protocol.h"
enum refresh_code apply(char error[160], int *threads);
int owned(uintptr_t page);
int adopt_inherited(const char *spec);
int adopt_region_segment(uintptr_t start, uintptr_t end, unsigned prot);
struct refresh_conflict { uintptr_t address, start, end; };
extern struct refresh_conflict apply_conflict;
/* Compatibility diagnostics are zero in release, populated only by test scans. */
struct refresh_scan_stats {
    uint64_t first_root, bytes, hits;
    double us;
    int retained, candidates, errors, reclaimed;
};
void apply_scan_stats(struct refresh_scan_stats *stats);
#endif
