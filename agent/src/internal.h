#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/syscall.h>
#include <linux/futex.h>
#include <ucontext.h>
#include <dirent.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <signal.h>
#include <unistd.h>
#include <errno.h>
#include <time.h>
#include <fcntl.h>
#include <dlfcn.h>
#include <libunwind.h>
#include <link.h>
#include <sys/prctl.h>

#ifndef REFRESH_INTERNAL_H
#define REFRESH_INTERNAL_H
#ifndef FRESH_RO
#define FRESH_RO 0
#endif
#define MAGIC 0x76313232
#define MAX_RECORDS 8192
#define MAX_THREADS 1024
#define MAX_PAGES 65536
struct rec { uint64_t address,length; uint32_t prot,kind; unsigned char *bytes; };
struct range {uintptr_t a,b;};
typedef void (*refresh_callback)(void);
extern uintptr_t load_base;
extern struct rec recs[MAX_RECORDS];
extern int nrec;
extern pthread_rwlock_t tick_gate;
extern int use_safepoint;
extern _Atomic uint64_t tick_enter_count,tick_leave_count;
extern refresh_callback after_patch;
extern int callback_enabled;
extern void (*register_frame)(void *),(*deregister_frame)(void *);
extern uintptr_t active_frames[MAX_RECORDS],staged_frames[MAX_RECORDS];
extern size_t active_sizes[MAX_RECORDS],staged_sizes[MAX_RECORDS];
extern int nframes,nstaged,disable_unwind;
extern int frame_generation123[MAX_RECORDS];
extern uint64_t frame_registrations,frame_deregistrations;
extern _Atomic int gate,parked,collision,slots;
extern uintptr_t pages[MAX_PAGES];
extern int npages;
extern uint64_t patch_bytes_total,patch_count;
extern ucontext_t contexts[MAX_THREADS];
extern int mode,memfd;
extern uint64_t attempts,retries,pc_hits,fp_hits,scan_hits,fp_frames,scan_words,fp_incomplete,cfi_frames,cfi_fail;
extern double unwind_us,park_us;
extern uintptr_t badsp,badhigh;
extern int read_errors,read_errno;
extern struct range hazards[MAX_RECORDS],maps[16384];
extern int nhaz,nmaps;
extern uintptr_t context_low[MAX_THREADS],context_high[MAX_THREADS];
extern uintptr_t first_root123[1024];
extern double scan_us123;
extern uint64_t scan_bytes123,scan_hits123;
extern int scan_retained123,scan_candidates123,scan_errors123,reclaimed123;
int base_cb(struct dl_phdr_info *,size_t,void *);
int register_stack(void);
void handler(int,siginfo_t *,void *);
int owned(uintptr_t);
double stamp(void);
#endif
