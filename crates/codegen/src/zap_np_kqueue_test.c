/* =============================================================================
 * macOS verification harness for the kqueue netpoller backend (ticket #224,
 * platform epic #158 / macOS Area; design/0029). Unlike the Windows A2 harness
 * (`zap_np_iocp_test.c`) — which links an *isolated* backend TU because `zaprt.c`
 * does not yet compile on Windows — macOS IS POSIX and `zaprt.c` compiles and
 * links natively, so this harness links the REAL runtime (`zaprt.c`, which
 * #includes the Immix collector under -DZAP_GC_IMMIX) and drives the actual
 * in-runtime netpoller self-test through the real M:N scheduler + channels.
 *
 * It calls `zap_netpoll_selftest()` — the same routine the `netpoller.zap` golden
 * runs — which, on a SINGLE worker M (so a parked reader is the only way the
 * writer can ever run), asserts:
 *   1. a reader coroutine parks on a not-yet-readable pipe (freeing its M), a
 *      writer coroutine runs on that freed M and writes the sentinel byte, and the
 *      kqueue EVFILT_READ edge wakes the parked reader which drains the byte; and
 *   2. a reader whose pipe never becomes readable is woken with ZAP_POLL_TIMEOUT
 *      when its deadline passes (the np_compute_timeout -> kevent timespec path).
 * Together these exercise the kqueue arm end-to-end — EVFILT_READ/WRITE arming,
 * the EVFILT_USER kick (deadline-recompute), edge-latched readiness, the
 * copark/coready wake — on top of the ucontext context switch the macOS build
 * keeps behind the A1 seam. A green run is the on-macOS proof the design rests on,
 * and a cheap CI signal that needs no LLVM.
 *
 * Build (on macOS; both collectors, mirroring build_executable's macOS link):
 *   # precise Immix (default):
 *   cc -std=c11 -DZAP_GC_IMMIX zap_np_kqueue_test.c zaprt.c -o t \
 *      $(pkg-config --cflags --libs python3-embed) \
 *      -lc++ -lm -lpcre2-8 -lz -lssl -lcrypto -lpthread && ./t
 *   # conservative Boehm (fallback):
 *   cc -std=c11 zap_np_kqueue_test.c zaprt.c -o t \
 *      $(pkg-config --cflags --libs python3-embed) \
 *      -lc++ -lm -lpcre2-8 -lz -lssl -lcrypto -lpthread -lgc && ./t
 * ========================================================================== */
#include <stdio.h>

/* zaprt.c's internal ABI (int64_t); declared here to avoid pulling the runtime's
 * private headers. Returns the number of sub-checks passed (2 on full success). */
extern long long zap_netpoll_selftest(void);

int main(void) {
    long long passed = zap_netpoll_selftest();
    if (passed == 2) {
        printf("kqueue netpoller selftest: OK (%lld/2)\n", passed);
        return 0;
    }
    fprintf(stderr, "kqueue netpoller selftest: FAIL (%lld/2)\n", passed);
    return 1;
}
