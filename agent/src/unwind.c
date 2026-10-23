#include "internal.h"
#include "apply.h"
#include "unwind.h"

void (*register_frame)(void *), (*deregister_frame)(void *);
uintptr_t active_frames[MAX_RECORDS], staged_frames[MAX_RECORDS];
size_t active_sizes[MAX_RECORDS], staged_sizes[MAX_RECORDS];
int nframes, nstaged, disable_unwind;
int frame_generation123[MAX_RECORDS];
uint64_t frame_registrations, frame_deregistrations;
static int staged_registered;
static size_t retained_unwind_bytes;
#include "unwind-copy.h"

#if defined(REFRESH_TESTING) && defined(E123_OWN_REGISTRY)
static void (*register_info123)(void *, void *);
static unsigned char registry_info123[MAX_RECORDS][512] __attribute__((aligned(16)));
static int registry_cursor123;
static void register_owned123(void *f) {
    if (registry_cursor123 == MAX_RECORDS) abort();
    register_info123(f, registry_info123[registry_cursor123++]);
}
#endif

void unwind_init(void) {
    void *gcc = dlopen("libgcc_s.so.1", RTLD_NOW | RTLD_LOCAL);
    if (!gcc) return;
    register_frame = dlsym(gcc, "__register_frame");
    deregister_frame = dlsym(gcc, "__deregister_frame");
#if defined(REFRESH_TESTING) && defined(E123_OWN_REGISTRY)
    register_info123 = dlsym(gcc, "__register_frame_info");
    if (register_info123) {
        register_frame = register_owned123;
        deregister_frame = dlsym(gcc, "__deregister_frame_info");
    }
#endif
}
size_t unwind_bytes122(void) { return retained_unwind_bytes; }
void discard_staged(void) {
    for (int i = staged_registered - 1; i >= 0; --i) {
        deregister_frame((void *)staged_frames[i]);
        ++frame_deregistrations;
    }
    for (int i = 0; i < nstaged; ++i)
        munmap((void *)staged_frames[i], staged_sizes[i]);
    nstaged = staged_registered = 0;
}
static enum refresh_code unwind_error(char error[160], enum refresh_code code, const char *why) {
    snprintf(error, 160, "%s", why);
    return code;
}
enum refresh_code prepare_frames(char error[160]) {
    discard_staged();
    for (int i = 0; i < nrec; ++i) if (recs[i].kind == 3) {
        if (!register_frame || !deregister_frame)
            return unwind_error(error, REFRESH_UNWIND_UNAVAILABLE, "libgcc frame registration unavailable");
        if (nframes + nstaged == MAX_RECORDS)
            return unwind_error(error, REFRESH_UNWIND_LIMIT, "retained unwind limit");
        uint64_t span;
        memcpy(&span, recs[i].bytes, 8);
        uintptr_t a = recs[i].address;
        if (span < 8 || span > 1024 * 1024 || a > UINTPTR_MAX - span - 4095)
            return unwind_error(error, REFRESH_UNWIND_INVALID, "invalid unwind span");
        for (uintptr_t p = a & ~4095ULL; p < ((a + span + 4095) & ~4095ULL); p += 4096)
            if (!owned(p))
                return unwind_error(error, REFRESH_UNWIND_INVALID, "unwind region outside patch pages");
        size_t size = (span + 4 + 4095) & ~4095ULL;
        void *buf = MAP_FAILED;
        for (int k = 0; k < 512; ++k) {
            uintptr_t hint = load_base + 0x10000000ULL + (uintptr_t)k * 0x200000;
            buf = mmap((void *)hint, size, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
            if (buf != MAP_FAILED) break;
        }
        if (buf == MAP_FAILED)
            return unwind_error(error, REFRESH_MMAP_FAILED, "near unwind mapping unavailable");
        staged_frames[nstaged] = (uintptr_t)buf;
        staged_sizes[nstaged++] = size;
        /* kind-0 payloads and fixups are already present in final-protected pages. */
        memcpy(buf, (void *)a, span);
        if (rebase121(buf, span, (intptr_t)buf - (intptr_t)a))
            return unwind_error(error, REFRESH_UNWIND_INVALID, "unsupported unwind-copy encoding");
        if (mprotect(buf, size, PROT_READ))
            return unwind_error(error, REFRESH_MPROTECT_FAILED, "unwind-copy mprotect failed");
    }
    return REFRESH_OK;
}
void register_staged(void) {
    if (disable_unwind) return;
    /* Unreachable staged code: libgcc may allocate/lock here, before parking. */
    for (int i = 0; i < nstaged; ++i) {
#if defined(REFRESH_TESTING) && defined(E123_TRACK_REGISTRY)
        registry_scope123(1);
#endif
        register_frame((void *)staged_frames[i]);
#if defined(REFRESH_TESTING) && defined(E123_TRACK_REGISTRY)
        registry_scope123(0);
#endif
        ++staged_registered;
        ++frame_registrations;
    }
}
void retain_staged(void) {
    if (disable_unwind) { discard_staged(); return; }
    for (int i = 0; i < nstaged; ++i) {
        active_frames[nframes] = staged_frames[i];
        active_sizes[nframes] = staged_sizes[i];
        retained_unwind_bytes += staged_sizes[i];
        frame_generation123[nframes++] = (int)patch_count;
    }
    nstaged = staged_registered = 0;
}
