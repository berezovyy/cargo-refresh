// Controlled prototype only: distinguish unwinder bookkeeping from app roots.
// glibc-specific interception; no general allocator compatibility claim.
extern void *__libc_malloc(size_t);
extern void *__libc_calloc(size_t,size_t);
extern void *__libc_realloc(void*,size_t);
extern void __libc_free(void*);
static _Thread_local int registry_allocating123;
static uintptr_t registry_lo123[32768],registry_hi123[32768];
static int registry_n123;
static void note_alloc123(void *v,size_t size){
    if(v && registry_allocating123 && registry_n123<32768){registry_lo123[registry_n123]=(uintptr_t)v;registry_hi123[registry_n123++]=(uintptr_t)v+size;}
}
__attribute__((visibility("default"))) void *malloc(size_t size){void *p=__libc_malloc(size);note_alloc123(p,size);return p;}
__attribute__((visibility("default"))) void *calloc(size_t n,size_t size){void *p=__libc_calloc(n,size);note_alloc123(p,n*size);return p;}
static void forget_alloc123(void *v){
    for(int j=0;j<registry_n123;j++)if(registry_lo123[j]==(uintptr_t)v){
        explicit_bzero(v,registry_hi123[j]-registry_lo123[j]);
        registry_n123--;registry_lo123[j]=registry_lo123[registry_n123];registry_hi123[j]=registry_hi123[registry_n123];break;
    }
}
__attribute__((visibility("default"))) void free(void *v){if(v)forget_alloc123(v);__libc_free(v);}
__attribute__((visibility("default"))) void *realloc(void *v,size_t size){
    // No realloc expected in these tested registration paths. Treat as ordinary
    // root storage if it occurs, rather than excluding a possibly reused block.
    if(v)for(int j=0;j<registry_n123;j++)if(registry_lo123[j]==(uintptr_t)v){registry_n123--;registry_lo123[j]=registry_lo123[registry_n123];registry_hi123[j]=registry_hi123[registry_n123];break;}
    void *p=__libc_realloc(v,size);note_alloc123(p,size);return p;
}
