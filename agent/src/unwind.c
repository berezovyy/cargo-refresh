#include "internal.h"
#include "protocol.h"
#include "park.h"
#include "hazard.h"
#include "apply.h"
#include "unwind.h"
// EXP-121: descriptors use kind=3, address=.eh_frame, bytes=u64 span.
// The source span excludes a terminator; a private terminated copy is staged.
void (*register_frame)(void *),(*deregister_frame)(void *);
#if defined(REFRESH_TESTING) && defined(E123_OWN_REGISTRY)
static void (*register_info123)(void*,void*);
static unsigned char registry_info123[MAX_RECORDS][512] __attribute__((aligned(16)));
static int registry_cursor123;
static void register_owned123(void *f){
    if(registry_cursor123==MAX_RECORDS)abort();
    register_info123(f,registry_info123[registry_cursor123++]);
}
#endif
uintptr_t active_frames[MAX_RECORDS],staged_frames[MAX_RECORDS];
size_t active_sizes[MAX_RECORDS],staged_sizes[MAX_RECORDS];
int nframes,nstaged,disable_unwind;
int frame_generation123[MAX_RECORDS];
uint64_t frame_registrations,frame_deregistrations;
size_t unwind_bytes122(void){size_t total=0;for(int i=0;i<nframes;i++)total+=active_sizes[i];return total;}
int owned(uintptr_t p);
#include "unwind-copy.h"
void discard_staged(void){for(int i=0;i<nstaged;i++)munmap((void*)staged_frames[i],staged_sizes[i]);nstaged=0;}
int prepare_frames(char *error) {
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
