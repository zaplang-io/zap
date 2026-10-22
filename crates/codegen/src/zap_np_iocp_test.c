/* =============================================================================
 * On-Windows verification harness for the A2 IOCP netpoller backend (ticket
 * #196; design/0020). This is the verification the design could not run without a
 * Windows toolchain: it links the REAL backend (`zap_np_iocp.c`) and checks, on a
 * native windows-latest runner (MinGW-w64, the gc-boehm Windows v1 toolchain), the
 * load-bearing mechanisms in isolation — before the full `zaprt.c` compiles on
 * Windows (that still needs A4 #198 to guard the fs/process/socket POSIX includes).
 *
 *   T1. AFD-poll readiness: an armed READ poll does NOT complete while the socket
 *       is not readable (it times out), and DOES complete — reporting the right
 *       userkey + readable — once the peer sends. This is the completion->readiness
 *       bridge the whole design rests on.
 *   T2. Park/wake round-trip: a waiter thread parks (blocks on a condvar, the
 *       copark analogue) on a not-yet-readable socket while a poller thread runs
 *       the real zap_iocp_wait loop; a peer send produces a completion the poller
 *       turns into a wake (the coready analogue). Proves a blocked waiter is woken
 *       by an IOCP completion packet.
 *   T3. The PostQueuedCompletionStatus kick (`zap_iocp_kick`) promptly unblocks a
 *       poller parked in zap_iocp_wait — the eventfd/self-pipe-kick equivalent used
 *       for deadline-recompute and shutdown.
 *   T4. Teardown: unregistering a descriptor with an in-flight poll (the CancelIoEx
 *       path) and reaping the cancelled completion does not leak or crash.
 *
 * The full M:N copark/coready + per-direction latch + deadline logic lives in
 * `zaprt.c` and is exercised unchanged by the Linux `netpoller.zap` golden under
 * both collectors; this harness proves the Windows-specific mechanism + the wake
 * delivery that `zaprt.c`'s Windows np_poller_main is built on.
 *
 * Build (on Windows, MinGW-w64):
 *   gcc -O2 -Wall zap_np_iocp_test.c zap_np_iocp.c -lws2_32 -o t && ./t
 * Exit 0 on success; nonzero (with a diagnostic) on any failure.
 * ========================================================================== */
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
/* CONDITION_VARIABLE / SleepConditionVariableCS need Vista+ (0x0600). */
#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0601
#endif
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>

#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "zap_np_iocp.h"

static int g_failures = 0;
#define CHECK(cond, msg)                                                                  \
    do {                                                                                  \
        if (!(cond)) {                                                                    \
            fprintf(stderr, "FAIL: %s (%s:%d)\n", (msg), __FILE__, __LINE__);             \
            g_failures++;                                                                 \
        } else {                                                                          \
            fprintf(stderr, "ok: %s\n", (msg));                                           \
        }                                                                                 \
    } while (0)

/* Create a connected loopback TCP socket pair with raw Winsock (NOT the Zap net
 * module — A4 #198 is out of A2 scope, per the orchestrator). Returns 0 on
 * success, filling *a (the end the test polls) and *b (its peer). */
static int make_socket_pair(SOCKET *a, SOCKET *b) {
    SOCKET lsn = WSASocketW(AF_INET, SOCK_STREAM, IPPROTO_TCP, NULL, 0, WSA_FLAG_OVERLAPPED);
    if (lsn == INVALID_SOCKET) {
        return -1;
    }
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof addr);
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    addr.sin_port = 0;
    if (bind(lsn, (struct sockaddr *)&addr, sizeof addr) != 0 || listen(lsn, 1) != 0) {
        closesocket(lsn);
        return -1;
    }
    int alen = (int)sizeof addr;
    if (getsockname(lsn, (struct sockaddr *)&addr, &alen) != 0) {
        closesocket(lsn);
        return -1;
    }
    SOCKET cli = WSASocketW(AF_INET, SOCK_STREAM, IPPROTO_TCP, NULL, 0, WSA_FLAG_OVERLAPPED);
    if (cli == INVALID_SOCKET) {
        closesocket(lsn);
        return -1;
    }
    /* Blocking connect over loopback completes immediately. */
    if (connect(cli, (struct sockaddr *)&addr, sizeof addr) != 0) {
        closesocket(lsn);
        closesocket(cli);
        return -1;
    }
    SOCKET srv = accept(lsn, NULL, NULL);
    closesocket(lsn);
    if (srv == INVALID_SOCKET) {
        closesocket(cli);
        return -1;
    }
    *a = cli; /* the end the harness polls */
    *b = srv; /* the peer that sends */
    return 0;
}

/* ---- T1: AFD-poll readiness ----------------------------------------------- */
static void test_readiness(void) {
    SOCKET rd, wr;
    CHECK(make_socket_pair(&rd, &wr) == 0, "T1 loopback socket pair");
    int marker = 0xA2;
    void *pd = zap_iocp_register((uint64_t)rd, &marker);
    CHECK(pd != NULL, "T1 register (SIO_BASE_HANDLE unwrap)");
    if (!pd) {
        closesocket(rd);
        closesocket(wr);
        return;
    }
    CHECK(zap_iocp_arm(pd, ZAP_IOCP_READ) == 0, "T1 arm read");
    CHECK(zap_iocp_is_armed(pd, ZAP_IOCP_READ), "T1 read armed");

    zap_iocp_ready readies[8];
    int n = zap_iocp_wait(readies, 8, 150);
    CHECK(n == 0, "T1 not readable yet -> wait times out");

    /* Some stacks disarm a one-shot poll on timeout; re-arm to be safe. */
    zap_iocp_arm(pd, ZAP_IOCP_READ);
    char msg = 'Z';
    CHECK(send(wr, &msg, 1, 0) == 1, "T1 peer send");

    n = zap_iocp_wait(readies, 8, 2000);
    CHECK(n == 1, "T1 readiness completion arrives");
    if (n == 1) {
        CHECK(readies[0].userkey == &marker, "T1 completion carries the userkey");
        CHECK(readies[0].readable, "T1 reported readable");
    }
    char buf = 0;
    CHECK(recv(rd, &buf, 1, 0) == 1 && buf == 'Z', "T1 drains the sent byte");

    zap_iocp_unregister(pd);
    closesocket(rd);
    closesocket(wr);
}

/* ---- T2: park/wake round-trip --------------------------------------------- */
typedef struct {
    CRITICAL_SECTION mu;
    CONDITION_VARIABLE cv;
    int woken;
} park_slot;

static volatile LONG g_poller_stop;
static void *g_poll_pd;      /* descriptor the poller keeps armed for READ        */
static park_slot *g_park;    /* woken when the poller sees READ readiness          */

static DWORD WINAPI poller_main(LPVOID arg) {
    (void)arg;
    zap_iocp_ready readies[8];
    while (!InterlockedCompareExchange(&g_poller_stop, 0, 0)) {
        if (g_poll_pd && !zap_iocp_is_armed(g_poll_pd, ZAP_IOCP_READ)) {
            zap_iocp_arm(g_poll_pd, ZAP_IOCP_READ); /* re-arm (np_iocp_arm_needed) */
        }
        int n = zap_iocp_wait(readies, 8, 100);
        for (int i = 0; i < n; i++) {
            if (readies[i].readable && g_park) {
                EnterCriticalSection(&g_park->mu);
                g_park->woken = 1;           /* coready analogue */
                WakeAllConditionVariable(&g_park->cv);
                LeaveCriticalSection(&g_park->mu);
            }
        }
    }
    return 0;
}

static void test_park_wake(void) {
    SOCKET rd, wr;
    CHECK(make_socket_pair(&rd, &wr) == 0, "T2 loopback socket pair");
    int marker = 0x55;
    void *pd = zap_iocp_register((uint64_t)rd, &marker);
    CHECK(pd != NULL, "T2 register");
    if (!pd) {
        closesocket(rd);
        closesocket(wr);
        return;
    }

    park_slot park;
    InitializeCriticalSection(&park.mu);
    InitializeConditionVariable(&park.cv);
    park.woken = 0;

    g_poller_stop = 0;
    g_poll_pd = pd;
    g_park = &park;
    HANDLE th = CreateThread(NULL, 0, poller_main, NULL, 0, NULL);
    CHECK(th != NULL, "T2 poller thread start");

    /* The waiter "parks": block on the condvar until the poller wakes it. Let the
     * poller arm the socket first, confirm no spurious wake, then send. */
    Sleep(150);
    EnterCriticalSection(&park.mu);
    int early = park.woken;
    LeaveCriticalSection(&park.mu);
    CHECK(!early, "T2 no wake before readiness (parked)");

    char msg = 'Q';
    CHECK(send(wr, &msg, 1, 0) == 1, "T2 peer send");

    EnterCriticalSection(&park.mu);
    BOOL got = TRUE;
    while (!park.woken) {
        if (!SleepConditionVariableCS(&park.cv, &park.mu, 2000)) {
            got = FALSE;
            break;
        }
    }
    LeaveCriticalSection(&park.mu);
    CHECK(got && park.woken, "T2 waiter woken by the completion");

    InterlockedExchange(&g_poller_stop, 1);
    zap_iocp_kick();
    WaitForSingleObject(th, 2000);
    CloseHandle(th);
    g_poll_pd = NULL;
    g_park = NULL;

    char buf = 0;
    CHECK(recv(rd, &buf, 1, 0) == 1 && buf == 'Q', "T2 drains the byte after wake");
    zap_iocp_unregister(pd);
    DeleteCriticalSection(&park.mu);
    closesocket(rd);
    closesocket(wr);
}

/* ---- T3: PostQueuedCompletionStatus kick ---------------------------------- */
static DWORD WINAPI kicker_main(LPVOID arg) {
    (void)arg;
    Sleep(150);
    zap_iocp_kick();
    return 0;
}

static void test_kick(void) {
    HANDLE th = CreateThread(NULL, 0, kicker_main, NULL, 0, NULL);
    CHECK(th != NULL, "T3 kicker thread start");
    zap_iocp_ready readies[8];
    DWORD t0 = GetTickCount();
    int n = zap_iocp_wait(readies, 8, 5000); /* would block 5s without the kick */
    DWORD elapsed = GetTickCount() - t0;
    CHECK(n == 0, "T3 kick yields a no-readiness return");
    CHECK(elapsed < 2000, "T3 kick unblocks the wait promptly");
    WaitForSingleObject(th, 2000);
    CloseHandle(th);
}

/* ---- T4: teardown of an armed descriptor (CancelIoEx path) ----------------- */
static void test_teardown_armed(void) {
    SOCKET rd, wr;
    CHECK(make_socket_pair(&rd, &wr) == 0, "T4 loopback socket pair");
    int marker = 0x11;
    void *pd = zap_iocp_register((uint64_t)rd, &marker);
    CHECK(pd != NULL, "T4 register");
    if (!pd) {
        closesocket(rd);
        closesocket(wr);
        return;
    }
    CHECK(zap_iocp_arm(pd, ZAP_IOCP_READ) == 0, "T4 arm read (nothing will send)");
    /* Unregister while the poll is in flight: the cancellation posts a completion
     * that the next wait reaps and drops (deferred free) — no leak, no crash. */
    zap_iocp_unregister(pd);
    zap_iocp_ready readies[8];
    int n = zap_iocp_wait(readies, 8, 500);
    CHECK(n == 0, "T4 cancelled completion reaped, not reported");
    closesocket(rd);
    closesocket(wr);
}

int main(void) {
    WSADATA wsa;
    if (WSAStartup(MAKEWORD(2, 2), &wsa) != 0) {
        fprintf(stderr, "FATAL: WSAStartup failed\n");
        return 2;
    }
    if (zap_iocp_global_open() != 0) {
        fprintf(stderr, "FATAL: zap_iocp_global_open failed (err %d)\n", zap_iocp_last_error());
        WSACleanup();
        return 2;
    }

    fprintf(stderr, "== T1 AFD-poll readiness ==\n");
    test_readiness();
    fprintf(stderr, "== T2 park/wake round-trip ==\n");
    test_park_wake();
    fprintf(stderr, "== T3 PostQueuedCompletionStatus kick ==\n");
    test_kick();
    fprintf(stderr, "== T4 teardown of an armed descriptor ==\n");
    test_teardown_armed();

    zap_iocp_global_close();
    WSACleanup();

    if (g_failures == 0) {
        fprintf(stderr, "zap_np_iocp_test: all checks passed\n");
        return 0;
    }
    fprintf(stderr, "zap_np_iocp_test: %d check(s) FAILED\n", g_failures);
    return 1;
}
