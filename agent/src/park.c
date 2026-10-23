/*
 * Allocation/lock audit (M3 section 3.8, production hybrid path):
 * Before gate=1: opendir/readdir/atoi/closedir enumerate tids. Stack registry
 * setup uses pthread_getattr_np, malloc/free and dlsym only outside a parked epoch.
 * Between gate=1 and release completion the complete call graph is:
 *   park: syscall(tgkill), getpid, clock_gettime, sched_yield, check_stacks;
 *   handler: atomic operations, ucontext copy (compiler memcpy), syscall(gettid,
 *            futex WAIT), bounded registry lookup;
 *   check_stacks: stamp -> clock_gettime, hazard/executable binary searches,
 *                 pread(/proc/self/mem), errno/TLS, bounded local copies/counters;
 *   apply commit/undo: memcpy, mprotect, x86 clear-cache (no-op), atomic fence;
 *   release: atomic stores/loads, syscall(futex WAKE), sched_yield.
 * No allocation/free, stdio, getenv, dynamic symbol lookup, or app/library locks
 * occur there. The library links with -z now to resolve PLT/IFUNCs before parking.
 * New pages, undo storage, original permissions, hazards and maps are prepared
 * first. libgcc register/deregister calls run BEFORE parking / AFTER release.
 * Error formatting and undo-storage/free/unmap cleanup run after release. A rollback
 * protection failure is fail-stop via syscall(exit_group) / trap fallback, never a false ACK.
 * Test-only CFI mode and E123 controlled GC are excluded from this production
 * audit: they retain research library calls, and are not release capabilities.
 */
#include "internal.h"
#include "park.h"
#include "hazard.h"

_Atomic int gate, parked, collision;
ucontext_t contexts[MAX_THREADS];
_Atomic int slots;
int memfd = -1;
static int park_signal;
struct thread_stack { _Atomic int tid; uintptr_t low, high; };
static struct thread_stack stacks[MAX_THREADS];
uintptr_t context_low[MAX_THREADS], context_high[MAX_THREADS];

int register_stack(void) {
    pthread_attr_t attr;
    void *base;
    size_t size;
    if (pthread_getattr_np(pthread_self(), &attr)) return -1;
    int rc = pthread_attr_getstack(&attr, &base, &size);
    pthread_attr_destroy(&attr);
    if (rc) return -1;
    for (int i = 0; i < MAX_THREADS; ++i) {
        int zero = 0;
        if (atomic_compare_exchange_strong(&stacks[i].tid, &zero, -1)) {
            stacks[i].low = (uintptr_t)base;
            stacks[i].high = (uintptr_t)base + size;
            atomic_store_explicit(&stacks[i].tid, syscall(SYS_gettid), memory_order_release);
            return i;
        }
    }
    return -1;
}
struct start_info { void *(*fn)(void *); void *arg; };
static int (*real_create)(pthread_t *, const pthread_attr_t *, void *(*)(void *), void *);
static void *start_thread(void *v) {
    struct start_info copy = *(struct start_info *)v;
    free(v);
    int slot = register_stack();
    void *result = copy.fn(copy.arg);
    if (slot >= 0) atomic_store(&stacks[slot].tid, 0);
    return result;
}
__attribute__((visibility("default")))
int pthread_create(pthread_t *t, const pthread_attr_t *attr, void *(*fn)(void *), void *arg) {
    if (!real_create) real_create = dlsym(RTLD_NEXT, "pthread_create");
    if (!real_create) return EAGAIN;
    struct start_info *v = malloc(sizeof(*v));
    if (!v) return ENOMEM;
    *v = (struct start_info){fn, arg};
    int rc = real_create(t, attr, start_thread, v);
    if (rc) free(v);
    return rc;
}
void handler(int sig, siginfo_t *info, void *ctx) {
    (void)sig; (void)info;
    if (!atomic_load_explicit(&gate, memory_order_acquire)) return;
    int saved_errno = errno;
    int slot = atomic_fetch_add(&slots, 1);
    if (slot < MAX_THREADS) {
        contexts[slot] = *(ucontext_t *)ctx;
        context_low[slot] = context_high[slot] = 0;
        int tid = syscall(SYS_gettid);
        for (int j = 0; j < MAX_THREADS; ++j)
            if (atomic_load_explicit(&stacks[j].tid, memory_order_acquire) == tid) {
                context_low[slot] = stacks[j].low;
                context_high[slot] = stacks[j].high;
                break;
            }
    } else atomic_store(&collision, 1);
    atomic_fetch_add_explicit(&parked, 1, memory_order_release);
    while (atomic_load_explicit(&gate, memory_order_acquire))
        syscall(SYS_futex, &gate, FUTEX_WAIT_PRIVATE, 1, NULL, NULL, 0);
    atomic_fetch_sub_explicit(&parked, 1, memory_order_release);
    errno = saved_errno;
}
void release(void) {
    atomic_store_explicit(&gate, 0, memory_order_release);
    syscall(SYS_futex, &gate, FUTEX_WAKE_PRIVATE, MAX_THREADS, NULL, NULL, 0);
    while (atomic_load(&parked)) sched_yield();
}
int park(void) {
    int tids[MAX_THREADS], n = 0, self = syscall(SYS_gettid);
    DIR *d = opendir("/proc/self/task");
    if (!d) return -1;
    struct dirent *e;
    while ((e = readdir(d))) {
        int tid = atoi(e->d_name);
        if (tid && tid != self) {
            if (n == MAX_THREADS) { closedir(d); return -1; }
            tids[n++] = tid;
        }
    }
    closedir(d);
    atomic_store(&collision, 0);
    atomic_store(&slots, 0);
    atomic_store(&gate, 1);
    int sent = 0;
    for (int i = 0; i < n; ++i)
        if (!syscall(SYS_tgkill, getpid(), tids[i], park_signal)) ++sent;
    struct timespec start, now;
    clock_gettime(CLOCK_MONOTONIC, &start);
    while (atomic_load(&parked) != sent) {
        clock_gettime(CLOCK_MONOTONIC, &now);
        if ((now.tv_sec - start.tv_sec) * 1e9 + now.tv_nsec - start.tv_nsec > 100000000) {
            release(); return -1;
        }
        sched_yield();
    }
    if (atomic_load(&collision) || check_stacks()) { release(); return -2; }
    return n;
}
int park_init(void) {
    park_signal = SIGRTMIN + 7;
    const char *chosen = getenv("REFRESH_PARK_SIGNAL");
    if (chosen) {
        char *end;
        long value = strtol(chosen, &end, 10);
        if (!chosen[0] || *end || value <= 0 || value >= NSIG || value == SIGKILL || value == SIGSTOP) {
            fputs("cargo-refresh: invalid REFRESH_PARK_SIGNAL\n", stderr); return -1;
        }
        park_signal = (int)value;
    }
    struct sigaction previous;
    if (sigaction(park_signal, NULL, &previous)) return -1;
    if (previous.sa_handler != SIG_DFL && previous.sa_handler != SIG_IGN)
        fputs("cargo-refresh: parking signal already has an application handler\n", stderr);
    if (register_stack() < 0) return -1;
    memfd = open("/proc/self/mem", O_RDONLY | O_CLOEXEC);
    if (memfd < 0) return -1;
#ifdef REFRESH_TESTING
    /* The optional research CFI mode uses libunwind; release is always hybrid. */
    unw_context_t warm;
    unw_cursor_t cur;
    unw_getcontext(&warm); unw_init_local(&cur, &warm); unw_step(&cur);
    unw_word_t ip; unw_get_reg(&cur, UNW_REG_IP, &ip);
#endif
    struct sigaction sa = {.sa_sigaction = handler, .sa_flags = SA_SIGINFO | SA_RESTART};
    sigemptyset(&sa.sa_mask);
    return sigaction(park_signal, &sa, NULL);
}
