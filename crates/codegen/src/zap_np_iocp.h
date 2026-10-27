/* =============================================================================
 * Windows IOCP netpoller backend — self-contained C ABI (ticket #196, platform
 * epic #158 / Area A2; design/0020-windows-iocp-netpoller-a2.md).
 *
 * This header declares the mechanism `zaprt.c`'s Windows `np_backend_*` arm (and
 * the standalone on-Windows harness `zap_np_iocp_test.c`) call. The mechanism is
 * defined in `zap_np_iocp.c`, a separate translation unit — like the A1 context
 * switch (`zap_ctx_win_x64.c`) and the A3 primitives (`zap_win_a3.c`) — so it
 * links into the Windows program build AND compiles standalone in the harness,
 * ahead of the full `zaprt.c` compiling on Windows (that still needs A4 #198).
 *
 * MODEL FIT (the crux I#196 flags). IOCP is a *completion* model; the netpoller's
 * `np_backend_*` seam and scheduler-facing ABI (`zap_netpoll_*`) are *readiness*-
 * oriented, exactly like epoll/kqueue. This layer bridges the two WITHOUT widening
 * the seam or touching Linux: it issues AFD *readiness-poll* requests
 * (`IOCTL_AFD_POLL` on a `\Device\Afd` helper handle) whose IOCP completions mean
 * "the readiness you asked about has arrived", which the poller maps straight onto
 * the existing `np_wake_dir_locked(pd, dir, OK)`. This is the wepoll/libuv/mio
 * mechanism. The scheduler-facing ABI is byte-for-byte unchanged.
 *
 * Each direction (read / write) is an INDEPENDENT one-shot AFD poll request, so a
 * late waiter in one direction never waits on the other direction's in-flight op.
 * A completion disarms that direction; `zaprt.c` re-arms it (gated so a latched-
 * but-undrained socket is not re-polled — I#196 Q1). Deadlines/timeouts stay in
 * `zaprt.c`'s backend-agnostic `np_compute_timeout` / `np_expire_deadlines`.
 *
 * Thread-safety: all entry points are internally serialized by this layer's own
 * lock; `zaprt.c` calls `arm`/`is_armed`/`unregister` under a `PollDesc` mutex and
 * `wait`/`kick` from the single poller thread. The layer never calls back into the
 * scheduler (no hooks) — `wait` returns readiness rows the caller translates.
 * ========================================================================== */
#ifndef ZAP_NP_IOCP_H
#define ZAP_NP_IOCP_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Direction selector for `zap_iocp_arm` / `zap_iocp_is_armed`. Mirrors the
 * netpoller's ZAP_POLL_READ/WRITE (1/2) but as a 0/1 index for a compact
 * two-slot request layout. */
enum { ZAP_IOCP_READ = 0, ZAP_IOCP_WRITE = 1 };

/* One readiness row returned by `zap_iocp_wait`. `userkey` is the opaque value
 * passed to `zap_iocp_register` (zaprt.c passes the owning `PollDesc *`). A single
 * completion sets exactly one of the read/write directions ready, except an
 * error/hangup condition (abort / disconnect / connect-fail / local-close), which
 * reports BOTH directions ready (+ `closed`) so the parked coroutine wakes and
 * observes the condition via its next socket call — mirroring how the epoll arm
 * treats EPOLLERR/EPOLLHUP. */
typedef struct {
    void *userkey;
    int readable;
    int writable;
    int closed;
} zap_iocp_ready;

/* Create the process-wide IOCP port + the `\Device\Afd` helper handle and
 * associate them. Returns 0 on success, -1 on failure (`zap_iocp_last_error`
 * carries the Win32/NTSTATUS-derived code). Idempotent-safe to call once from the
 * netpoller's lazy start. */
int zap_iocp_global_open(void);

/* Tear down the port + helper handle. Called at poller shutdown after the poller
 * thread has joined (no completions can still be in flight). */
void zap_iocp_global_close(void);

/* The last Win32 error code a failing entry point recorded (e.g. the base-handle
 * unwrap or an arm failure), for a clear runtime diagnostic instead of a silent
 * hang (I#196 Q2). */
int zap_iocp_last_error(void);

/* Register `sock` (a Windows `SOCKET`, carried in the netpoller ABI's `int64_t
 * fd`) for readiness polling; `userkey` is echoed back by `zap_iocp_wait`.
 * Unwraps the base socket handle via `SIO_BASE_HANDLE`, falling back to the
 * layered-LSP `SIO_BSP_HANDLE_POLL` / `SIO_BSP_HANDLE` descent chain for non-IFS
 * LSPs that break `SIO_BASE_HANDLE` (I#221, follow-up to I#196 Q2; mirrors wepoll).
 * Returns an opaque descriptor, or NULL on failure (un-unwrappable socket / OOM;
 * `zap_iocp_last_error` is set) — the caller reports it, never hangs. */
void *zap_iocp_register(uint64_t sock, void *userkey);

/* Deregister and release `pd`. Any in-flight AFD poll is cancelled; the descriptor
 * is freed immediately if nothing is in flight, else when the poller reaps the
 * (cancelled) completion — so it is never freed out from under a pending IOCP
 * packet. The caller still owns and should close the socket itself. */
void zap_iocp_unregister(void *pd);

/* Arm a one-shot AFD readiness poll for one direction of `pd` (ZAP_IOCP_READ /
 * _WRITE). Returns 0 on success (the poll is in flight), -1 on failure
 * (`zap_iocp_last_error` set). A no-op returning 0 if that direction is already
 * armed. */
int zap_iocp_arm(void *pd, int dir);

/* Whether a one-shot poll for `dir` is currently in flight on `pd`. */
int zap_iocp_is_armed(void *pd, int dir);

/* Block up to `timeout_ms` (<0 = infinite) for completions; fill up to `max`
 * readiness rows into `out`. Returns the row count (0 on timeout or on a bare
 * `zap_iocp_kick`), or -1 on an unexpected IOCP failure. Completions for a
 * descriptor being torn down are reaped and dropped (not reported). Called only
 * from the single poller thread. */
int zap_iocp_wait(zap_iocp_ready *out, int max, int timeout_ms);

/* Wake the poller thread blocked in `zap_iocp_wait` (the eventfd/self-pipe
 * equivalent): posts a sentinel completion with a NULL overlapped via
 * `PostQueuedCompletionStatus`. Used for deadline-recompute and shutdown. */
void zap_iocp_kick(void);

#ifdef __cplusplus
}
#endif

#endif /* ZAP_NP_IOCP_H */
