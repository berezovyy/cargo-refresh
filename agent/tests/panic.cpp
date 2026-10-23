#include <atomic>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <pthread.h>
#include <unistd.h>
#include <unwind.h>
static std::atomic<bool> running{true};
static std::atomic<unsigned long> caught{0}, errors{0};
extern "C" __attribute__((noinline,noreturn)) void throw_helper() { throw 73; }
extern "C" void f();
asm(".text\n.p2align 12\n.global f\n.type f,@function\nf:\n"
    ".cfi_startproc\nsub $8,%rsp\n.cfi_def_cfa_offset 16\ncall throw_helper\nud2\n"
    ".fill 32,1,0x90\n.cfi_endproc\n.size f,.-f\n");
static void *panic_loop(void *) {
    while (running.load()) {
        try { f(); errors.fetch_add(1); }
        catch (int value) { if (value == 73) caught.fetch_add(1); else errors.fetch_add(1); }
        catch (...) { errors.fetch_add(1); }
    }
    return nullptr;
}
extern "C" void fast_refresh_after_patch() {}
int main() {
    pthread_t threads[2];
    for (auto &t: threads) if (pthread_create(&t, nullptr, panic_loop, nullptr)) return 3;
    setvbuf(stdout, nullptr, _IONBF, 0);
    /* Host protocol's second address carries the helper, needed by JIT code. */
    printf("ready %p %p %p %p %p\n",(void *)f,(void *)throw_helper,(void *)f,(void *)f,(void *)f);
    char c;
    while (read(0,&c,1)==1) {
        if(c=='q') break;
        if(c=='s') printf("stats %lu %lu\n",caught.load(),errors.load());
    }
    running.store(false);
    for (auto &t: threads) pthread_join(t,nullptr);
    return errors.load() ? 4 : 0;
}
