/*
 * Allocation/lock audit (M3 section 3.8, production hybrid path):
 * Before gate=1: take the private creation rwlock, open /proc/self/task and
 * enumerate via getdents64. The wrapper holds a read lock through the child's
 * stack-registration handshake. Stack setup uses pthread_getattr_np, malloc/
 * free and dlsym only before parking. No user code runs under that read lock.
 * Between gate=1 and release completion the complete call graph is:
 *   park: syscall(tgkill/getdents64/lseek), close, getpid, clock_gettime,
 *         sched_yield, check_stacks;
 *   handler: atomic operations, ucontext copy (compiler memcpy), syscall(gettid,
 *            futex WAIT), bounded registry lookup;
 *   check_stacks: stamp -> clock_gettime, hazard/executable binary searches,
 *                 pread(/proc/self/mem), errno/TLS, bounded local copies/counters;
 *   apply commit/undo: memcpy, mprotect, x86 clear-cache (no-op), atomic fence;
 *   release: atomic stores/loads, syscall(futex WAKE), sched_yield, then unlock
 *            the private creation rwlock once every parked handler has left.
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
static _Atomic int handlers_active;
#ifdef REFRESH_TESTING
static _Atomic int delay_once;
static void (*after_task_list)(void);
#endif
int memfd = -1;
static int park_signal;
/* Only the agent and its pthread wrapper use this lock. Never acquire an app lock. */
static pthread_rwlock_t creation_gate = PTHREAD_RWLOCK_WRITER_NONRECURSIVE_INITIALIZER_NP;
static int creation_locked;
static int context_tids[MAX_THREADS];
uint64_t unregistered_threads;

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
struct start_info { void *(*fn)(void *); void *arg; _Atomic int ready; };
static int (*real_create)(pthread_t *, const pthread_attr_t *, void *(*)(void *), void *);
static pthread_once_t create_once = PTHREAD_ONCE_INIT;
static void resolve_create(void) { real_create = dlsym(RTLD_NEXT, "pthread_create"); }
static void unregister_stack(void *value) {
    int slot = (int)(intptr_t)value;
    pthread_rwlock_rdlock(&creation_gate);
    if (slot >= 0) atomic_store_explicit(&stacks[slot].tid, 0, memory_order_release);
    pthread_rwlock_unlock(&creation_gate);
}
static void *start_thread(void *v) {
    struct start_info *start = v;
    void *(*fn)(void *) = start->fn;
    void *arg = start->arg;
    /* Parent holds the read side until registration is complete. Do not take
     * another read lock before acknowledging: a queued writer would block it,
     * leaving that writer waiting on the parent, and parent waiting on us. */
    int slot = register_stack();
    atomic_store_explicit(&start->ready, 1, memory_order_release);
    /* A writer queued during registration runs before this child enters user
     * code. The registered stack is safe to park while this barrier waits. */
    pthread_rwlock_rdlock(&creation_gate);
    pthread_rwlock_unlock(&creation_gate);
    void *result;
    pthread_cleanup_push(unregister_stack, (void *)(intptr_t)slot);
    result = fn(arg);
    pthread_cleanup_pop(1);
    return result;
}
__attribute__((visibility("default")))
int pthread_create(pthread_t *t, const pthread_attr_t *attr, void *(*fn)(void *), void *arg) {
    pthread_once(&create_once, resolve_create);
    if (!real_create) return EAGAIN;
    struct start_info *v = malloc(sizeof(*v));
    if (!v) return ENOMEM;
    v->fn = fn; v->arg = arg; atomic_init(&v->ready, 0);
    int cancellation;
    pthread_setcancelstate(PTHREAD_CANCEL_DISABLE, &cancellation);
    pthread_rwlock_rdlock(&creation_gate);
    int rc = real_create(t, attr, start_thread, v);
    if (!rc) while (!atomic_load_explicit(&v->ready, memory_order_acquire)) sched_yield();
    pthread_rwlock_unlock(&creation_gate);
    free(v);
    pthread_setcancelstate(cancellation, NULL);
    return rc;
}
void handler(int sig, siginfo_t *info, void *ctx) {
    (void)sig; (void)info;
    int saved_errno = errno;
    /* Count entry before reading gate: release must also wait for a handler
     * delayed between its gate load and publication of the saved context. */
    atomic_fetch_add_explicit(&handlers_active, 1, memory_order_seq_cst);
    if (!atomic_load_explicit(&gate, memory_order_seq_cst)) goto done;
#ifdef REFRESH_TESTING
    int delay = atomic_exchange(&delay_once, 0);
    if (delay) { double until = stamp() + delay; while (stamp() < until) {} }
#endif
    if (!atomic_load_explicit(&gate, memory_order_seq_cst)) goto done;
    int slot = atomic_fetch_add(&slots, 1);
    if (slot < MAX_THREADS) {
        contexts[slot] = *(ucontext_t *)ctx;
        context_tids[slot] = syscall(SYS_gettid);
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
done:
    atomic_fetch_sub_explicit(&handlers_active, 1, memory_order_seq_cst);
    errno = saved_errno;
}
void release(void) {
    atomic_store_explicit(&gate, 0, memory_order_seq_cst);
    syscall(SYS_futex, &gate, FUTEX_WAKE_PRIVATE, MAX_THREADS, NULL, NULL, 0);
    while (atomic_load_explicit(&handlers_active, memory_order_seq_cst)) sched_yield();
    if (creation_locked) {
        creation_locked = 0;
        pthread_rwlock_unlock(&creation_gate);
    }
}
/* Kernel task enumeration works even when a parked thread owns malloc/stdio.
 * The directory fd is opened before gate=1; second pass uses only raw syscalls. */
struct linux_dirent64 {
    uint64_t ino; int64_t off; unsigned short reclen; unsigned char type; char name[];
};
static int list_tasks(int fd, int self, int tids[MAX_THREADS]) {
    unsigned char buffer[8192] __attribute__((aligned(8)));
    int count = 0;
    if (syscall(SYS_lseek, fd, 0, SEEK_SET) < 0) return -1;
    for (;;) {
        long got = syscall(SYS_getdents64, fd, buffer, sizeof(buffer));
        if (!got) return count;
        if (got < 0) { if (errno == EINTR) continue; return -1; }
        for (long at = 0; at < got;) {
            struct linux_dirent64 *entry = (void *)(buffer + at);
            size_t prefix = offsetof(struct linux_dirent64, name);
            if ((size_t)(got - at) < prefix + 1 || entry->reclen < prefix + 1 ||
                entry->reclen > got - at) return -1;
            int tid = 0;
            size_t k = 0, len = entry->reclen - prefix;
            while (k < len && entry->name[k] >= '0' && entry->name[k] <= '9') {
                if (tid > (INT32_MAX - 9) / 10) return -1;
                tid = tid * 10 + entry->name[k++] - '0';
            }
            if (k < len && !entry->name[k] && tid && tid != self) {
                if (count == MAX_THREADS) return -1;
                tids[count++] = tid;
            }
            at += entry->reclen;
        }
    }
}
int park(void) {
    int tids[MAX_THREADS], self = syscall(SYS_gettid);
    /* No child can enter/leave user code across enumeration and commit. */
    struct timespec deadline;
    if (clock_gettime(CLOCK_REALTIME, &deadline)) return -1;
    deadline.tv_nsec += 100000000;
    if (deadline.tv_nsec >= 1000000000) { ++deadline.tv_sec; deadline.tv_nsec -= 1000000000; }
    /* Queue a writer instead of trylock spinning: incoming creators must not
     * starve parking. Parent/child handshakes finish without a recursive read. */
    if (pthread_rwlock_timedwrlock(&creation_gate, &deadline)) return -1;
    creation_locked = 1;
    int fd = open("/proc/self/task", O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    if (fd < 0) { release(); return -1; }
    int n = list_tasks(fd, self, tids);
    if (n < 0) { close(fd); release(); return -1; }
#ifdef REFRESH_TESTING
    if (after_task_list) after_task_list();
#endif
    atomic_store(&collision, 0);
    atomic_store(&slots, 0);
    atomic_store(&gate, 1);
    int sent = 0, targets[MAX_THREADS];
    for (int i = 0; i < n; ++i)
        if (!syscall(SYS_tgkill, getpid(), tids[i], park_signal)) targets[sent++] = tids[i];
    int expected = sent;
    struct timespec start, now;
    clock_gettime(CLOCK_MONOTONIC, &start);
    long long next_exit_check = 1000000;
    while (atomic_load_explicit(&parked, memory_order_acquire) != expected) {
        clock_gettime(CLOCK_MONOTONIC, &now);
        long long elapsed = (now.tv_sec - start.tv_sec) * 1000000000LL + now.tv_nsec - start.tv_nsec;
        /* tgkill may succeed just before a child exits without taking its
         * signal. Such a dead target cannot acknowledge parking. A handler
         * already parked cannot exit while gate=1, so dropping only ESRCH
         * targets preserves the acknowledgement invariant. */
        if (elapsed >= next_exit_check) {
            for (int i = 0; i < sent; ++i) if (targets[i] &&
                syscall(SYS_tgkill, getpid(), targets[i], 0) < 0 && errno == ESRCH) {
                targets[i] = 0; --expected;
            }
            next_exit_check = elapsed + 1000000;
        }
        if (elapsed > 100000000 && atomic_load_explicit(&parked, memory_order_acquire) != expected) {
            close(fd); release(); return -1;
        }
        sched_yield();
    }
    /* Detect raw clone users that bypass the interposed creation wrapper. */
    n = list_tasks(fd, self, tids);
    close(fd);
    if (n < 0) { release(); return -1; }
    int count = atomic_load_explicit(&slots, memory_order_acquire);
    if (count > MAX_THREADS || count != expected) { release(); return -1; }
    for (int i = 0; i < count; ++i) if (!context_high[i]) ++unregistered_threads;
    for (int i = 0; i < n; ++i) {
        int found = 0;
        for (int j = 0; j < count; ++j) if (tids[i] == context_tids[j]) { found = 1; break; }
        if (!found) { release(); return -3; }
    }
    if (unregistered_threads) { release(); return -4; }
    if (atomic_load(&collision) || check_stacks()) { release(); return -2; }
    return count;
}
int park_init(void) {
#ifdef REFRESH_TESTING
    if (getenv("REFRESH_TESTING_TASK_RACE"))
        after_task_list = (void (*)(void))dlsym(RTLD_DEFAULT, "refresh_testing_after_task_list");
    const char *delay = getenv("REFRESH_TESTING_HANDLER_DELAY_US");
    if (delay) {
        char *end; long us = strtol(delay, &end, 10);
        if (*delay && !*end && us > 0 && us <= 200000) atomic_store(&delay_once, (int)us);
    }
#endif
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
