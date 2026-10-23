// Diagnostic conservative root scan. NO app generation is unmapped: writable
// roots alone omit readonly/code dependencies, encoded/interior pointers and
// vector registers. Candidate counts are not a proof of unreachability.
#define GMAX123 1024
static struct range generations123[GMAX123],roots123[16384];
static int ng123,nroots123;
int scan_retained123,scan_candidates123,scan_errors123,reclaimed123;
static unsigned char marked123[GMAX123];
#ifdef E123_CONTROLLED_LEAF_GC
static unsigned char freed123[GMAX123];
#endif
uint64_t scan_bytes123,scan_hits123;
static uintptr_t origin123;
uintptr_t first_root123[GMAX123];
double scan_us123;
static uintptr_t scan_stack_lo123,scan_stack_hi123;
static int scanner_cb123(struct dl_phdr_info *i,size_t n,void *v){
    (void)n;struct range *r=v;
    uintptr_t me=(uintptr_t)&ng123;
    for(int j=0;j<i->dlpi_phnum;j++){
        const ElfW(Phdr) *p=&i->dlpi_phdr[j];
        uintptr_t lo=i->dlpi_addr+p->p_vaddr,hi=lo+p->p_memsz;
        if(p->p_type==PT_LOAD && me>=lo && me<hi){
            r->a=i->dlpi_addr;r->b=0;
            for(int k=0;k<i->dlpi_phnum;k++)if(i->dlpi_phdr[k].p_type==PT_LOAD){
                uintptr_t h=i->dlpi_addr+i->dlpi_phdr[k].p_vaddr+i->dlpi_phdr[k].p_memsz;
                if(h>r->b)r->b=h;
            }
            r->b=(r->b+4095)&~4095ULL;return 1;
        }
    }return 0;
}
static void prepare_scan123(void){
    if(!getenv("E123_SCAN"))return;
    struct range own={0,0};dl_iterate_phdr(scanner_cb123,&own);
    pthread_attr_t at;void *base;size_t size;
    if(!pthread_getattr_np(pthread_self(),&at)){
        if(!pthread_attr_getstack(&at,&base,&size)){scan_stack_lo123=(uintptr_t)base;scan_stack_hi123=(uintptr_t)base+size;}
        pthread_attr_destroy(&at);
    }
    nroots123=0;FILE *f=fopen("/proc/self/maps","r");if(!f)return;
    char line[1024],perms[8];unsigned long a,b;
    while(fgets(line,sizeof(line),f))if(sscanf(line,"%lx-%lx %7s",&a,&b,perms)==3 && perms[0]=='r' && perms[1]=='w'){
        // Omit the scanner DSO's metadata and its executing stack; retain heap
        // staging/allocation noise, intentionally conservative false roots.
        if(a<own.b && b>own.a){
            if(a<own.a && nroots123<16384)roots123[nroots123++]=(struct range){a,own.a};
            if(b>own.b && nroots123<16384)roots123[nroots123++]=(struct range){own.b,b};
        }else if(a<scan_stack_hi123 && b>scan_stack_lo123)continue;
        else if(nroots123<16384)roots123[nroots123++]=(struct range){a,b};
    }fclose(f);
}
static void record_generation123(void){
    if(ng123==GMAX123)return;
    uintptr_t lo=UINTPTR_MAX,hi=0;
    for(int i=0;i<nrec;i++)if(recs[i].kind==0){
        if(recs[i].address<lo)lo=recs[i].address;
        if(recs[i].address+recs[i].length>hi)hi=recs[i].address+recs[i].length;
    }
    if(hi)generations123[ng123++]=(struct range){lo,hi};
}
static void mark123(uintptr_t value){

    int lo=0,hi=ng123;
    while(lo<hi){int m=(lo+hi)/2;if(generations123[m].a<=value)lo=m+1;else hi=m;}
    if(lo && value<generations123[lo-1].b){
#ifdef E123_TRACK_REGISTRY
    for(int j=0;j<registry_n123;j++)if(origin123>=registry_lo123[j] && origin123+8<=registry_hi123[j])return;
#endif
marked123[lo-1]=1;if(!first_root123[lo-1])first_root123[lo-1]=origin123;scan_hits123++;}
}
static void scan123(void){
    double start=stamp();memset(marked123,0,sizeof(marked123));memset(first_root123,0,sizeof(first_root123));origin123=1;scan_errors123=0;scan_bytes123=scan_hits123=0;
    for(int j=0;j<atomic_load(&slots) && j<MAX_THREADS;j++){
        for(int k=0;k<NGREG;k++)mark123(contexts[j].uc_mcontext.gregs[k]);
    }
    // Every byte offset catches unaligned/packed ordinary machine pointers.
    unsigned char buffer[65536+8];
    for(int j=0;j<nroots123;j++)for(uintptr_t at=roots123[j].a;at<roots123[j].b;){
        size_t len=roots123[j].b-at;if(len>65536)len=65536;
        // Read overlaps to include words crossing chunk boundaries.
        size_t extra=roots123[j].b-at-len;if(extra>7)extra=7;
        ssize_t got=pread(memfd,buffer,len+extra,at);
        if(got!=(ssize_t)(len+extra)){scan_errors123++;break;}
        scan_bytes123+=len;
        for(size_t k=0;k+8<=(size_t)got;k++){uintptr_t value;memcpy(&value,buffer+k,8);origin123=at+k;mark123(value);}
        at+=len;
    }
    scan_retained123=0;for(int j=0;j<ng123;j++)scan_retained123+=!!marked123[j];
    scan_candidates123=ng123-scan_retained123;
#ifdef E123_CONTROLLED_LEAF_GC
    // Test-only: harness promises leaf code has no outgoing references, no
    // readonly external roots, no vector/encoded pointers. Not an app GC.
    if(getenv("E123_GC_TEST") && !scan_errors123){
        marked123[ng123-1]=1; // Publication root until the host obtains its pointer.
        for(int g=0;g<ng123-1;g++)if(!marked123[g] && !freed123[g]){
            for(int f=0;f<nframes;){
                if(frame_generation123[f]==g){
                    deregister_frame((void*)active_frames[f]);frame_deregistrations++;
                    munmap((void*)active_frames[f],active_sizes[f]);
                    nframes--;active_frames[f]=active_frames[nframes];active_sizes[f]=active_sizes[nframes];frame_generation123[f]=frame_generation123[nframes];
                }else f++;
            }
            for(int p=0;p<npages;){
                if(pages[p]>=generations123[g].a && pages[p]<generations123[g].b){
                    munmap((void*)pages[p],4096);pages[p]=pages[--npages];
                }else p++;
            }
            freed123[g]=1;reclaimed123++;
        }
    }
#endif
    scan_us123=stamp()-start;
}
