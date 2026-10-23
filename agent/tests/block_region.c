#define _GNU_SOURCE
#include <link.h>
#include <stdint.h>
#include <sys/mman.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
static int block(struct dl_phdr_info *image, size_t size, void *unused) {
    (void)size; (void)unused;
    if (image->dlpi_name && image->dlpi_name[0]) return 0;
    uintptr_t end = 0;
    for (int i=0;i<image->dlpi_phnum;++i) {
        const ElfW(Phdr) *p=&image->dlpi_phdr[i];
        if (p->p_type==PT_LOAD && p->p_vaddr+p->p_memsz>end) end=p->p_vaddr+p->p_memsz;
    }
    uintptr_t start=image->dlpi_addr+((end+0x1fffff)&~0x1fffffULL)+0x10000000;
    unsigned char *p=mmap((void *)start,4096,PROT_READ|PROT_WRITE,
                         MAP_PRIVATE|MAP_ANONYMOUS|MAP_FIXED_NOREPLACE,-1,0);
    if(p!=(void *)start) _exit(90);
    memcpy(p,"unowned!",8);
    return 1;
}
__attribute__((constructor)) static void init(void) {
    if(getenv("REFRESH_TEST_BLOCK_REGION")) dl_iterate_phdr(block,0);
}
