#define _GNU_SOURCE
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
#ifdef E123_TRACK_REGISTRY
#include "registry_alloc123.h"
#endif
static uintptr_t load_base;
static int base_cb(struct dl_phdr_info *i,size_t n,void *p){(void)n;(void)p;if(!i->dlpi_name || !i->dlpi_name[0]){load_base=i->dlpi_addr;return 1;}return 0;}



// Research protocol, host-endian x86-64 only. Trusted local controller.
// u32 magic, u32 count, then {u64 address,u64 length,u32 prot,u32 kind,bytes}.
// kind 0: appended segment, kind 1: entry write/restore; kind 2: original immutable data.
// Alternative B requires the producer to place RO data on fresh non-X pages.
// The fixed-address wire protocol carries no relocation metadata: reject overlap.
#ifndef FRESH_RO
#define FRESH_RO 0
#endif
#define MAGIC 0x76313232
#define MAX_RECORDS 8192
#define MAX_THREADS 1024
#define MAX_PAGES 65536
struct rec { uint64_t address,length; uint32_t prot,kind; unsigned char *bytes; };
static struct rec recs[MAX_RECORDS];
static int nrec;
static pthread_rwlock_t tick_gate=PTHREAD_RWLOCK_INITIALIZER;
static int use_safepoint;
static _Atomic uint64_t tick_enter_count,tick_leave_count;
void e054_tick_enter(void){pthread_rwlock_rdlock(&tick_gate);atomic_fetch_add(&tick_enter_count,1);}
void e054_tick_leave(void){atomic_fetch_add(&tick_leave_count,1);pthread_rwlock_unlock(&tick_gate);}
typedef void (*refresh_callback)(void);
static refresh_callback after_patch;
static int callback_enabled;
// EXP-121: descriptors use kind=3, address=.eh_frame, bytes=u64 span.
// The source span excludes a terminator; a private terminated copy is staged.
static void (*register_frame)(void *),(*deregister_frame)(void *);
#ifdef E123_OWN_REGISTRY
static void (*register_info123)(void*,void*);
static unsigned char registry_info123[MAX_RECORDS][512] __attribute__((aligned(16)));
static int registry_cursor123;
static void register_owned123(void *f){
    if(registry_cursor123==MAX_RECORDS)abort();
    register_info123(f,registry_info123[registry_cursor123++]);
}
#endif
static uintptr_t active_frames[MAX_RECORDS],staged_frames[MAX_RECORDS];
static size_t active_sizes[MAX_RECORDS],staged_sizes[MAX_RECORDS];
static int nframes,nstaged,disable_unwind;
static int frame_generation123[MAX_RECORDS];
static uint64_t frame_registrations,frame_deregistrations;
static size_t unwind_bytes122(void){size_t total=0;for(int i=0;i<nframes;i++)total+=active_sizes[i];return total;}
static int owned(uintptr_t p);
#include "unwind-copy.h"
static void discard_staged(void){for(int i=0;i<nstaged;i++)munmap((void*)staged_frames[i],staged_sizes[i]);nstaged=0;}
static int prepare_frames(char *error) {
    if(!register_frame || !deregister_frame){strcpy(error,"libgcc frame registration unavailable");return -1;}
    discard_staged();
    for(int i=0;i<nrec;i++)if(recs[i].kind==3){
        uint64_t span;memcpy(&span,recs[i].bytes,8);uintptr_t a=recs[i].address;
        if(span<8 || span>1024*1024 || a>UINTPTR_MAX-span){strcpy(error,"invalid unwind span");return -1;}
        for(uintptr_t p=a&~4095ULL;p<((a+span+4095)&~4095ULL);p+=4096)
            if(!owned(p)){strcpy(error,"unwind region outside patch pages");return -1;}
        size_t size=(span+4+4095)&~4095ULL;void *buf=MAP_FAILED;
        for(int k=0;k<512;k++){
            uintptr_t hint=load_base+0x10000000ULL+(uintptr_t)k*0x200000;
            buf=mmap((void*)hint,size,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED_NOREPLACE,-1,0);
            if(buf!=MAP_FAILED)break;
        }
        if(buf==MAP_FAILED){strcpy(error,"near unwind mapping unavailable");return -1;}
        staged_frames[nstaged]=(uintptr_t)buf;staged_sizes[nstaged++]=size;
        memcpy(buf,(void*)a,span);
        for(int j=0;j<nrec;j++)if(recs[j].kind==0){
            uintptr_t lo=recs[j].address,hi=lo+recs[j].length;
            uintptr_t start=lo>a?lo:a,end=hi<a+span?hi:a+span;
            if(start<end)memcpy((char*)buf+start-a,recs[j].bytes+start-lo,end-start);
        }
        if(rebase121(buf,span,(intptr_t)buf-(intptr_t)a)){strcpy(error,"unsupported unwind-copy encoding");return -1;}
        if(mprotect(buf,size,PROT_READ)){strcpy(error,"unwind-copy mprotect failed");return -1;}
    }
    return 0;
}
static _Atomic int gate,parked,collision;
static uintptr_t pages[MAX_PAGES];
static int npages;
static uint64_t patch_bytes_total, patch_count;
// All records/contexts are private to one stopped epoch; the handler only copies.
static ucontext_t contexts[MAX_THREADS];
static _Atomic int slots;
static int mode=4,memfd=-1;
static uint64_t attempts,retries,pc_hits,fp_hits,scan_hits,fp_frames,scan_words,fp_incomplete,cfi_frames,cfi_fail;
static double unwind_us,park_us; static uintptr_t badsp,badhigh; static int read_errors,read_errno;
struct range {uintptr_t a,b;};
static struct range hazards[MAX_RECORDS],maps[16384];
static int nhaz,nmaps;
static double stamp(void);
static int cmp_range(const void *a,const void *b){uintptr_t x=((const struct range*)a)->a,y=((const struct range*)b)->a;return (x>y)-(x<y);}
static int hazard(uintptr_t p){int lo=0,hi=nhaz;while(lo<hi){int m=(lo+hi)/2;if(hazards[m].a<=p)lo=m+1;else hi=m;}return lo && p<hazards[lo-1].b;}
static int executable(uintptr_t p){int lo=0,hi=nmaps;while(lo<hi){int m=(lo+hi)/2;if(maps[m].a<=p)lo=m+1;else hi=m;}return lo && p<maps[lo-1].b;}
static int prepare_stacks(void){
    nhaz=0;
    for(int i=0;i<nrec;i++)if(recs[i].kind!=2 && recs[i].kind!=3){
        uintptr_t a=recs[i].address,b=a+recs[i].length;
        // Any permission restore without X affects the full mprotect page span.
        // Include every such record, even if a later record restores X again.
        if(!FRESH_RO && !(recs[i].prot&PROT_EXEC)){a&=~4095ULL;b=(b+4095)&~4095ULL;}
        hazards[nhaz++]=(struct range){a,b};
    }
    qsort(hazards,nhaz,sizeof(*hazards),cmp_range);
    int count=0;for(int i=0;i<nhaz;i++){if(count && hazards[i].a<=hazards[count-1].b){if(hazards[i].b>hazards[count-1].b)hazards[count-1].b=hazards[i].b;}else hazards[count++]=hazards[i];}nhaz=count;
    nmaps=0;FILE *f=fopen("/proc/self/maps","r");if(!f)return -1;
    char line[1024],perms[8];unsigned long a,b;
    while(fgets(line,sizeof(line),f))if(sscanf(line,"%lx-%lx %7s",&a,&b,perms)==3 && perms[2]=='x'){
        if(nmaps==16384){fclose(f);return -1;}maps[nmaps++]=(struct range){a,b};
    }fclose(f);return 0;
}

struct thread_stack { _Atomic int tid; uintptr_t low,high; };
static struct thread_stack stacks[MAX_THREADS];
static uintptr_t context_low[MAX_THREADS],context_high[MAX_THREADS];
static int register_stack(void){
    pthread_attr_t attr;void *base;size_t size;
    if(pthread_getattr_np(pthread_self(),&attr))return -1;
    int rc=pthread_attr_getstack(&attr,&base,&size);pthread_attr_destroy(&attr);if(rc)return -1;
    for(int i=0;i<MAX_THREADS;i++){
        int zero=0;if(atomic_compare_exchange_strong(&stacks[i].tid,&zero,-1)){
            stacks[i].low=(uintptr_t)base;stacks[i].high=(uintptr_t)base+size;
            atomic_store_explicit(&stacks[i].tid,syscall(SYS_gettid),memory_order_release);return i;
        }
    }return -1;
}
struct start_info {void *(*fn)(void*);void *arg;};
static int (*real_create)(pthread_t*,const pthread_attr_t*,void*(*)(void*),void*);
static void *start_thread(void *v){struct start_info copy=*(struct start_info*)v;free(v);int slot=register_stack();void *result=copy.fn(copy.arg);if(slot>=0)atomic_store(&stacks[slot].tid,0);return result;}
int pthread_create(pthread_t *t,const pthread_attr_t *attr,void *(*fn)(void*),void *arg){
    if(!real_create)real_create=dlsym(RTLD_NEXT,"pthread_create");
    struct start_info *v=malloc(sizeof(*v));if(!v)return ENOMEM;*v=(struct start_info){fn,arg};int rc=real_create(t,attr,start_thread,v);if(rc)free(v);return rc;
}
static void handler(int sig,siginfo_t *info,void *ctx) {
    (void)sig;(void)info;
    if(!atomic_load_explicit(&gate,memory_order_acquire)) return;
    int slot=atomic_fetch_add(&slots,1);
    if(slot<MAX_THREADS){
        contexts[slot]=*(ucontext_t*)ctx;context_low[slot]=context_high[slot]=0;
        int tid=syscall(SYS_gettid);for(int j=0;j<MAX_THREADS;j++)if(atomic_load_explicit(&stacks[j].tid,memory_order_acquire)==tid){context_low[slot]=stacks[j].low;context_high[slot]=stacks[j].high;break;}
    }
    else atomic_store(&collision,1);
    atomic_fetch_add_explicit(&parked,1,memory_order_release);
    while(atomic_load_explicit(&gate,memory_order_acquire))
        syscall(SYS_futex,&gate,FUTEX_WAIT_PRIVATE,1,NULL,NULL,0);
    atomic_fetch_sub_explicit(&parked,1,memory_order_release);
}
static int check_stacks(void){
    double begin=stamp();int hit=0;
    for(int i=0;i<atomic_load(&slots) && i<MAX_THREADS;i++){
        ucontext_t *c=&contexts[i];uintptr_t pc=c->uc_mcontext.gregs[REG_RIP],sp=c->uc_mcontext.gregs[REG_RSP],bp=c->uc_mcontext.gregs[REG_RBP];
        if(hazard(pc)){pc_hits++;hit=1;}
        if(mode==0)continue;
        uintptr_t low=context_low[i],high=context_high[i];
        if(!high || high-sp>64*1024*1024){badsp=sp;badhigh=high;fp_incomplete++;hit=1;continue;}
        if(mode==1 || mode==4){
            unsigned depth=0;uintptr_t pair[2];
            while(bp>=sp && bp>=low && bp+16<=high && !(bp&7) && depth++<1024){
                if(pread(memfd,pair,16,bp)!=16){read_errors++;read_errno=errno;badsp=bp;hit=1;break;}
                fp_frames++;if(hazard(pair[1])){fp_hits++;hit=1;}
                if(pair[0]<=bp){if(pair[0])fp_incomplete++;break;}bp=pair[0];
            }
            if(depth==0 || depth>=1024 || (bp && bp+16>high))fp_incomplete++;
        }
        if(mode==2){
            unw_cursor_t cursor;int rc=unw_init_local2(&cursor,(unw_context_t*)c,UNW_INIT_SIGNAL_FRAME),depth=0;
            if(rc<0){cfi_fail++;hit=1;continue;}
            while((rc=unw_step(&cursor))>0 && depth++<1024){unw_word_t ip=0;unw_get_reg(&cursor,UNW_REG_IP,&ip);cfi_frames++;if(hazard(ip)){fp_hits++;hit=1;}}
            if(rc<0 || depth>=1024){cfi_fail++;hit=1;}
        }
        if(mode==3 || mode==4){
            // A conservative superset: includes return addresses and false-positive data.
            uintptr_t words[512];uintptr_t pos=sp;
            while(pos<high){size_t bytes=high-pos;if(bytes>sizeof(words))bytes=sizeof(words);
                ssize_t got=pread(memfd,words,bytes,pos);if(got!=(ssize_t)bytes){read_errors++;read_errno=errno;badsp=pos;badhigh=high;hit=1;break;}
                for(size_t k=0;k<bytes/8;k++){scan_words++;if(hazard(words[k]) && executable(words[k])){scan_hits++;hit=1;}}
                pos+=bytes;
            }
        }
    }
    unwind_us+=stamp()-begin;return hit;
}
static void release(void) {
    atomic_store_explicit(&gate,0,memory_order_release);
    syscall(SYS_futex,&gate,FUTEX_WAKE_PRIVATE,MAX_THREADS,NULL,NULL,0);
    while(atomic_load(&parked)) sched_yield();
}
static int park(void) {
    int tids[MAX_THREADS],n=0,self=syscall(SYS_gettid);
    DIR *d=opendir("/proc/self/task");if(!d)return -1;
    struct dirent *e;
    while((e=readdir(d))) {int tid=atoi(e->d_name);if(tid && tid!=self){if(n==MAX_THREADS){closedir(d);return -1;}tids[n++]=tid;}}
    closedir(d);
    atomic_store(&collision,0);atomic_store(&slots,0);atomic_store(&gate,1);
    int sent=0;
    for(int i=0;i<n;i++)if(!syscall(SYS_tgkill,getpid(),tids[i],SIGUSR2))sent++;
    struct timespec start,now;clock_gettime(CLOCK_MONOTONIC,&start);
    while(atomic_load(&parked)!=sent) {
        clock_gettime(CLOCK_MONOTONIC,&now);
        if((now.tv_sec-start.tv_sec)*1e9+now.tv_nsec-start.tv_nsec>100000000){release();return -1;}
        sched_yield();
    }
    if(atomic_load(&collision) || check_stacks()){release();return -2;}
    return n;
}
static int fullread(int fd,void *p,size_t len) {
    while(len){ssize_t n=read(fd,p,len);if(n<=0)return -1;p=(char*)p+n;len-=n;}return 0;
}
static double stamp(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec*1e6+t.tv_nsec/1e3;}
static int owned(uintptr_t p){for(int i=0;i<npages;i++)if(pages[i]==p)return 1;return 0;}
static int original_readonly(void) {
    struct stat executable;
    if(stat("/proc/self/exe",&executable)) return 0;
    unsigned char valid[MAX_RECORDS]={0};
    FILE *f=fopen("/proc/self/maps","r");if(!f)return 0;
    char line[1024],permissions[8];unsigned long start,end,offset,inode;unsigned devmajor,devminor;
    while(fgets(line,sizeof(line),f)) {
        if(sscanf(line,"%lx-%lx %7s %lx %x:%x %lu",&start,&end,permissions,&offset,&devmajor,&devminor,&inode)!=7)continue;
        if(inode!=(unsigned long)executable.st_ino || strcmp(permissions,"r--p"))continue;
        for(int i=0;i<nrec;i++)if(recs[i].kind==2 && recs[i].address>=start && recs[i].address<end && recs[i].length<=end-recs[i].address)valid[i]=1;
    }
    fclose(f);
    for(int i=0;i<nrec;i++)if(recs[i].kind==2 && !valid[i])return 0;
    return 1;
}
#include "scan123.h"
static int apply(char *error,int *threads) {
    attempts=retries=pc_hits=fp_hits=scan_hits=fp_frames=scan_words=fp_incomplete=cfi_frames=cfi_fail=0;unwind_us=park_us=0;
    if(!original_readonly()) {
        strcpy(error,"constant record is not original read-only executable memory");return -1;
    }
    if(FRESH_RO){
        // Current X is also ever-X: this policy never removes X, including on
        // refused transactions. Snapshot before allocating or touching any bytes.
        FILE *f=fopen("/proc/self/maps","r");if(!f){strcpy(error,"fresh RO map read failed");return -1;}
        char line[1024],perms[8];unsigned long lo,hi;int overlap=0;
        while(fgets(line,sizeof(line),f))if(sscanf(line,"%lx-%lx %7s",&lo,&hi,perms)==3 && perms[2]=='x')
            for(int i=0;i<nrec;i++)if(recs[i].kind!=3 && !(recs[i].prot&PROT_EXEC)){
                uintptr_t a=recs[i].address&~4095ULL,b=(recs[i].address+recs[i].length+4095)&~4095ULL;
                if(a<hi && b>lo)overlap=1;
            }
        fclose(f);
        // Also reject mixed-X final records inside one transaction, before mmap.
        for(int i=0;i<nrec;i++)if(!(recs[i].prot&PROT_EXEC))
            for(int j=0;j<nrec;j++)if(recs[j].kind!=3 && recs[j].prot&PROT_EXEC){
                uintptr_t a=recs[i].address&~4095ULL,b=(recs[i].address+recs[i].length+4095)&~4095ULL;
                uintptr_t c=recs[j].address&~4095ULL,d=(recs[j].address+recs[j].length+4095)&~4095ULL;
                if(a<d && b>c)overlap=1;
            }
        if(overlap){strcpy(error,"fresh RO required: non-X record overlaps executable page");return -1;}
    }
    // EXP-122: refuse every rewrite of an owned generation before staging.
    // All pages are retained, including data and code, because addresses may escape.
    for(int i=0;i<nrec;i++)if(recs[i].kind==0){
        uintptr_t a=recs[i].address&~4095ULL,end=(recs[i].address+recs[i].length+4095)&~4095ULL;
        for(uintptr_t p=a;p<end;p+=4096)if(owned(p)){
            strcpy(error,"append-only generation overlaps retained page");return -1;
        }
    }
    int frame_additions=0;
    for(int i=0;i<nrec;i++)if(recs[i].kind==3)frame_additions++;
    if(nframes+frame_additions>MAX_RECORDS){strcpy(error,"retained unwind limit");return -1;}
    // Allocate all pages before parking. MAP_FIXED_NOREPLACE never clobbers app memory.
    for(int i=0;i<nrec;i++)if(!recs[i].kind){
        uintptr_t a=recs[i].address&~4095ULL,end=(recs[i].address+recs[i].length+4095)&~4095ULL;
        for(uintptr_t p=a;p<end;p+=4096)if(!owned(p)){
            if(npages==MAX_PAGES){strcpy(error,"page limit");return -1;}
            void *v=mmap((void*)p,4096,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED_NOREPLACE,-1,0);
            if(v==MAP_FAILED){snprintf(error,160,"mmap %lx: %s",p,strerror(errno));return -1;}
            pages[npages++]=p;
        }
    }
    if(prepare_frames(error))return -1;
    attempts=retries=pc_hits=fp_hits=scan_hits=fp_frames=scan_words=fp_incomplete=cfi_frames=cfi_fail=0;unwind_us=park_us=0;
    prepare_scan123();
    if(prepare_stacks()){strcpy(error,"stack map preparation failed");return -1;}
    int n=-1;
    for(int attempt=0;attempt<100;attempt++){
        attempts++;double pstart=stamp();n=park();park_us+=stamp()-pstart;if(n>=0)break;
        retries++;
        if(n==-1){strcpy(error,"park timeout or thread limit");return -1;}
        usleep(1000);
    }
    if(n<0){snprintf(error,160,"stack collision retry limit reads=%d errno=%d pos=%lx high=%lx",read_errors,read_errno,badsp,badhigh);return -1;}
    *threads=n;
    // libgcc entry points resolved before parking. Registration is inside this
    // safe epoch. Unlike ordinary memcpy, libgcc may allocate/take internal
    // locks; this prototype does not prove safety against a parked lock owner.
    // No explicit loader activity or stdio while app threads are parked.
    for(int i=0;i<nrec;i++){
        struct rec *r=&recs[i];if(r->kind==3)continue;uintptr_t a=r->address&~4095ULL;
        size_t size=((r->address+r->length+4095)&~4095ULL)-a;
        if(mprotect((void*)a,size,((r->kind==2 || (FRESH_RO && !(r->prot&PROT_EXEC))) ? PROT_READ|PROT_WRITE : PROT_READ|PROT_WRITE|PROT_EXEC))){release();strcpy(error,"mprotect write failed");return -1;}
        memcpy((void*)r->address,r->bytes,r->length);
        __builtin___clear_cache((char*)r->address,(char*)r->address+r->length);
        if(mprotect((void*)a,size,r->prot)){release();strcpy(error,"mprotect restore failed");return -1;}
    }
    if(!disable_unwind)for(int i=0;i<nstaged;i++){
#ifdef E123_TRACK_REGISTRY
        registry_allocating123=1;
#endif
        register_frame((void*)staged_frames[i]);frame_registrations++;
#ifdef E123_TRACK_REGISTRY
        registry_allocating123=0;
#endif
    }
    record_generation123();
    if(getenv("E123_SCAN") && (ng123%20==0 || getenv("E123_GC_TEST"))) scan123();
    atomic_thread_fence(memory_order_seq_cst);
    release();
    // EXP-122: previous code can still be called; retain its unwind copies too.
    if(!disable_unwind)for(int i=0;i<nstaged;i++){
        active_frames[nframes]=staged_frames[i];frame_generation123[nframes]=ng123-1;active_sizes[nframes++]=staged_sizes[i];
    }
    if(disable_unwind)discard_staged();else nstaged=0;
    return 0;
}
static void *server(void *unused) {
    (void)unused;
    const char *path=getenv("E077_AGENT");if(!path)return NULL;
    int fd=socket(AF_UNIX,SOCK_STREAM,0);struct sockaddr_un addr={.sun_family=AF_UNIX};
    if(strlen(path)>=sizeof(addr.sun_path))return NULL;
    strcpy(addr.sun_path,path);unlink(path);
    if(bind(fd,(void*)&addr,sizeof(addr))||listen(fd,4))return NULL;
    for(;;){
        int c=accept(fd,NULL,NULL);if(c<0)continue;
        uint32_t head[2];char err[160]="bad protocol";int ok=0,threads=0;
        double start=stamp();
        nrec=0;
        int got_head=fullread(c,head,8);
        if(!got_head && head[0]==0xE099CA11 && head[1]==0){
            if(after_patch)after_patch();
            const char *r=after_patch?"{\"ok\":true,\"callback\":true}\n":"{\"ok\":false,\"callback\":false}\n";
            ssize_t sent=write(c,r,strlen(r));(void)sent;close(c);continue;
        }
        if(!got_head&&head[0]==MAGIC&&head[1]<=MAX_RECORDS){
            ok=1;nrec=head[1];
            for(int i=0;i<nrec;i++){
                unsigned char wire[28];recs[i].bytes=NULL;
                if(fullread(c,wire,28)){ok=0;break;}
                memcpy(&recs[i],wire,24);
                uint32_t nfix;memcpy(&nfix,wire+24,4);
                if(nfix>recs[i].length/8){ok=0;break;}
                if(recs[i].address>UINTPTR_MAX-load_base){ok=0;break;}
                recs[i].address+=load_base;
                if(!recs[i].length||recs[i].length>128*1024*1024||recs[i].kind>3 || (recs[i].kind==3 && (recs[i].prot!=1 || recs[i].length!=8 || nfix)) || (recs[i].kind==1 && (recs[i].prot!=5 || recs[i].length!=5)) || (recs[i].kind==2 && (recs[i].prot!=1 || recs[i].length>4096))){ok=0;break;}
                recs[i].bytes=malloc(recs[i].length);
                if(!recs[i].bytes||fullread(c,recs[i].bytes,recs[i].length)){ok=0;break;}
                uint32_t prior=0;
                for(uint32_t j=0;j<nfix;j++){
                    uint32_t off;uint64_t value;
                    if(fullread(c,&off,4)||off+8ULL>recs[i].length||(j && off<prior+8)){ok=0;break;}
                    memcpy(&value,recs[i].bytes+off,8);
                    if(value>UINT64_MAX-load_base){ok=0;break;}
                    value+=load_base;memcpy(recs[i].bytes+off,&value,8);prior=off;
                }
                if(!ok)break;
            }
            if(ok){
                if(use_safepoint)pthread_rwlock_wrlock(&tick_gate);
                ok=apply(err,&threads)==0;
                if(use_safepoint)pthread_rwlock_unlock(&tick_gate);
            }
        }
        uint64_t patch_bytes=0;
        if(ok){for(int i=0;i<nrec;i++)patch_bytes+=recs[i].length;patch_bytes_total+=patch_bytes;patch_count++;}
        int callback_invoked=ok && callback_enabled && after_patch;
        if(callback_invoked)after_patch();
        double elapsed=stamp()-start;char reply[2048];
        int len=snprintf(reply,sizeof(reply),"{\"ok\":%s,\"agent_us\":%.3f,\"threads\":%d,\"pages\":%d,\"patch_bytes_total\":%llu,\"patch_count\":%llu,\"error\":\"%s\",\"load_base\":%llu,\"mode\":%d,\"attempts\":%llu,\"retries\":%llu,\"pc_hits\":%llu,\"fp_hits\":%llu,\"scan_hits\":%llu,\"fp_frames\":%llu,\"fp_incomplete\":%llu,\"scan_words\":%llu,\"cfi_frames\":%llu,\"cfi_fail\":%llu,\"unwind_us\":%.3f,\"park_us\":%.3f,\"scan_first_root123\":%llu,\"scan_us123\":%.3f,\"scan_bytes123\":%llu,\"scan_hits123\":%llu,\"scan_retained123\":%d,\"scan_candidates123\":%d,\"scan_errors123\":%d,\"reclaimed123\":%d,\"unwind_active\":%d,\"unwind_retained_bytes\":%zu,\"unwind_registered_total\":%llu,\"unwind_deregistered_total\":%llu}\n",ok?"true":"false",elapsed,threads,npages,(unsigned long long)patch_bytes_total,(unsigned long long)patch_count,ok?"":err,(unsigned long long)load_base,mode,(unsigned long long)attempts,(unsigned long long)retries,(unsigned long long)pc_hits,(unsigned long long)fp_hits,(unsigned long long)scan_hits,(unsigned long long)fp_frames,(unsigned long long)fp_incomplete,(unsigned long long)scan_words,(unsigned long long)cfi_frames,(unsigned long long)cfi_fail,unwind_us,park_us,(unsigned long long)first_root123[0],scan_us123,(unsigned long long)scan_bytes123,(unsigned long long)scan_hits123,scan_retained123,scan_candidates123,scan_errors123,reclaimed123,nframes,unwind_bytes122(),(unsigned long long)frame_registrations,(unsigned long long)frame_deregistrations);
        if(len>2){len-=2;len+=snprintf(reply+len,sizeof(reply)-len,",\"callback_present\":%s,\"callback_invoked\":%s,\"safe_point\":%s,\"tick_enters\":%llu,\"tick_leaves\":%llu}\n",after_patch?"true":"false",callback_invoked?"true":"false",use_safepoint?"true":"false",(unsigned long long)atomic_load(&tick_enter_count),(unsigned long long)atomic_load(&tick_leave_count));}
        ssize_t written=write(c,reply,len);(void)written;close(c);
        for(int i=0;i<nrec;i++){free(recs[i].bytes);recs[i].bytes=NULL;}
        discard_staged();nrec=0;
    }
    return NULL;
}
__attribute__((constructor)) static void init(void) {
    if(!getenv("E077_AGENT"))return;
    const char *host=getenv("E106_HOST_EXE");
    if(host) {
        char exe[4096];ssize_t n=readlink("/proc/self/exe",exe,sizeof(exe)-1);
        if(n<0 || n>=(ssize_t)sizeof(exe)-1)return;
        exe[n]=0;if(strcmp(exe,host))return;
    }
    if(getenv("E121_DEBUG"))prctl(PR_SET_PTRACER,PR_SET_PTRACER_ANY,0,0,0);
    const char *m=getenv("E084_MODE");
    if(m){if(!strcmp(m,"ip"))mode=0;else if(!strcmp(m,"fp"))mode=1;else if(!strcmp(m,"cfi"))mode=2;else if(!strcmp(m,"scan"))mode=3;}
    use_safepoint=getenv("E054_SAFEPOINT") && atoi(getenv("E054_SAFEPOINT"));
    after_patch=(refresh_callback)dlsym(RTLD_DEFAULT,"fast_refresh_after_patch");
    callback_enabled=getenv("E099_AUTO_HOOK") && !strcmp(getenv("E099_AUTO_HOOK"),"1");
    dl_iterate_phdr(base_cb,NULL);
    void *gcc=dlopen("libgcc_s.so.1",RTLD_NOW|RTLD_LOCAL);
    if(gcc){register_frame=dlsym(gcc,"__register_frame");deregister_frame=dlsym(gcc,"__deregister_frame");}
#ifdef E123_OWN_REGISTRY
    register_info123=dlsym(gcc,"__register_frame_info");
    if(register_info123){register_frame=register_owned123;deregister_frame=dlsym(gcc,"__deregister_frame_info");}
#endif
    disable_unwind=getenv("E121_DISABLE_UNWIND")!=NULL;
    register_stack();
    memfd=open("/proc/self/mem",O_RDONLY|O_CLOEXEC);if(memfd<0)return;
    // Resolve unwind library lazy symbols before any app thread can be parked.
    unw_context_t warm;unw_cursor_t cur;unw_getcontext(&warm);unw_init_local(&cur,&warm);unw_step(&cur);unw_word_t ip;unw_get_reg(&cur,UNW_REG_IP,&ip);
    struct sigaction sa={.sa_sigaction=handler,.sa_flags=SA_SIGINFO|SA_RESTART};sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR2,&sa,NULL);pthread_t t;pthread_create(&t,NULL,server,NULL);pthread_detach(t);
}
