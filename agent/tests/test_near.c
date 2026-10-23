#include "internal.h"
#include "unwind.h"
#include <assert.h>
/* Link the production unwind allocator, with a minimal retained-source fixture. */
uintptr_t load_base;
struct rec recs[MAX_RECORDS];
int nrec;
uint64_t patch_count;
static uintptr_t source;
int owned(uintptr_t p) { return p == source; }
static void *fixed(uintptr_t a, size_t n) {
    void *p = mmap((void *)a, n, PROT_READ | PROT_WRITE,
                  MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
    assert(p == (void *)a);
    return p;
}
int main(void) {
    load_base = UINT64_C(0x500000000000);
    source = load_base + 0x20001000;
    unsigned char *data = fixed(source, 4096);
    /* CIE and FDE for a six-byte leaf one page below the descriptor. */
    const unsigned char cie[] = {20,0,0,0, 0,0,0,0, 1,'z','R',0, 1,0x78,16,1,
                                0x1b,0x0c,7,8,0x90,1,0,0};
    memcpy(data, cie, sizeof(cie));
    uint32_t fde[] = {16,28,(uint32_t)(int32_t)-4128,6,0};
    memcpy(data + sizeof(cie), fde, sizeof(fde));
    uint64_t span = sizeof(cie) + sizeof(fde);
    recs[0] = (struct rec){source, 8, PROT_READ, 3, (void *)&span};
    nrec = 1;
    uintptr_t old_slot = load_base + 0x10000000;
    fixed(old_slot, 4096);
    errno = 0;
    assert(mmap((void *)old_slot, 4096, PROT_READ,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0) == MAP_FAILED);
    assert(errno == EEXIST); /* old near-copy blocks an appended code page */
    assert(!munmap((void *)old_slot, 4096));
    /* Concurrent app mapping at the preferred new slot must be preserved. */
    unsigned char *occupied = fixed(load_base - 0x200000, 4096);
    occupied[0] = 0x5a;
    unwind_init();
    char error[160];
    assert(prepare_frames(error) == REFRESH_OK);
    assert(nstaged == 1 && staged_frames[0] < load_base - 0x200000);
    assert(occupied[0] == 0x5a);
    /* The same future generation address now remains available. */
    fixed(old_slot, 4096);
    register_staged();
    assert(frame_registrations == 1);
    discard_staged();
    assert(frame_deregistrations == 1 && nstaged == 0);
    /* More refusals than the entire pool: object slots must be recycled. */
    for (unsigned i = 0; i < REFRESH_NEAR_SLOTS + 10; ++i) {
        assert(prepare_frames(error) == REFRESH_OK);
        register_staged(); discard_staged();
    }
    assert(frame_registrations == frame_deregistrations);
    nframes = REFRESH_NEAR_SLOTS;
    assert(prepare_frames(error) == REFRESH_UNWIND_LIMIT);
    nframes = 0;
    munmap((void *)source, 4096);
    munmap((void *)old_slot, 4096);
    munmap(occupied, 4096);
    puts("near: old forward-slot EEXIST reproduced; backward copy, occupied-slot retry, registration cleanup and slot cap PASS");
}
