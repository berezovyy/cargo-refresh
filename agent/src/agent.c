#include "internal.h"
#include "protocol.h"
#include "park.h"
#include "hazard.h"
#include "apply.h"
#include "region.h"
#include "unwind.h"
#include <sys/time.h>
#include <locale.h>

/* This query must not start a server or depend on application state. */
__attribute__((visibility("default"))) uint32_t refresh_agent_support_protocol(void) {
    return REFRESH_SUPPORT_PROTOCOL;
}

uintptr_t load_base;
pthread_rwlock_t tick_gate = PTHREAD_RWLOCK_INITIALIZER;
int use_safepoint;
_Atomic uint64_t tick_enter_count, tick_leave_count;
refresh_callback after_patch;
int callback_enabled;
static const char *socket_path;
static int legacy_environment;
static locale_t numeric_locale;

__attribute__((visibility("default"))) void e054_tick_enter(void) {
    pthread_rwlock_rdlock(&tick_gate);
    atomic_fetch_add(&tick_enter_count, 1);
}
__attribute__((visibility("default"))) void e054_tick_leave(void) {
    atomic_fetch_add(&tick_leave_count, 1);
    pthread_rwlock_unlock(&tick_gate);
}
int base_cb(struct dl_phdr_info *i, size_t n, void *p) {
    (void)n; (void)p;
    if (!i->dlpi_name || !i->dlpi_name[0]) { load_base = i->dlpi_addr; return 1; }
    return 0;
}
static const char *refresh_env(const char *name, const char *legacy) {
    const char *value = getenv(name);
    if (value) return value;
    value = getenv(legacy);
    if (value) legacy_environment = 1;
    return value;
}
static void send_reply(int fd, const struct refresh_message *message,
                       enum refresh_code code, const char *error,
                       int threads, double elapsed, int callback_invoked) {
    char escaped[1024], reply[4096];
    struct refresh_scan_stats scan;
    apply_scan_stats(&scan);
    if (json_string(escaped, sizeof(escaped), code == REFRESH_OK ? "" : error) == SIZE_MAX)
        strcpy(escaped, "\"message overflow\"");
    locale_t previous_locale = uselocale(numeric_locale);
    int len = snprintf(reply, sizeof(reply),
        "{\"ok\":%s,\"code\":\"%s\",\"message\":%s,\"error\":%s,"
        "\"support_protocol\":%u,\"txid\":%u,\"generation\":%llu,\"agent_us\":%.3f,\"threads\":%d,"
        "\"pages\":%d,\"patch_bytes_total\":%llu,\"patch_count\":%llu,"
        "\"retained_bytes\":%llu,\"unwind_copies\":%d,\"near_slots_used\":%d,\"pressure\":%s,"
        "\"conflict_address\":%llu,\"conflict_start\":%llu,\"conflict_end\":%llu,"
        "\"region_ok\":%s,\"region_start\":%llu,\"region_end\":%llu,\"region_used\":%llu,"
        "\"load_base\":%llu,\"mode\":%d,\"attempts\":%llu,\"retries\":%llu,"
        "\"pc_hits\":%llu,\"fp_hits\":%llu,\"scan_hits\":%llu,\"fp_frames\":%llu,"
        "\"fp_incomplete\":%llu,\"scan_words\":%llu,\"cfi_frames\":%llu,\"cfi_fail\":%llu,"
        "\"unwind_us\":%.3f,\"park_us\":%.3f,\"unregistered_threads\":%llu,"
        "\"scan_first_root123\":%llu,\"scan_us123\":%.3f,\"scan_bytes123\":%llu,"
        "\"scan_hits123\":%llu,\"scan_retained123\":%d,\"scan_candidates123\":%d,"
        "\"scan_errors123\":%d,\"reclaimed123\":%d,\"unwind_active\":%d,"
        "\"unwind_retained_bytes\":%zu,\"unwind_registered_total\":%llu,"
        "\"unwind_deregistered_total\":%llu,\"callback_present\":%s,"
        "\"callback_invoked\":%s,\"callback\":%s,\"safe_point\":%s,"
        "\"tick_enters\":%llu,\"tick_leaves\":%llu}\n",
        code == REFRESH_OK ? "true" : "false", refresh_code_name(code), escaped, escaped,
        REFRESH_SUPPORT_PROTOCOL, message->txid, (unsigned long long)patch_count, elapsed, threads,
        npages, (unsigned long long)patch_bytes_total, (unsigned long long)patch_count,
        (unsigned long long)npages * 4096 + unwind_bytes122(), nframes, nframes,
        (npages > MAX_PAGES * 3 / 4 || nframes > MAX_RECORDS * 3 / 4 ||
         nframes > REFRESH_NEAR_SLOTS * 3 / 4) ? "true" : "false",
        (unsigned long long)apply_conflict.address, (unsigned long long)apply_conflict.start,
        (unsigned long long)apply_conflict.end,
        region_ok ? "true" : "false", (unsigned long long)region_start,
        (unsigned long long)region_end, (unsigned long long)region_used(),
        (unsigned long long)load_base, mode, (unsigned long long)attempts,
        (unsigned long long)retries, (unsigned long long)pc_hits,
        (unsigned long long)fp_hits, (unsigned long long)scan_hits,
        (unsigned long long)fp_frames, (unsigned long long)fp_incomplete,
        (unsigned long long)scan_words, (unsigned long long)cfi_frames,
        (unsigned long long)cfi_fail, unwind_us, park_us, (unsigned long long)unregistered_threads,
        (unsigned long long)scan.first_root, scan.us, (unsigned long long)scan.bytes,
        (unsigned long long)scan.hits, scan.retained, scan.candidates, scan.errors, scan.reclaimed,
        nframes, unwind_bytes122(),
        (unsigned long long)frame_registrations, (unsigned long long)frame_deregistrations,
        after_patch ? "true" : "false", callback_invoked ? "true" : "false",
        callback_invoked ? "true" : "false", use_safepoint ? "true" : "false",
        (unsigned long long)atomic_load(&tick_enter_count),
        (unsigned long long)atomic_load(&tick_leave_count));
    uselocale(previous_locale);
    if (len < 0 || (size_t)len >= sizeof(reply)) return;
    size_t pos = 0;
    while (pos < (size_t)len) {
        ssize_t sent = send(fd, reply + pos, (size_t)len - pos, MSG_NOSIGNAL);
        if (sent < 0 && errno == EINTR) continue;
        if (sent <= 0) return;
        pos += (size_t)sent;
    }
}
/* Report startup failures to stderr and the controller's private error file.
   A failed preload must not terminate the application or wait out a timeout. */
static void startup_failed(const char *reason) {
    fprintf(stderr, "cargo-refresh: agent startup failed: %s\n", reason);
    const char *path = getenv("REFRESH_AGENT_ERROR");
    if (path && path[0]) {
        int fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
        if (fd >= 0) {
            dprintf(fd, "%s\n", reason);
            close(fd);
        }
    }
}
static void *server(void *unused) {
    (void)unused;
    struct sockaddr_un addr = {.sun_family = AF_UNIX};
    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) { startup_failed(strerror(errno)); return NULL; }
    strcpy(addr.sun_path, socket_path);
    unlink(socket_path);
    if (bind(fd, (void *)&addr, sizeof(addr)) || listen(fd, 16)) {
        char reason[256];
        snprintf(reason, sizeof(reason), "cannot bind/listen on agent socket: %s", strerror(errno));
        startup_failed(reason);
        close(fd);
        return NULL;
    }
    for (;;) {
        int c = accept4(fd, NULL, NULL, SOCK_CLOEXEC);
        if (c < 0) continue;
        struct ucred peer;
        socklen_t peer_len = sizeof(peer);
        struct timeval timeout = {.tv_sec = 5};
        if (getsockopt(c, SOL_SOCKET, SO_PEERCRED, &peer, &peer_len) ||
            peer_len != sizeof(peer) || peer.uid != getuid() ||
            setsockopt(c, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) ||
            setsockopt(c, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout))) {
            close(c); continue;
        }
        char error[160] = "";
        apply_conflict = (struct refresh_conflict){0};
        int threads = 0, invoked = 0;
        double start = stamp();
        struct refresh_message message;
        enum refresh_code code = protocol_read(c, load_base, &message, error);
        if (code == REFRESH_OK) {
            if (message.type == REFRESH_HOOK) {
                if (after_patch) { after_patch(); invoked = 1; }
                else { code = REFRESH_INTERNAL; strcpy(error, "after-patch callback is unavailable"); }
            } else if (message.type == REFRESH_APPLY) {
                if (use_safepoint) pthread_rwlock_wrlock(&tick_gate);
                code = apply(error, &threads);
                if (use_safepoint) pthread_rwlock_unlock(&tick_gate);
                if (code == REFRESH_OK) {
                    for (int i = 0; i < nrec; ++i) patch_bytes_total += recs[i].length;
                    ++patch_count;
                    if (callback_enabled && after_patch) { after_patch(); invoked = 1; }
                }
            }
        }
        send_reply(c, &message, code, error, threads, stamp() - start, invoked);
        close(c);
        protocol_clear();
        discard_staged();
    }
    return NULL;
}
__attribute__((constructor)) static void init(void) {
    socket_path = refresh_env("REFRESH_AGENT_SOCKET", "E077_AGENT");
    if (!socket_path || !socket_path[0]) return;
    const char *host = refresh_env("REFRESH_HOST_EXE", "E106_HOST_EXE");
    /* New deployments must select one executable; legacy clients retain v0 behavior. */
    if (getenv("REFRESH_AGENT_SOCKET") && (!host || !host[0])) {
        fputs("cargo-refresh: REFRESH_HOST_EXE is required\n", stderr); return;
    }
    if (host) {
        char exe[4096];
        ssize_t n = readlink("/proc/self/exe", exe, sizeof(exe) - 1);
        if (n < 0 || n >= (ssize_t)sizeof(exe) - 1) return;
        exe[n] = 0;
        if (strcmp(exe, host)) return;
    }
    struct sockaddr_un address;
    if (strlen(socket_path) >= sizeof(address.sun_path)) {
        startup_failed("Unix socket path too long (maximum 107 bytes)");
        return;
    }
    const char *debug = refresh_env("REFRESH_DEBUG", "E121_DEBUG");
    const char *hook = refresh_env("REFRESH_AFTER_PATCH", "E099_AUTO_HOOK");
    const char *inherited = refresh_env("REFRESH_INHERITED", "E106_INHERITED_SEGMENTS");
    if (legacy_environment)
        fputs("cargo-refresh: legacy agent environment variables are deprecated; use REFRESH_*\n", stderr);
    if (debug && strcmp(debug, "0")) prctl(PR_SET_PTRACER, PR_SET_PTRACER_ANY, 0, 0, 0);
#ifdef REFRESH_TESTING
    const char *m = getenv("E084_MODE");
    if (m) {
        if (!strcmp(m, "ip")) mode = 0;
        else if (!strcmp(m, "fp")) mode = 1;
        else if (!strcmp(m, "cfi")) mode = 2;
        else if (!strcmp(m, "scan")) mode = 3;
    }
    use_safepoint = getenv("E054_SAFEPOINT") && atoi(getenv("E054_SAFEPOINT"));
    disable_unwind = getenv("E121_DISABLE_UNWIND") != NULL;
#endif
    /* Reserve before dlsym, inherited-list parsing or unwind setup can initialize
       a randomized brk heap inside the window. This path performs no malloc. */
    dl_iterate_phdr(base_cb, NULL);
    region_init();
    after_patch = (refresh_callback)dlsym(RTLD_DEFAULT, "fast_refresh_after_patch");
    callback_enabled = hook && !strcmp(hook, "1");
    if (adopt_inherited(inherited)) {
        fputs("cargo-refresh: invalid REFRESH_INHERITED; agent disabled\n", stderr);
        return;
    }
    unwind_init();
    if (park_init()) return;
    /* Application setlocale must not turn JSON decimals into comma decimals. */
    numeric_locale = newlocale(LC_NUMERIC_MASK, "C", (locale_t)0);
    if (!numeric_locale) return;
    pthread_t t;
    int error = pthread_create(&t, NULL, server, NULL);
    if (!error) pthread_detach(t);
    else startup_failed(strerror(error));
}
