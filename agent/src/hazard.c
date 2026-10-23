#include "internal.h"
#include "protocol.h"
#include "park.h"
#include "hazard.h"
#include "apply.h"
#include "unwind.h"
int mode=4;
uint64_t attempts,retries,pc_hits,fp_hits,scan_hits,fp_frames,scan_words,fp_incomplete,cfi_frames,cfi_fail;
double unwind_us,park_us; uintptr_t badsp,badhigh; int read_errors,read_errno;

struct range hazards[MAX_RECORDS],maps[16384];
int nhaz,nmaps;
double stamp(void);
static int cmp_range(const void *a,const void *b){uintptr_t x=((const struct range*)a)->a,y=((const struct range*)b)->a;return (x>y)-(x<y);}
int hazard(uintptr_t p){int lo=0,hi=nhaz;while(lo<hi){int m=(lo+hi)/2;if(hazards[m].a<=p)lo=m+1;else hi=m;}return lo && p<hazards[lo-1].b;}
static int executable(uintptr_t p){int lo=0,hi=nmaps;while(lo<hi){int m=(lo+hi)/2;if(maps[m].a<=p)lo=m+1;else hi=m;}return lo && p<maps[lo-1].b;}
int prepare_stacks(void){
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

int check_stacks(void){
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
