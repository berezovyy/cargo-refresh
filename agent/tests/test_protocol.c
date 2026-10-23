#define _GNU_SOURCE
#include "protocol.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

extern struct rec recs[REFRESH_MAX_RECORDS];
extern int nrec;
static unsigned cases;
static void put16(unsigned char *p, uint16_t x) { memcpy(p, &x, 2); }
static void put32(unsigned char *p, uint32_t x) { memcpy(p, &x, 4); }
static void put64(unsigned char *p, uint64_t x) { memcpy(p, &x, 8); }
static size_t header(unsigned char *b, unsigned version, unsigned flags, unsigned count, uint64_t total) {
    memcpy(b, "RFR1", 4); put16(b + 4, version); put16(b + 6, flags);
    put32(b + 8, count); put32(b + 12, 123); put64(b + 16, total); return 24;
}
static void rec_header(unsigned char *b, uint64_t address, uint64_t length,
                       unsigned prot, unsigned kind, unsigned fixups) {
    put64(b, address); put64(b + 8, length); put32(b + 16, prot);
    put32(b + 20, kind); put32(b + 24, fixups);
}
static struct refresh_message parse(const unsigned char *b, size_t len, uintptr_t base,
                                     enum refresh_code expected) {
    int s[2]; assert(!socketpair(AF_UNIX, SOCK_STREAM, 0, s));
    assert(write(s[0], b, len) == (ssize_t)len);
    assert(!shutdown(s[0], SHUT_WR));
    struct refresh_message msg;
    char error[160] = "";
    enum refresh_code actual = protocol_read(s[1], base, &msg, error);
    if (actual != expected) fprintf(stderr, "case %u: %s instead of %s (%s)\n", cases,
                                    refresh_code_name(actual), refresh_code_name(expected), error);
    assert(actual == expected);
    if (actual != REFRESH_OK) assert(error[0]);
    close(s[0]); close(s[1]); ++cases;
    return msg;
}
int main(void) {
    unsigned char b[512] = {0};
    struct refresh_message msg;
    put32(b, REFRESH_V0_MAGIC); put32(b + 4, 0);
    msg = parse(b, 8, 0, REFRESH_OK); assert(msg.type == REFRESH_APPLY && msg.version == 0);
    put32(b, REFRESH_V0_HOOK);
    msg = parse(b, 8, 0, REFRESH_OK); assert(msg.type == REFRESH_HOOK);
    header(b, 1, 1, 0, 24);
    msg = parse(b, 24, 0, REFRESH_OK); assert(msg.type == REFRESH_PING && msg.txid == 123);
    header(b, 1, 2, 0, 24);
    msg = parse(b, 24, 0, REFRESH_OK); assert(msg.type == REFRESH_HOOK);
    header(b, 1, 0, 0, 24); parse(b, 24, 0, REFRESH_OK);
    header(b, 2, 0, 0, 24); parse(b, 24, 0, REFRESH_BAD_PROTOCOL);
    header(b, 1, 3, 0, 24); parse(b, 24, 0, REFRESH_BAD_PROTOCOL);
    header(b, 1, 1, 1, 24); parse(b, 24, 0, REFRESH_BAD_PROTOCOL);
    header(b, 1, 2, 0, 25); parse(b, 24, 0, REFRESH_BAD_PROTOCOL);
    header(b, 1, 0, 0, 23); parse(b, 24, 0, REFRESH_BAD_PROTOCOL);
    header(b, 1, 0, 0, REFRESH_MAX_MESSAGE + 1); parse(b, 24, 0, REFRESH_BAD_PROTOCOL);
    header(b, 1, 0, 8193, 24); parse(b, 24, 0, REFRESH_BAD_PROTOCOL);
    header(b, 1, 0, 1, 24); parse(b, 24, 0, REFRESH_BAD_PROTOCOL);
    parse(b, 23, 0, REFRESH_BAD_PROTOCOL);
    parse(b, 7, 0, REFRESH_BAD_PROTOCOL);
    parse(b, 0, 0, REFRESH_BAD_PROTOCOL);
    memcpy(b, "BAD!", 4); parse(b, 8, 0, REFRESH_BAD_PROTOCOL);
    header(b, 1, 0, 1, 24 + 28 + 16 + 8);
    rec_header(b + 24, 0x2000, 16, 3, 0, 2);
    put64(b + 52, 0x3456); put64(b + 60, 0x4567); put32(b + 68, 0); put32(b + 72, 8);
    parse(b, 76, 0x10000, REFRESH_OK);
    uint64_t a, c; memcpy(&a, recs[0].bytes, 8); memcpy(&c, recs[0].bytes + 8, 8);
    assert(nrec == 1 && recs[0].address == 0x12000 && a == 0x13456 && c == 0x14567);
    parse(b, 75, 0x10000, REFRESH_BAD_PROTOCOL);
    put32(b + 72, 4); parse(b, 76, 0, REFRESH_BAD_PROTOCOL);
    put32(b + 68, 8); put32(b + 72, 0); parse(b, 76, 0, REFRESH_BAD_PROTOCOL);
    put32(b + 68, UINT32_MAX); parse(b, 76, 0, REFRESH_BAD_PROTOCOL);
    put32(b + 68, 0); put32(b + 72, 8); put64(b + 52, UINT64_MAX);
    parse(b, 76, 1, REFRESH_BAD_PROTOCOL);
    put64(b + 52, 0); put64(b + 24, UINT64_MAX); parse(b, 76, 1, REFRESH_BAD_PROTOCOL);
    put64(b + 24, UINT64_MAX - 16); parse(b, 76, 0, REFRESH_BAD_PROTOCOL);
    header(b, 1, 0, 1, 24 + 28 + 8);
    rec_header(b + 24, 0x2000, 8, 1, 3, 0); put64(b + 52, 16);
    parse(b, 60, 0, REFRESH_OK);
    rec_header(b + 24, 0x2000, 8, 1, 3, 1); parse(b, 60, 0, REFRESH_BAD_PROTOCOL);
    rec_header(b + 24, 0x2000, 8, 5, 3, 0); parse(b, 60, 0, REFRESH_BAD_PROTOCOL);
    rec_header(b + 24, 0x2000, 8, 1, 4, 0); parse(b, 60, 0, REFRESH_BAD_PROTOCOL);
    rec_header(b + 24, 0x2000, 0, 1, 0, 0); parse(b, 60, 0, REFRESH_BAD_PROTOCOL);
    rec_header(b + 24, 0x2000, REFRESH_MAX_RECORD + 1, 1, 0, 0); parse(b, 60, 0, REFRESH_BAD_PROTOCOL);
    rec_header(b + 24, 0x2000, 8, 8, 0, 0); parse(b, 60, 0, REFRESH_BAD_PROTOCOL);
    rec_header(b + 24, 0x2000, 8, 1, 1, 0); parse(b, 60, 0, REFRESH_BAD_PROTOCOL);
    rec_header(b + 24, 0x2000, 4097, 1, 2, 0); parse(b, 60, 0, REFRESH_BAD_PROTOCOL);
    rec_header(b + 24, 0x2000, 8, 1, 0, 2); parse(b, 60, 0, REFRESH_BAD_PROTOCOL);
    rec_header(b + 24, 0x2000, 8, 1, 0, 0); parse(b, 59, 0, REFRESH_BAD_PROTOCOL);
    header(b, 1, 0, 1, 61); parse(b, 60, 0, REFRESH_BAD_PROTOCOL);
    header(b, 1, 0, 1, 59); parse(b, 60, 0, REFRESH_BAD_PROTOCOL);
    put32(b, REFRESH_V0_MAGIC); put32(b + 4, 1);
    rec_header(b + 8, 0x2000, 8, 1, 0, 0); put64(b + 36, 123);
    parse(b, 44, 0, REFRESH_OK); protocol_clear(); assert(nrec == 0);
    char out[512];
    assert(json_string(out, sizeof(out), "a\"b\\c\n\r\t\001\177\377") != SIZE_MAX);
    assert(!strcmp(out, "\"a\\\"b\\\\c\\u000a\\u000d\\u0009\\u0001\\u007f\\u00ff\""));
    assert(json_string(out, 3, "") == 2 && !strcmp(out, "\"\""));
    assert(json_string(out, 2, "") == SIZE_MAX);
    assert(json_string(out, 4, "a") == 3 && !strcmp(out, "\"a\""));
    assert(json_string(out, 3, "a") == SIZE_MAX);
    assert(json_string(out, 8, "\001") == SIZE_MAX);
    assert(json_string(out, 9, "\001") == 8);
    assert(!strcmp(refresh_code_name(REFRESH_ROLLED_BACK), "ROLLED_BACK"));
    assert(!strcmp(refresh_code_name((enum refresh_code)999), "INTERNAL"));
    printf("protocol: %u parsing cases + JSON escaping/bounds/codes PASS\n", cases);
    return 0;
}
