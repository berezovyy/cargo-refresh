#include "internal.h"
#include "apply.h"
#include "region.h"

/* Shared with rustd splice.rs. All values in the note are little-endian. */
#define REGION_ALIGN (2ULL << 20)
#define REGION_GAP (256ULL << 20)
#define REGION_SIZE (1ULL << 30)
static const unsigned char note_prefix[20] = {
    8,0,0,0,16,0,0,0,0x31,0x52,0x46,0x52,'R','E','F','R','E','S','H',0
};
uintptr_t region_start, region_end;
int region_ok;
static uintptr_t reserved[2 * (MAX_PAGES + 1)];
static size_t nreserved;
int region_contains(uintptr_t p) {
    return p >= region_start && p < region_end && region_end - p >= 4096;
}
uint64_t region_used(void) { return (uint64_t)npages * 4096; }
static int init_cb(struct dl_phdr_info *image, size_t size, void *unused) {
    (void)size; (void)unused;
    if (image->dlpi_name && image->dlpi_name[0]) return 0;
    uintptr_t base_end = 0;
    int notes = 0;
    for (int i = 0; i < image->dlpi_phnum; ++i) {
        const ElfW(Phdr) *ph = &image->dlpi_phdr[i];
        if (ph->p_type == PT_LOAD) {
            if (ph->p_vaddr > UINTPTR_MAX - ph->p_memsz) return 1;
            uintptr_t end = ph->p_vaddr + ph->p_memsz;
            if (end > base_end) base_end = end;
        }
    }
    /* Only inspect notes contained in a readable main-image PT_LOAD. */
    for (int i = 0; i < image->dlpi_phnum; ++i) {
        const ElfW(Phdr) *ph = &image->dlpi_phdr[i];
        if (ph->p_type != PT_NOTE || ph->p_filesz != 36) continue;
        int readable = 0;
        for (int j = 0; j < image->dlpi_phnum; ++j) {
            const ElfW(Phdr) *load = &image->dlpi_phdr[j];
            if (load->p_type == PT_LOAD && (load->p_flags & PF_R) &&
                ph->p_vaddr >= load->p_vaddr && load->p_memsz >= 36 &&
                ph->p_vaddr - load->p_vaddr <= load->p_memsz - 36) readable = 1;
        }
        if (!readable) continue;
        const unsigned char *note = (const void *)(load_base + ph->p_vaddr);
        if (memcmp(note, note_prefix, 20)) continue;
        uint64_t version;
        memcpy(&base_end, note + 20, 8); memcpy(&version, note + 28, 8);
        if (++notes != 1 || version != 1) return 1;
    }
    if (base_end > (700ULL << 20)) return 1;
    uintptr_t relative = ((base_end + REGION_ALIGN - 1) & ~(REGION_ALIGN - 1)) + REGION_GAP;
    if (load_base > UINTPTR_MAX - relative - REGION_SIZE) return 1;
    region_start = load_base + relative; region_end = region_start + REGION_SIZE;
    /* Every segment above the base must be fully in the immutable region and
       explicitly adopted before any gap is reserved. Unrecognized pages fail closed. */
    for (int i = 0; i < image->dlpi_phnum; ++i) {
        const ElfW(Phdr) *ph = &image->dlpi_phdr[i];
        if (ph->p_type != PT_LOAD || !ph->p_memsz) continue;
        if (ph->p_vaddr + ph->p_memsz <= base_end) continue;
        uintptr_t start = load_base + (ph->p_vaddr & ~4095ULL);
        uintptr_t end = load_base + ((ph->p_vaddr + ph->p_memsz + 4095) & ~4095ULL);
        if (start < region_start || end > region_end) return 1;
        if (!notes) return 1;
        unsigned prot = ((ph->p_flags & PF_R) ? PROT_READ : 0) |
                        ((ph->p_flags & PF_W) ? PROT_WRITE : 0) |
                        ((ph->p_flags & PF_X) ? PROT_EXEC : 0);
        if (adopt_region_segment(start, end, prot)) return 1;
    }
    /* Reserve contiguous gaps, skipping only validated, indexed inherited pages.
       Keep exact gap bounds for atomic startup rollback. */
    uintptr_t at = region_start;
    while (at < region_end) {
        if (owned(at)) { at += 4096; continue; }
        uintptr_t end = at + 4096;
        while (end < region_end && !owned(end)) end += 4096;
        void *p = mmap((void *)at, end - at, PROT_NONE,
                      MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
        if (p == MAP_FAILED || p != (void *)at) {
            if (p != MAP_FAILED) munmap(p, end - at);
            for (size_t j = 0; j < nreserved; j += 2)
                munmap((void *)reserved[j], reserved[j+1] - reserved[j]);
            nreserved = 0;
            return 1;
        }
        if (nreserved + 2 > sizeof(reserved)/sizeof(reserved[0])) {
            munmap(p, end - at);
            for (size_t j = 0; j < nreserved; j += 2)
                munmap((void *)reserved[j], reserved[j+1] - reserved[j]);
            nreserved = 0;
            return 1;
        }
        reserved[nreserved++] = at; reserved[nreserved++] = end;
        at = end;
    }
    region_ok = 1;
    return 1;
}
void region_init(void) { dl_iterate_phdr(init_cb, NULL); }
