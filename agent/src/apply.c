#include "internal.h"
#include "protocol.h"
#include "park.h"
#include "hazard.h"
#include "apply.h"
#include "unwind.h"
uintptr_t pages[MAX_PAGES];
int npages;
uint64_t patch_bytes_total, patch_count;
int owned(uintptr_t p){for(int i=0;i<npages;i++)if(pages[i]==p)return 1;return 0;}
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
#ifdef REFRESH_TESTING
#include "scan123.h"
#else
uintptr_t first_root123[1024];
double scan_us123;
uint64_t scan_bytes123,scan_hits123;
int scan_retained123,scan_candidates123,scan_errors123,reclaimed123;
static int ng123;
static void prepare_scan123(void){}
static void record_generation123(void){}
static void scan123(void){}
#endif
int apply(char *error,int *threads) {
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
