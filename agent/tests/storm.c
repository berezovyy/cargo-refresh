#define main ordinary_host_main
#include "host.c"
#undef main
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <sys/syscall.h>
#include <sched.h>
#include <errno.h>
#include <string.h>
#include <time.h>
static _Atomic unsigned long created, exited, epoll_eintr, sleep_eintr, events, sleeps;
static int event_fd, epoll_fd;
static void *short_lived(void *arg) {
    for (unsigned j = 0; j < 32; ++j) {
        int value = f();
        if (value != 1 && value != 2) atomic_fetch_add(&errors, 1);
    }
    uint64_t one = 1;
    if (write(event_fd, &one, sizeof(one)) < 0 && errno != EAGAIN && errno != EINTR)
        atomic_fetch_add(&errors, 1);
    atomic_fetch_add(&exited, 1);
    if (arg) pthread_exit(NULL); /* exercise wrapper cleanup on non-return */
    return NULL;
}
static void *create_loop(void *unused) {
    (void)unused;
    unsigned n = 0;
    while (atomic_load(&running)) {
        pthread_t t;
        if (pthread_create(&t, NULL, short_lived, (void *)(uintptr_t)(n++ & 1))) {
            atomic_fetch_add(&errors, 1); break;
        }
        atomic_fetch_add(&created, 1);
        if (pthread_join(t, NULL)) atomic_fetch_add(&errors, 1);
    }
    return NULL;
}
static void *epoll_loop(void *unused) {
    (void)unused;
    struct epoll_event ev;
    while (atomic_load(&running)) {
        int n = epoll_wait(epoll_fd, &ev, 1, 10);
        if (n < 0) {
            if (errno == EINTR) { atomic_fetch_add(&epoll_eintr, 1); continue; }
            atomic_fetch_add(&errors, 1); break;
        }
        if (n) {
            uint64_t value;
            ssize_t got = read(event_fd, &value, sizeof(value));
            if (got == sizeof(value)) atomic_fetch_add(&events, value);
            else if (got < 0 && errno != EAGAIN && errno != EINTR) atomic_fetch_add(&errors, 1);
        }
    }
    return NULL;
}
static void *sleep_loop(void *unused) {
    (void)unused;
    while (atomic_load(&running)) {
        struct timespec remaining = {.tv_nsec = 1000000};
        while (nanosleep(&remaining, &remaining)) {
            if (errno != EINTR) { atomic_fetch_add(&errors, 1); return NULL; }
            atomic_fetch_add(&sleep_eintr, 1);
        }
        atomic_fetch_add(&sleeps, 1);
    }
    return NULL;
}
static _Atomic int raw_running, raw_ready;
static void *raw_stack;
static int spawn_once;
static int raw_thread(void *unused) {
    (void)unused;
    atomic_store(&raw_ready, 1);
    while (atomic_load(&raw_running)) syscall(SYS_sched_yield);
    atomic_store(&raw_ready, 0);
    return 0;
}
/* Testing-only callback: raw clone is born after the agent's first listing. */
void refresh_testing_after_task_list(void) {
    if (spawn_once) return;
    spawn_once = 1;
    if (clone(raw_thread, (char *)raw_stack+1024*1024,
              CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM, NULL) < 0)
        _exit(6);
    while (!atomic_load(&raw_ready)) sched_yield();
}
int main(int argc, char **argv) {
    int raw = argc > 1 && argv[1][0] == 'r';
    int race = raw && !strcmp(argv[1], "race");
    event_fd = eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC);
    epoll_fd = epoll_create1(EPOLL_CLOEXEC);
    struct epoll_event event = {.events = EPOLLIN, .data.fd = event_fd};
    if (event_fd < 0 || epoll_fd < 0 || epoll_ctl(epoll_fd, EPOLL_CTL_ADD, event_fd, &event)) return 3;
    pthread_t threads[4];
    raw_stack = MAP_FAILED;
    if (raw) {
        raw_stack = mmap(NULL, 1024*1024, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (raw_stack == MAP_FAILED) return 3;
        atomic_store(&raw_running, 1);
        if (!race) refresh_testing_after_task_list();
    } else {
        if (pthread_create(&threads[0], NULL, create_loop, NULL) ||
            pthread_create(&threads[1], NULL, create_loop, NULL) ||
            pthread_create(&threads[2], NULL, epoll_loop, NULL) ||
            pthread_create(&threads[3], NULL, sleep_loop, NULL)) return 3;
    }
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("ready %p %p %p %p %p\n",(void *)f,(const void *)&ro_word,(void *)g,(void *)cross_f,(const void *)&cross_ro);
    char c;
    while (read(0, &c, 1) == 1) {
        if (c == 'q') break;
        if (c == 'v') printf("value %d errors %d\n", f(), atomic_load(&errors));
        if (c == 's') printf("stats %lu %lu %lu %lu %lu %lu\n",atomic_load(&created),atomic_load(&exited),
                             atomic_load(&epoll_eintr),atomic_load(&sleep_eintr),atomic_load(&events),atomic_load(&sleeps));
    }
    atomic_store(&running, 0);
    if (raw) {
        atomic_store(&raw_running, 0);
        while (atomic_load(&raw_ready)) sched_yield();
        /* The thread may still be executing its return trampoline: keep its
         * private stack mapped until exit_group, rather than racing munmap. */
    } else for (int i = 0; i < 4; ++i) pthread_join(threads[i], NULL);
    close(event_fd); close(epoll_fd);
    return atomic_load(&errors) ? 4 : 0;
}
