#include "internal.h"
#include "apply.h"
#include "hazard.h"
#include "park.h"
#include "unwind.h"
#include <sys/sysmacros.h>

uintptr_t pages[MAX_PAGES];
int npages;
uint64_t patch_bytes_total, patch_count;

/* Open addressing keeps retained-page checks independent of session length. */
#define PAGE_SLOTS (MAX_PAGES * 2)
struct page_slot { uintptr_t address; unsigned prot; };
static struct page_slot page_index[PAGE_SLOTS];
static size_t page_hash(uintptr_t p) {
    return ((p >> 12) * UINT64_C(11400714819323198485)) & (PAGE_SLOTS - 1);
}
static struct page_slot *find_page(uintptr_t p) {
    size_t at = page_hash(p);
    while (page_index[at].address && page_index[at].address != p)
        at = (at + 1) & (PAGE_SLOTS - 1);
    return &page_index[at];
}
int owned(uintptr_t p) { return p && find_page(p)->address == p; }
static void remove_page(uintptr_t p) {
    struct page_slot *slot = find_page(p);
    if (slot->address != p) return;
    size_t at = (size_t)(slot - page_index);
    page_index[at] = (struct page_slot){0};
    at = (at + 1) & (PAGE_SLOTS - 1);
    /* Reinsert the following cluster; no tombstones accumulate on refusals. */
    while (page_index[at].address) {
        struct page_slot saved = page_index[at];
        page_index[at] = (struct page_slot){0};
        *find_page(saved.address) = saved;
        at = (at + 1) & (PAGE_SLOTS - 1);
    }
}
static void discard_pages(int before) {
    while (npages > before) {
        uintptr_t p = pages[--npages];
        munmap((void *)p, 4096);
        remove_page(p);
    }
}
struct undo_page { uintptr_t address; int prot; };
struct undo_record {
    unsigned char *bytes;
    struct undo_page pages[2];
    int npages, touched;
};
static enum refresh_code fail(char error[160], enum refresh_code code, const char *why) {
    snprintf(error, 160, "%s", why);
    return code;
}

/* Resolve and validate every original page/protection while threads are running. */
static enum refresh_code prepare_undo(struct undo_record *undo, unsigned char *bytes,
                                       char error[160]) {
    struct stat executable;
    if (stat("/proc/self/exe", &executable))
        return fail(error, REFRESH_INTERNAL, "cannot stat original executable");
    for (int i = 0; i < nrec; ++i) if (recs[i].kind == 1 || recs[i].kind == 2) {
        undo[i].bytes = bytes;
        bytes += recs[i].length;
        uintptr_t end = (recs[i].address + recs[i].length + 4095) & ~4095ULL;
        for (uintptr_t a = recs[i].address & ~4095ULL; a < end; a += 4096) {
            if (undo[i].npages == 2) return fail(error, REFRESH_BAD_PROTOCOL, "existing write spans too many pages");
            undo[i].pages[undo[i].npages++].address = a;
        }
    }
    FILE *f = fopen("/proc/self/maps", "r");
    if (!f) return fail(error, REFRESH_INTERNAL, "cannot read original mappings");
    char line[1024], perms[8];
    unsigned long start, end, offset, inode;
    unsigned devmajor, devminor;
    while (fgets(line, sizeof(line), f)) {
        if (sscanf(line, "%lx-%lx %7s %lx %x:%x %lu", &start, &end, perms,
                   &offset, &devmajor, &devminor, &inode) != 7) continue;
        if (inode != (unsigned long)executable.st_ino ||
            makedev(devmajor, devminor) != executable.st_dev) continue;
        for (int i = 0; i < nrec; ++i) if (recs[i].kind == 1 || recs[i].kind == 2) {
            const char *expected = recs[i].kind == 1 ? "r-xp" : "r--p";
            if (strcmp(perms, expected)) continue;
            for (int j = 0; j < undo[i].npages; ++j) {
                struct undo_page *p = &undo[i].pages[j];
                if (p->address >= start && p->address < end && end - p->address >= 4096)
                    p->prot = recs[i].kind == 1 ? PROT_READ | PROT_EXEC : PROT_READ;
            }
        }
    }
    fclose(f);
    for (int i = 0; i < nrec; ++i)
        for (int j = 0; j < undo[i].npages; ++j) if (!undo[i].pages[j].prot)
            return fail(error, recs[i].kind == 2 ? REFRESH_NOT_ORIGINAL_RO : REFRESH_BAD_PROTOCOL,
                        recs[i].kind == 2 ? "constant record is not original read-only executable memory" :
                                           "entry record is not original executable text");
    return REFRESH_OK;
}

static _Noreturn void rollback_fatal(void) {
    syscall(SYS_exit_group, 127);
    /* Also fail closed if an application seccomp policy blocks exit_group. */
    __builtin_trap();
}
/* No allocation, formatting or libgcc calls. Failure to restore is fail-stop. */
static void rollback(struct undo_record *undo) {
    for (int i = nrec - 1; i >= 0; --i) if (undo[i].touched) {
        for (int j = 0; j < undo[i].npages; ++j) {
            struct undo_page *p = &undo[i].pages[j];
            if (mprotect((void *)p->address, 4096, p->prot | PROT_WRITE))
                rollback_fatal();
        }
        memcpy((void *)recs[i].address, undo[i].bytes, recs[i].length);
        __builtin___clear_cache((char *)recs[i].address, (char *)recs[i].address + recs[i].length);
        for (int j = 0; j < undo[i].npages; ++j) {
            struct undo_page *p = &undo[i].pages[j];
            if (mprotect((void *)p->address, 4096, p->prot))
                rollback_fatal();
        }
    }
}

#if defined(REFRESH_TESTING) && defined(E123_TRACK_REGISTRY)
#include "registry_alloc123.h"
void registry_scope123(int enabled) { registry_allocating123 = enabled; }
#endif
#ifdef REFRESH_TESTING
#include "scan123.h"
#endif

void apply_scan_stats(struct refresh_scan_stats *stats) {
    *stats = (struct refresh_scan_stats){0};
#ifdef REFRESH_TESTING
    stats->first_root = first_root123[0]; stats->us = scan_us123;
    stats->bytes = scan_bytes123; stats->hits = scan_hits123;
    stats->retained = scan_retained123; stats->candidates = scan_candidates123;
    stats->errors = scan_errors123; stats->reclaimed = reclaimed123;
#endif
}

enum refresh_code apply(char error[160], int *threads) {
    attempts = retries = pc_hits = fp_hits = scan_hits = fp_frames = scan_words = 0;
    fp_incomplete = cfi_frames = cfi_fail = 0;
    read_errors = read_errno = 0; badsp = badhigh = 0;
    unwind_us = park_us = 0;
    *threads = 0;
    const int before = npages;
    enum refresh_code code = REFRESH_OK;
    int fail_at = -1, fail_phase = 0;
#ifdef REFRESH_TESTING
    const char *inject = getenv("REFRESH_TESTING_FAIL_AT");
    if (inject) {
        char *end;
        long index = strtol(inject, &end, 10);
        if (inject[0] && !*end && index >= 0 && index < nrec) fail_at = (int)index;
    }
    const char *phase = getenv("REFRESH_TESTING_FAIL_PHASE");
    if (phase && !strcmp(phase, "write")) fail_phase = 1;
    if (phase && !strcmp(phase, "restore")) fail_phase = 2;
#endif
    size_t undo_size = 0;
    for (int i = 0; i < nrec; ++i)
        if (recs[i].kind == 1 || recs[i].kind == 2) undo_size += recs[i].length;
    struct undo_record *undo = calloc(nrec ? (size_t)nrec : 1, sizeof(*undo));
    unsigned char *old_bytes = malloc(undo_size ? undo_size : 1);
    if (!undo || !old_bytes) {
        free(undo); free(old_bytes);
        return fail(error, REFRESH_INTERNAL, "undo allocation failed");
    }
    code = prepare_undo(undo, old_bytes, error);
    if (code != REFRESH_OK) goto refuse;
    /* Refuse rewrites of any retained generation before creating new mappings. */
    for (int i = 0; i < nrec; ++i) if (recs[i].kind == 0) {
        uintptr_t end = (recs[i].address + recs[i].length + 4095) & ~4095ULL;
        for (uintptr_t a = recs[i].address & ~4095ULL; a < end; a += 4096)
            if (owned(a)) {
                code = fail(error, REFRESH_OVERLAP, "append-only generation overlaps retained page");
                goto refuse;
            }
    }
    /* Allocate all new pages RW. Records may share a page only with equal final permissions. */
    for (int i = 0; i < nrec; ++i) if (recs[i].kind == 0) {
        uintptr_t end = (recs[i].address + recs[i].length + 4095) & ~4095ULL;
        for (uintptr_t a = recs[i].address & ~4095ULL; a < end; a += 4096) {
            struct page_slot *slot = find_page(a);
            if (slot->address) {
                if (slot->prot != recs[i].prot) {
                    code = fail(error, REFRESH_BAD_PROTOCOL, "conflicting final permissions on staged page");
                    goto refuse;
                }
                continue;
            }
            if (npages == MAX_PAGES) {
                code = fail(error, REFRESH_PAGE_LIMIT, "retained page limit"); goto refuse;
            }
            void *mapped = mmap((void *)a, 4096, PROT_READ | PROT_WRITE,
                                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
            if (mapped == MAP_FAILED) {
                snprintf(error, 160, "mmap %lx: %s", a, strerror(errno));
                code = REFRESH_MMAP_FAILED; goto refuse;
            }
            *slot = (struct page_slot){a, recs[i].prot};
            pages[npages++] = a;
        }
    }
    for (int i = 0; i < nrec; ++i) if (recs[i].kind == 0)
        memcpy((void *)recs[i].address, recs[i].bytes, recs[i].length);
    for (int i = before; i < npages; ++i) {
        uintptr_t a = pages[i];
        __builtin___clear_cache((char *)a, (char *)a + 4096);
        if (mprotect((void *)a, 4096, find_page(a)->prot)) {
            code = fail(error, REFRESH_MPROTECT_FAILED, "staged page mprotect failed"); goto refuse;
        }
    }
    if (fail_at >= 0 && recs[fail_at].kind == 0) {
        code = fail(error, REFRESH_ROLLED_BACK, "injected staged record failure"); goto refuse;
    }
    code = prepare_frames(error);
    if (code != REFRESH_OK) goto refuse;
    register_staged();
    if (fail_at >= 0 && recs[fail_at].kind == 3) {
        code = fail(error, REFRESH_ROLLED_BACK, "injected unwind descriptor failure"); goto refuse;
    }
#ifdef REFRESH_TESTING
    prepare_scan123();
#endif
    if (prepare_stacks()) {
        code = fail(error, REFRESH_INTERNAL, "stack map preparation failed"); goto refuse;
    }
    int n = -1;
    for (int attempt = 0; attempt < 100; ++attempt) {
        ++attempts;
        double pstart = stamp();
        n = park();
        park_us += stamp() - pstart;
        if (n >= 0) break;
        ++retries;
        if (n == -1) {
            code = fail(error, REFRESH_PARK_TIMEOUT, "park timeout or thread limit"); goto refuse;
        }
        usleep(1000);
    }
    if (n < 0) {
        code = fail(error, REFRESH_STACK_COLLISION, "stack collision retry limit"); goto refuse;
    }
    *threads = n;
    /* Only original entry jumps and validated small RO writes are reachable here. */
    int failed = 0;
    for (int i = 0; i < nrec; ++i) if (recs[i].kind == 1 || recs[i].kind == 2) {
        memcpy(undo[i].bytes, (void *)recs[i].address, recs[i].length);
        undo[i].touched = 1;
        if (i == fail_at && fail_phase == 1) { failed = 1; break; }
        for (int j = 0; j < undo[i].npages; ++j) {
            struct undo_page *p = &undo[i].pages[j];
            if (mprotect((void *)p->address, 4096, p->prot | PROT_WRITE)) { failed = 1; break; }
        }
        if (failed) break;
        memcpy((void *)recs[i].address, recs[i].bytes, recs[i].length);
        __builtin___clear_cache((char *)recs[i].address, (char *)recs[i].address + recs[i].length);
        if (i == fail_at && fail_phase == 2) { failed = 1; break; }
        for (int j = 0; j < undo[i].npages; ++j) {
            struct undo_page *p = &undo[i].pages[j];
            if (mprotect((void *)p->address, 4096, p->prot)) { failed = 1; break; }
        }
        if (failed || i == fail_at) { failed = 1; break; }
    }
    if (failed) rollback(undo);
#ifdef REFRESH_TESTING
    if (!failed) {
        record_generation123();
        /* Research diagnostics are test-only; default test builds do not enable GC. */
        if (nroots123) scan123();
    }
#endif
    atomic_thread_fence(memory_order_seq_cst);
    release();
    if (failed) {
        code = fail(error, REFRESH_ROLLED_BACK, "commit failed; original bytes and protections restored");
        goto refuse;
    }
    retain_staged();
    free(old_bytes); free(undo);
    return REFRESH_OK;
refuse:
    /* Every path here is before park or after release, so cleanup may allocate/lock. */
    discard_staged();
    discard_pages(before);
    free(old_bytes); free(undo);
    return code;
}
