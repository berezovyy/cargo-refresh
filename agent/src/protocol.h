#ifndef REFRESH_PROTOCOL_H
#define REFRESH_PROTOCOL_H
#include <stddef.h>
#include <stdint.h>

#define REFRESH_SUPPORT_PROTOCOL 1

#define REFRESH_V0_MAGIC UINT32_C(0x76313232)
#define REFRESH_V0_HOOK UINT32_C(0xe099ca11)
#define REFRESH_MAX_RECORDS 8192
#define REFRESH_MAX_MESSAGE (UINT64_C(256) * 1024 * 1024)
#define REFRESH_MAX_RECORD (UINT64_C(128) * 1024 * 1024)

struct rec {
    uint64_t address, length;
    uint32_t prot, kind;
    unsigned char *bytes;
};
enum refresh_code {
    REFRESH_OK, REFRESH_BAD_PROTOCOL, REFRESH_NOT_ORIGINAL_RO,
    REFRESH_OVERLAP, REFRESH_PAGE_LIMIT, REFRESH_UNWIND_LIMIT,
    REFRESH_MMAP_FAILED, REFRESH_UNWIND_UNAVAILABLE, REFRESH_UNWIND_INVALID,
    REFRESH_PARK_TIMEOUT, REFRESH_STACK_COLLISION, REFRESH_MPROTECT_FAILED,
    REFRESH_ROLLED_BACK, REFRESH_INTERNAL
};
enum refresh_message_type { REFRESH_APPLY, REFRESH_PING, REFRESH_HOOK };
struct refresh_message {
    enum refresh_message_type type;
    uint16_t version;
    uint32_t txid;
    uint64_t total_len;
};
const char *refresh_code_name(enum refresh_code code);
/* Uses the single server-owned recs/nrec arena. Clear after every request. */
enum refresh_code protocol_read(int fd, uintptr_t base,
                               struct refresh_message *message, char error[160]);
void protocol_clear(void);
/* Writes a complete quoted JSON string, including NUL; SIZE_MAX on overflow. */
size_t json_string(char *dst, size_t capacity, const char *source);
double stamp(void);
#endif
