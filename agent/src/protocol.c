#include "internal.h"
#include "protocol.h"

struct rec recs[MAX_RECORDS];
int nrec;

static int fullread(int fd, void *p, size_t len) {
    while (len) {
        ssize_t n = read(fd, p, len);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return -1;
        p = (char *)p + n;
        len -= (size_t)n;
    }
    return 0;
}

static uint16_t get16(const unsigned char *p) {
    return (uint16_t)p[0] | (uint16_t)p[1] << 8;
}
static uint32_t get32(const unsigned char *p) {
    return (uint32_t)p[0] | (uint32_t)p[1] << 8 |
           (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}
static uint64_t get64(const unsigned char *p) {
    return (uint64_t)get32(p) | (uint64_t)get32(p + 4) << 32;
}

void protocol_clear(void) {
    for (int i = 0; i < nrec; ++i) {
        free(recs[i].bytes);
        recs[i].bytes = NULL;
    }
    nrec = 0;
}

static enum refresh_code bad(char error[160], const char *why) {
    snprintf(error, 160, "%s", why);
    return REFRESH_BAD_PROTOCOL;
}

enum refresh_code protocol_read(int fd, uintptr_t base,
                               struct refresh_message *message, char error[160]) {
    unsigned char head[24];
    uint32_t count;
    uint64_t remaining = REFRESH_MAX_MESSAGE - 8;
    int v1 = 0;
    protocol_clear();
    *message = (struct refresh_message){.type = REFRESH_APPLY};
    if (fullread(fd, head, 8)) return bad(error, "truncated protocol header or receive timeout");
    uint32_t magic = get32(head);
    if (!memcmp(head, "RFR1", 4)) {
        v1 = 1;
        if (fullread(fd, head + 8, 16)) return bad(error, "truncated v1 header or receive timeout");
        message->version = get16(head + 4);
        uint16_t flags = get16(head + 6);
        count = get32(head + 8);
        message->txid = get32(head + 12);
        message->total_len = get64(head + 16);
        if (message->version != 1 || flags > 2)
            return bad(error, "unsupported version or message flags");
        if (message->total_len < sizeof(head) || message->total_len > REFRESH_MAX_MESSAGE)
            return bad(error, "total_len outside message budget");
        remaining = message->total_len - sizeof(head);
        if (flags) {
            if (count || remaining) return bad(error, "PING and HOOK require an empty body");
            message->type = flags == 1 ? REFRESH_PING : REFRESH_HOOK;
            return REFRESH_OK;
        }
    } else {
        count = get32(head + 4);
        if (magic == REFRESH_V0_HOOK && count == 0) {
            message->type = REFRESH_HOOK;
            return REFRESH_OK;
        }
        if (magic != REFRESH_V0_MAGIC) return bad(error, "bad protocol magic");
    }
    if (count > MAX_RECORDS || (uint64_t)count * 28 > remaining)
        return bad(error, "record count outside message budget");
    for (uint32_t i = 0; i < count; ++i) {
        unsigned char wire[28];
        if (remaining < sizeof(wire) || fullread(fd, wire, sizeof(wire)))
            return bad(error, "truncated record header or receive timeout");
        remaining -= sizeof(wire);
        struct rec *r = &recs[nrec++];
        *r = (struct rec){.address = get64(wire), .length = get64(wire + 8),
                          .prot = get32(wire + 16), .kind = get32(wire + 20)};
        uint32_t nfix = get32(wire + 24);
        if (!r->length || r->length > REFRESH_MAX_RECORD || r->kind > 3 ||
            !(r->prot & PROT_READ) || (r->prot & ~7U) ||
            (r->kind == 1 && (r->prot != 5 || r->length != 5)) ||
            (r->kind == 2 && (r->prot != 1 || r->length > 4096)) ||
            (r->kind == 3 && (r->prot != 1 || r->length != 8 || nfix)) ||
            nfix > r->length / 8)
            return bad(error, "invalid record kind, length, permissions or fixups");
        if (r->address > UINTPTR_MAX - base)
            return bad(error, "record base address overflow");
        r->address += base;
        /* Leave room for page-ceiling arithmetic, including unwind descriptors. */
        if (r->address > UINTPTR_MAX - 4095 ||
            r->length > UINTPTR_MAX - 4095 - r->address)
            return bad(error, "record address range overflow");
        uint64_t body = r->length + (uint64_t)nfix * 4;
        if (body > remaining) return bad(error, "record exceeds declared message length");
        remaining -= body;
        r->bytes = malloc((size_t)r->length);
        if (!r->bytes) {
            snprintf(error, 160, "record allocation failed");
            return REFRESH_INTERNAL;
        }
        if (fullread(fd, r->bytes, (size_t)r->length))
            return bad(error, "truncated record payload or receive timeout");
        uint32_t prior = 0;
        for (uint32_t j = 0; j < nfix; ++j) {
            unsigned char offset[4];
            if (fullread(fd, offset, sizeof(offset))) return bad(error, "truncated fixups or receive timeout");
            uint32_t off = get32(offset);
            if ((uint64_t)off + 8 > r->length || (j && (uint64_t)off < (uint64_t)prior + 8))
                return bad(error, "fixups must be sorted, disjoint and in bounds");
            uint64_t value = get64(r->bytes + off);
            if (value > UINT64_MAX - base) return bad(error, "fixup address overflow");
            value += base;
            memcpy(r->bytes + off, &value, sizeof(value));
            prior = off;
        }
    }
    if (v1 && remaining) return bad(error, "record lengths do not match total_len");
    return REFRESH_OK;
}

const char *refresh_code_name(enum refresh_code code) {
    static const char *const names[] = {
        "OK", "BAD_PROTOCOL", "NOT_ORIGINAL_RO", "OVERLAP", "PAGE_LIMIT",
        "UNWIND_LIMIT", "MMAP_FAILED", "UNWIND_UNAVAILABLE", "UNWIND_INVALID",
        "PARK_TIMEOUT", "STACK_COLLISION", "MPROTECT_FAILED", "ROLLED_BACK", "INTERNAL"
    };
    if ((unsigned)code >= sizeof(names) / sizeof(names[0])) return "INTERNAL";
    return names[code];
}

size_t json_string(char *dst, size_t capacity, const char *source) {
    static const char hex[] = "0123456789abcdef";
    size_t n = 0;
    if (capacity < 3) return SIZE_MAX;
    dst[n++] = '"';
    for (const unsigned char *s = (const unsigned char *)source; *s; ++s) {
        unsigned char c = *s;
        size_t need = c < 32 || c >= 127 ? 6 : (c == '"' || c == '\\') ? 2 : 1;
        if (n + need + 2 > capacity) return SIZE_MAX;
        if (need == 6) {
            dst[n++] = '\\'; dst[n++] = 'u'; dst[n++] = '0'; dst[n++] = '0';
            dst[n++] = hex[c >> 4]; dst[n++] = hex[c & 15];
        } else {
            if (need == 2) dst[n++] = '\\';
            dst[n++] = (char)c;
        }
    }
    dst[n++] = '"'; dst[n] = 0;
    return n;
}

double stamp(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e6 + t.tv_nsec / 1e3;
}
