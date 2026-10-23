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
static unsigned near_cursor;
#include "unwind-copy.h"

/* Linux/x86-64 libgcc's opaque object fits in this conservative, 16-byte
 * aligned storage (same contract as the EXP-123 own-registry prototype).
 * The object must never move while registered. Refusals return it to a free
 * list; successful append-only generations retain it for their lifetime. */
static void (*register_info)(void *, void *);
static void *(*deregister_info)(void *);
struct registry_object { unsigned char bytes[512]; } __attribute__((aligned(16)));
static struct registry_object registry_objects[REFRESH_NEAR_SLOTS];
static unsigned free_objects[REFRESH_NEAR_SLOTS], free_count;
static void register_owned(void *frame) {
    /* prepare_frames reserves capacity before any registration. */
    unsigned slot = free_objects[--free_count];
    memset(&registry_objects[slot], 0, sizeof(registry_objects[slot]));
    register_info(frame, &registry_objects[slot]);
}
static void deregister_owned(void *frame) {
    void *object = deregister_info(frame);
    uintptr_t a = (uintptr_t)object, start = (uintptr_t)registry_objects;
    if (a >= start && a - start < sizeof(registry_objects) &&
        !((a - start) % sizeof(struct registry_object)) && free_count < REFRESH_NEAR_SLOTS)
        free_objects[free_count++] = (unsigned)((a - start) / sizeof(struct registry_object));
    /* An unexpected libgcc return is never reused; next preflight fails closed
     * on capacity, rather than corrupting another live registration. */
}
void unwind_init(void) {
    void *gcc = dlopen("libgcc_s.so.1", RTLD_NOW | RTLD_LOCAL);
    if (!gcc) return;
    register_info = dlsym(gcc, "__register_frame_info");
    deregister_info = dlsym(gcc, "__deregister_frame_info");
    if (!register_info || !deregister_info) return;
    for (unsigned i = 0; i < REFRESH_NEAR_SLOTS; ++i) free_objects[i] = i;
    free_count = REFRESH_NEAR_SLOTS;
    register_frame = register_owned;
    deregister_frame = deregister_owned;
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
        if (nframes + nstaged >= REFRESH_NEAR_SLOTS || (unsigned)nstaged >= free_count)
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
        int invalid = 0;
        /* PIE: stay below the image, opposite rustd's append-only growth.
         * ET_EXEC: use 64 KiB slots in the top 32 MiB of the rel32 window,
         * above every allowed code reservation (base end <=700 MiB).
         * NOREPLACE is authoritative if another thread maps a candidate meanwhile.
         * Rebase each candidate: proximity to the descriptor alone does not prove
         * all personality/LSDA/PC signed displacements are representable. */
        for (unsigned step = 0; step < REFRESH_NEAR_SLOTS; ++step) {
            unsigned k = (near_cursor + step) % REFRESH_NEAR_SLOTS;
            uintptr_t distance = (uintptr_t)(k + 1) * 0x200000;
            uintptr_t hint = load_base > 0x40000000ULL ? load_base - distance :
                             0x80000000ULL - (uintptr_t)(k + 1) * 0x10000;
            uintptr_t delta = hint > a ? hint - a : a - hint;
            if (delta > INT32_MAX - size) continue;
            buf = mmap((void *)hint, size, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
            if (buf == MAP_FAILED) continue;
            /* Fail closed on kernels that do not implement NOREPLACE. */
            if ((uintptr_t)buf != hint) { munmap(buf, size); buf = MAP_FAILED; continue; }
            memcpy(buf, (void *)a, span);
            if (!rebase121(buf, span, (intptr_t)buf - (intptr_t)a)) {
                near_cursor = (k + 1) % REFRESH_NEAR_SLOTS;
                break;
            }
            invalid = 1;
            munmap(buf, size); buf = MAP_FAILED;
        }
        if (buf == MAP_FAILED)
            return unwind_error(error, invalid ? REFRESH_UNWIND_INVALID : REFRESH_MMAP_FAILED,
                                invalid ? "unwind encoding or signed displacement invalid" :
                                          "near unwind mapping unavailable");
        staged_frames[nstaged] = (uintptr_t)buf;
        staged_sizes[nstaged++] = size;
        if (mprotect(buf, size, PROT_READ))
            return unwind_error(error, REFRESH_MPROTECT_FAILED, "unwind-copy mprotect failed");
    }
    return REFRESH_OK;
}
void register_staged(void) {
    if (disable_unwind) return;
    /* Own object storage avoids registration allocation. libgcc may still lock:
     * register before parking while new code is unreachable, with no exposure
     * gap between publishing entry jumps and installing unwind coverage. */
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

#ifdef REFRESH_TESTING
void forget_active_frame123(int index) {
    deregister_frame((void *)active_frames[index]);
    ++frame_deregistrations;
    retained_unwind_bytes -= active_sizes[index];
    munmap((void *)active_frames[index], active_sizes[index]);
    --nframes;
    active_frames[index] = active_frames[nframes];
    active_sizes[index] = active_sizes[nframes];
    frame_generation123[index] = frame_generation123[nframes];
}
#endif
