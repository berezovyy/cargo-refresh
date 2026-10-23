#include "internal.h"
#include "protocol.h"
#include "park.h"
#include "hazard.h"
#include "apply.h"
#include "unwind.h"
uintptr_t load_base;
int base_cb(struct dl_phdr_info *i,size_t n,void *p){(void)n;(void)p;if(!i->dlpi_name || !i->dlpi_name[0]){load_base=i->dlpi_addr;return 1;}return 0;}



pthread_rwlock_t tick_gate=PTHREAD_RWLOCK_INITIALIZER;
int use_safepoint;
_Atomic uint64_t tick_enter_count,tick_leave_count;
void e054_tick_enter(void){pthread_rwlock_rdlock(&tick_gate);atomic_fetch_add(&tick_enter_count,1);}
void e054_tick_leave(void){atomic_fetch_add(&tick_leave_count,1);pthread_rwlock_unlock(&tick_gate);}

refresh_callback after_patch;
int callback_enabled;
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
#if defined(REFRESH_TESTING) && defined(E123_OWN_REGISTRY)
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
