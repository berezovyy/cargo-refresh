#ifndef REFRESH_APPLY_H
#define REFRESH_APPLY_H
#include "protocol.h"
enum refresh_code apply(char error[160], int *threads);
int owned(uintptr_t page);
/* Compatibility diagnostics are zero in release, populated only by test scans. */
struct refresh_scan_stats {
    uint64_t first_root, bytes, hits;
    double us;
    int retained, candidates, errors, reclaimed;
};
void apply_scan_stats(struct refresh_scan_stats *stats);
#endif
