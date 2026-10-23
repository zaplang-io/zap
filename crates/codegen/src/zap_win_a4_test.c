/* =============================================================================
 * Windows A4 runtime integration smoke test (ticket #198, platform epic #158).
 *
 * This is the on-`windows-latest` verification that the A4 port of the C runtime
 * (`zaprt.c`) compiles, LINKS into a real executable against the MinGW-w64 /
 * MSYS2 runtime libraries, and RUNS — the milestone A4 is responsible for: a Zap
 * program can run on Windows. It drives the public runtime ABI directly from C
 * (the LLVM-backed Zap compiler is not yet built on Windows — see the PR body),
 * exercising exactly the subsystems A4 ported behind the `ZAP_PLATFORM_WINDOWS`
 * seam, end to end and integrated with the A1 context switch, A2 IOCP netpoller
 * and A3 primitives the same link pulls in:
 *
 *   T1  fs/io:   mkdtemp -> write_file -> read_file (byte-exact, O_BINARY) ->
 *                size -> is_file/exists -> listdir.
 *   T2  os/env:  setenv -> getenv/hasenv round-trip.
 *   T3  path:    os.path_join keeps '/'-joined output and tolerates a '\\' or a
 *                drive-absolute right operand (the logged Path::join gap).
 *   T4  crypto:  BCryptGenRandom-backed secure random fills the requested length.
 *   T5  net:     a Winsock listener binds an ephemeral port (socket/bind/listen/
 *                getsockname + IOCP/AFD netpoller registration) and closes.
 *   T6  sched:   a coroutine spawned on the M:N scheduler (A1 hand-asm context
 *                switch + Windows preempt wiring) sends over a channel and the
 *                main thread receives it — the headline "the scheduler runs on
 *                Windows" check — then zap_rt_join_all tears everything down.
 *
 * Build (see the `windows-runtime` CI job): the shipping runtime TUs plus this
 * harness, linked against the MSYS2 mingw-w64-x86_64-{gc,pcre2,zlib,openssl}
 * import libraries and Winsock/BCrypt/DbgHelp. CPython is gated off for Windows
 * v1, so no `-lpython`. Exit 0 on success; any runtime fault/panic aborts (the
 * CI step fails), which is the intended pass/fail signal.
 * ========================================================================== */
#include <stdint.h>
#include <stdio.h>
#include <string.h>

/* The two-word string object (RUNTIME-ABI.md): { len, data }. */
typedef struct {
    int64_t len;
    const char *data;
} ZapStr;

/* Public runtime ABI (subset) — defined in zaprt.c. */
extern void *zap_str_new(const char *data, int64_t len);
/* os / env */
extern void *zap_os_getcwd(void);
extern void zap_os_setenv(void *name, void *val);
extern void *zap_os_getenv(void *name);
extern int64_t zap_os_hasenv(void *name);
extern void *zap_os_path_join(void *a, void *b);
extern void *zap_os_listdir(void *path);
/* fs / io */
extern void *zap_fs_mkdtemp(void);
extern void zap_fs_write_file(void *path, void *data);
extern void *zap_fs_read_file(void *path);
extern int64_t zap_fs_size(void *path);
extern int64_t zap_fs_is_file(void *path);
extern int64_t zap_fs_exists(void *path);
/* collections */
extern int64_t zap_list_len(void *l);
/* crypto */
extern void *zap_crypto_random_bytes(int64_t n);
/* net */
extern void *zap_net_listen(void *host, int64_t port);
extern int64_t zap_net_port(void *s);
extern void zap_net_close(void *s);
/* scheduler + channels */
extern void *zap_chan_new(int64_t cap);
extern void zap_spawn(void *fn, void *arg);
extern void zap_chan_send_i64(void *ch, int64_t v);
extern int64_t zap_chan_recv_i64(void *ch);
extern void zap_rt_join_all(void);

static int g_failures = 0;
#define CHECK(cond, label)                                                                     \
    do {                                                                                       \
        if (cond) {                                                                            \
            printf("ok   - %s\n", label);                                                      \
        } else {                                                                               \
            printf("FAIL - %s\n", label);                                                      \
            g_failures++;                                                                      \
        }                                                                                      \
    } while (0)

static void *s(const char *c) { return zap_str_new(c, (int64_t)strlen(c)); }
static int streq(void *zs, const char *c) {
    ZapStr *z = (ZapStr *)zs;
    size_t n = strlen(c);
    return z->len == (int64_t)n && memcmp(z->data, c, n) == 0;
}

/* T6 coroutine body: runs on a green stack via the A1 context switch. */
static void *producer(void *arg) {
    void *ch = arg;
    zap_chan_send_i64(ch, 0x5A33); /* 'Z3' */
    return NULL;
}

int main(void) {
    printf("== Zap Windows A4 runtime smoke ==\n");

    /* T1 fs/io: temp dir, byte-exact write/read, size, predicates, listdir. */
    void *dir = zap_fs_mkdtemp();
    CHECK(((ZapStr *)dir)->len > 0, "T1 mkdtemp returns a path");
    void *file = zap_os_path_join(dir, s("data.bin"));
    const char payload[] = "zap\r\nwin\x00\x01\x02tail"; /* embedded NUL + CRLF */
    void *data = zap_str_new(payload, (int64_t)sizeof(payload) - 1);
    zap_fs_write_file(file, data);
    void *back = zap_fs_read_file(file);
    CHECK(((ZapStr *)back)->len == (int64_t)sizeof(payload) - 1 &&
              memcmp(((ZapStr *)back)->data, payload, sizeof(payload) - 1) == 0,
          "T1 read_file is byte-exact (O_BINARY, no CRLF/NUL mangling)");
    CHECK(zap_fs_size(file) == (int64_t)sizeof(payload) - 1, "T1 fs.size matches");
    CHECK(zap_fs_is_file(file) == 1 && zap_fs_exists(file) == 1, "T1 is_file/exists");
    CHECK(zap_list_len(zap_os_listdir(dir)) == 1, "T1 listdir sees the one file");

    /* T2 os/env round-trip. */
    zap_os_setenv(s("ZAP_WIN_SMOKE"), s("hello-win"));
    CHECK(zap_os_hasenv(s("ZAP_WIN_SMOKE")) == 1, "T2 hasenv after setenv");
    CHECK(streq(zap_os_getenv(s("ZAP_WIN_SMOKE")), "hello-win"), "T2 getenv round-trip");
    CHECK(((ZapStr *)zap_os_getcwd())->len > 0, "T2 getcwd");

    /* T3 path_join: '/'-joined output; '\\' and drive-absolute right operands. */
    CHECK(streq(zap_os_path_join(s("a/b"), s("c")), "a/b/c"), "T3 join inserts '/'");
    CHECK(streq(zap_os_path_join(s("a/b/"), s("c")), "a/b/c"), "T3 join no double sep ('/')");
    CHECK(streq(zap_os_path_join(s("a\\b"), s("c")), "a\\b/c"),
          "T3 join tolerates a trailing '\\'");
    CHECK(streq(zap_os_path_join(s("a"), s("C:\\x")), "C:\\x"),
          "T3 drive-absolute right operand replaces left");

    /* T4 crypto secure random (BCryptGenRandom). */
    void *rnd = zap_crypto_random_bytes(32);
    CHECK(((ZapStr *)rnd)->len == 32, "T4 crypto.random_bytes length");

    /* T5 net: ephemeral Winsock listener + netpoller registration. */
    void *lst = zap_net_listen(s("127.0.0.1"), 0);
    int64_t port = zap_net_port(lst);
    CHECK(port > 0 && port < 65536, "T5 net.listen binds an ephemeral port");
    zap_net_close(lst);

    /* T6 the M:N scheduler runs a coroutine on Windows (A1 ctx switch + A4). */
    void *ch = zap_chan_new(1);
    zap_spawn((void *)producer, ch);
    int64_t got = zap_chan_recv_i64(ch);
    CHECK(got == 0x5A33, "T6 coroutine -> channel -> main round-trip");
    zap_rt_join_all();

    if (g_failures == 0) {
        printf("ALL PASS: Zap runtime runs on Windows (A4)\n");
        return 0;
    }
    printf("FAILURES: %d\n", g_failures);
    return 1;
}
