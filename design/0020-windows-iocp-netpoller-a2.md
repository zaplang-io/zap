# Proposal: Windows IOCP netpoller backend — I#196 (A2)

- **Author(s):** win-iocp-netpoller session (I#196)
- **Last updated:** 2026-10-23
- **Status:** Draft (design — pending orchestrator sign-off before implementation)
- **Discussion:** https://github.com/parveshgarg/zap/issues/196
- **Epic:** I#158 (platform/1.0), Area A2. See `design/0016-platform-1.0.md`,
  and the sibling Windows lanes `design/0017` (A1 ctxswitch), `design/0018`
  (A5 toolchain), `design/0019` (A3 signals/faults/SEH).

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

The netpoller (I#80) parks a coroutine blocked on a not-yet-ready fd and frees
its M, waking it on a readiness edge delivered by a dedicated poller OS thread.
The readiness mechanism sits behind a thin backend seam —
`np_backend_open/add/del/kick` plus a backend-specific poller loop — that is
`epoll`+`eventfd` on Linux and an "unsupported" stub elsewhere.
This proposal implements that seam on Windows using **IOCP**
(`CreateIoCompletionPort` / `GetQueuedCompletionStatusEx` /
`PostQueuedCompletionStatus`).

The crux is that IOCP is a *completion* model — you learn that an overlapped
operation finished — whereas the seam and the scheduler-facing ABI
(`zap_netpoll_open/wait/wake/close`) are *readiness*-oriented, exactly like
`epoll`/`kqueue`.
The recommendation is to **keep the readiness contract and the scheduler-facing
ABI byte-for-byte unchanged**, and bridge the completion model to it with a thin
Windows-only adapter built on **AFD poll** (`\Device\Afd` +
`IOCTL_AFD_POLL` via `NtDeviceIoControlFile`), which turns IOCP into a
readiness source.
No seam widening touches Linux; the only shared-struct change is Windows-only
fields on `PollDesc` guarded by `#if defined(ZAP_PLATFORM_WINDOWS)`.

## Background

### The seam as built (I#80, `crates/codegen/src/zaprt.c`)

The scheduler-facing ABI is stable and is what I#77 (signals) and the future
socket layer (I#104 / A4 #198) call:

```
void*   zap_netpoll_open(int64_t fd)                  # register; NULL if unsupported
int64_t zap_netpoll_wait(void* pd, mode, deadline_ns) # park; 0=OK 1=TIMEOUT 2=CLOSED
void    zap_netpoll_wake(void* pd, mode)              # force-wake a parked waiter
void    zap_netpoll_close(void* pd)                   # deregister + release
```

`mode` is `ZAP_POLL_READ` (1) / `ZAP_POLL_WRITE` (2).
The contract is epoll/Go-style **readiness**: the caller sets the fd
non-blocking, calls `zap_netpoll_wait(READ)`, and on `ZAP_POLL_OK` performs a
non-blocking `recv()` draining to `EAGAIN`/`WSAEWOULDBLOCK`.
A readiness edge that arrives before any waiter parks is **latched** per
direction so the next `wait` returns immediately.

Underneath, a backend produces readiness events:

- `np_backend_open()` — create the readiness instance (Linux: `epoll_create1` +
  an `eventfd` kick fd registered `EPOLLIN`).
- `np_backend_add(pd)` — register `pd->fd` once for both directions
  (`EPOLLIN|EPOLLOUT|EPOLLRDHUP|EPOLLET`).
- `np_backend_del(pd)` — deregister.
- `np_backend_kick()` — wake the poller thread (Linux: `write()` the eventfd).
- `np_poller_main()` — the poller loop, already wrapped in
  `#if defined(ZAP_NETPOLL_EPOLL)`.
  It blocks in `epoll_wait` with a timeout from `np_compute_timeout()`, then for
  each ready fd calls `np_wake_dir_locked(pd, is_read, ZAP_POLL_OK)` (wakes the
  parked `Waiter` via `coready`, or latches readiness), and finally runs
  `np_expire_deadlines()`.

The **generic** pieces — `PollDesc` registry, the per-direction waiter/latch/
deadline slots, `np_wake_dir_locked`, `np_compute_timeout`, `np_expire_deadlines`,
and the entire scheduler-facing ABI — are backend-agnostic.
Only `np_backend_*` and `np_poller_main` are backend-specific, and
`np_poller_main` is *already* conditionally compiled per backend.

### What the Windows netpoller must (and need not) serve

- I#77 signal dispatch **does not** use the netpoller on Windows — A3 §5
  (`design/0019`) deliberately decoupled Windows signal dispatch onto a console
  handler + condvar.
  So the Windows netpoller's only consumer is **sockets** (future A4 #198).
- AFD poll is a **socket-only** readiness mechanism, which exactly matches that
  single consumer.
  The I#80 self-test uses an anonymous `pipe()`; on Windows those fds are not
  sockets, so the Windows harness must use a loopback socket pair instead (see
  Testing).
- Windows v1 runs under **boehm** (immix needs a separate `mmap`→`VirtualAlloc`
  port, I#218; out of this lane — matches A1/A3).

### Why "unsupported stub compiles" ≠ "A2 done"

The non-Linux `np_backend_*` arm already compiles (it returns `-1`/no-ops), so
the netpoller section is *not itself* a compile wall.
A2's job is to ship the **real** IOCP backend behind the seam (dead code until
the TU compiles on Windows, which still needs A4), add a real-runner CI job that
proves it, and narrow the `#error` gate from "A2/A4 remain" to "A4 remains".

## Proposal

### Model-fit verdict: readiness contract preserved; thin adapter; no Linux change

The central question the orchestrator asked — *does the readiness-oriented seam
accommodate a completion model as-is, need a thin adapter, or need the seam
widened (which affects Linux)?* — resolves to **thin adapter, no seam
widening**:

1. **Scheduler-facing ABI: unchanged.**
   `zap_netpoll_{open,wait,wake,close}` keep identical signatures and identical
   readiness semantics (`OK`/`TIMEOUT`/`CLOSED`, per-direction latch, deadlines).
   The socket layer (A4) that sits on top is written once, POSIX-shaped:
   non-blocking socket, `wait(READ)`, `recv()` to `WSAEWOULDBLOCK`.

2. **Generic poller core: unchanged and reused verbatim.**
   `PollDesc` waiter/latch/deadline logic, `np_wake_dir_locked`,
   `np_compute_timeout`, `np_expire_deadlines`, the registry, lock order — all
   reused as-is.
   The completion→readiness translation happens entirely inside the Windows
   `np_poller_main` and the Windows `np_backend_*`.

3. **Bridge: AFD poll turns IOCP completions into readiness edges.**
   Instead of issuing real overlapped `WSARecv`/`WSASend` (which would transfer
   data and force a *completion* contract — see Rationale), we post a
   **readiness query** overlapped op to the socket's base AFD handle:
   `IOCTL_AFD_POLL` with an `AFD_POLL_INFO` naming the directions of interest
   (`AFD_POLL_RECEIVE`, `AFD_POLL_SEND`, `AFD_POLL_DISCONNECT`,
   `AFD_POLL_ABORT`, `AFD_POLL_LOCAL_CLOSE`, `AFD_POLL_CONNECT_FAIL`).
   When any requested condition becomes true, IOCP delivers a completion packet;
   the poller reads which bits fired and calls the existing
   `np_wake_dir_locked(pd, is_read, ZAP_POLL_OK)`.
   This is the mechanism `wepoll`, `libuv`, Rust `mio`, and tokio use to expose
   epoll-like readiness on Windows — battle-tested, though the AFD IOCTL surface
   is only semi-documented (lives in `ntdll`).

The completion model is therefore **accommodated via a thin adapter**; the seam
is not widened in any way visible to Linux.
The only shared-definition change is Windows-only fields on `PollDesc`
(`#if defined(ZAP_PLATFORM_WINDOWS)`): a per-direction `OVERLAPPED` and an
`AFD_POLL_INFO` buffer, plus the socket's base handle.
These are appended under an `#ifdef`, do not change the struct's layout on
Linux, and are invisible to the generic core.

### Windows backend shape

```
np_backend_open():
    g_np_iocp = CreateIoCompletionPort(INVALID_HANDLE_VALUE, NULL, 0, 0);
    open a handle to \Device\Afd (NtCreateFile) for posting IOCTL_AFD_POLL;
    (kick needs no fd — PostQueuedCompletionStatus targets the port directly.)

np_backend_add(pd):
    base = WSAIoctl(pd->fd, SIO_BASE_HANDLE, ...);   # unwrap LSP layers
    CreateIoCompletionPort(base, g_np_iocp, (ULONG_PTR)pd, 0);
    arm an IOCTL_AFD_POLL for READ|WRITE interest on `pd` (one-shot);

np_backend_del(pd):
    cancel any in-flight AFD poll for pd (CancelIoEx) and drain its completion;
    (IOCP association is dropped when the base handle closes — caller owns it.)

np_backend_kick():
    PostQueuedCompletionStatus(g_np_iocp, 0, ZAP_NP_WAKE_KEY, NULL);

np_poller_main():      # Windows arm, sibling to the epoll arm
    loop:
        timeout = np_compute_timeout();              # reused verbatim
        GetQueuedCompletionStatusEx(g_np_iocp, entries, 64, &n, timeout, FALSE);
        if g_np_shutdown: break;
        for each entry:
            if key == ZAP_NP_WAKE_KEY: continue;     # kick/shutdown: recompute
            pd = (PollDesc*)entry.lpCompletionKey;
            read AFD_POLL_INFO result bits →
                np_wake_dir_locked(pd, READ, OK)  / (pd, WRITE, OK);  # reused
            RE-ARM the AFD poll for pd for still-wanted directions;   # one-shot
        np_expire_deadlines();                       # reused verbatim
```

### 1:1 mappings (completion primitive → existing readiness primitive)

| Readiness seam (Linux)                 | IOCP/AFD (Windows)                                   |
|----------------------------------------|------------------------------------------------------|
| `epoll_create1`                        | `CreateIoCompletionPort(INVALID_HANDLE_VALUE,…)`     |
| `epoll_ctl(ADD, EPOLLIN\|EPOLLOUT\|ET)`| associate base handle + arm `IOCTL_AFD_POLL`         |
| `epoll_wait(timeout)`                  | `GetQueuedCompletionStatusEx(…, timeout, FALSE)`     |
| ready event → `np_wake_dir_locked`     | completion bits → `np_wake_dir_locked` + **re-arm**  |
| `eventfd` write (`np_backend_kick`)    | `PostQueuedCompletionStatus(port, 0, WAKE_KEY, NULL)`|
| `EPOLLET` edge + per-dir latch         | one-shot AFD poll + per-dir latch (re-armed by loop) |
| `epoll_ctl(DEL)`                        | `CancelIoEx` + drain                                 |

The deadline/timeout path needs **zero** change: `np_compute_timeout()` already
returns a millisecond timeout that drops straight into
`GetQueuedCompletionStatusEx`, and `np_expire_deadlines()` is backend-agnostic.

### How a coroutine parks and wakes (unchanged flow)

1. Socket code (A4) sets the socket non-blocking, tries `recv`, gets
   `WSAEWOULDBLOCK`, calls `zap_netpoll_wait(pd, READ, deadline)`.
2. `zap_netpoll_wait` parks the coroutine via `copark` (frees its M) exactly as
   today — it is backend-agnostic.
3. The poller thread, blocked in `GetQueuedCompletionStatusEx`, receives the AFD
   completion when the socket becomes readable, calls
   `np_wake_dir_locked(pd, READ, OK)` → `coready(w)` re-queues the coroutine, and
   re-arms the AFD poll.
4. The coroutine resumes on some M, calls `recv` (now succeeds), drains to
   `WSAEWOULDBLOCK`, and loops back to `wait` if it needs more.

### GC safepoint / STW and A3 preemption vs. a thread in `GetQueuedCompletionStatusEx`

- The poller thread is **not a registered mutator** (same as the Linux epoll
  thread): it allocates no GC memory and touches only non-traced scheduler
  fields under their mutexes.
  It is created with `GC_pthread_create` (winpthreads under MinGW provides the
  pthread surface already used throughout zaprt.c), so boehm accounts for the
  thread, but STW does not need to interrupt it mid-`GetQueuedCompletionStatusEx`
  — blocking there is exactly analogous to blocking in `epoll_wait`.
- A3 preemption (`SuspendThread`-hijack, boehm) targets **running mutator M
  threads at a safepoint** only; the poller thread is excluded just as the Linux
  `SIGURG` kick only targets mutators.
  A coroutine *blocked on I/O* has already parked and freed its M — there is
  nothing running to preempt; its M is off running other coroutines.
  The completion simply re-queues it via `coready`.
- Lock discipline is unchanged: `g_np_mu` then `pd->mu`, never reversed; the
  poller holds only these, never a GC lock, so it cannot deadlock against STW.

### The `PostQueuedCompletionStatus` wakeup (eventfd/self-pipe equivalent)

`np_backend_kick()` posts a sentinel completion with key `ZAP_NP_WAKE_KEY` and a
`NULL` overlapped; the poller recognizes the key and simply recomputes its
timeout / observes `g_np_shutdown`.
This is the exact 1:1 of the Linux `eventfd` kick (whose completion the loop
just drains).
Shutdown is identical to Linux: set `g_np_shutdown`, kick, join.

## Rationale

### Why AFD-readiness and not true overlapped (completion) I/O

The issue (and `design/0016`) call out two routes: an **AFD-readiness shim** or
switching to **`RIO`/overlapped sockets** (a "completion-model variant of the
netpoller abstraction").

- **True overlapped `WSARecv`/`WSASend`** is the canonical Windows idiom and
  avoids the semi-documented AFD surface.
  But it *breaks the readiness contract on Windows*: a completion means the data
  has **already** been transferred into a buffer, not "the socket is now
  readable, go call `recv`".
  That forces `zap_netpoll_wait` to carry a completion result (bytes
  transferred, buffer ownership), i.e. **widening the scheduler-facing ABI** and
  diverging the A4 socket layer into a wholly different, completion-shaped code
  path per platform.
  It also mismatches the only other readiness consumer pattern (the self-pipe
  shape I#77 uses on Linux).
  This is the "seam widened (affects Linux)" outcome we were asked to avoid.
- **AFD-readiness** keeps the ABI and the A4 socket code single-shape across
  platforms, confines all Windows specifics behind `np_backend_*` +
  `np_poller_main`, and is proven in production by wepoll/libuv/mio/tokio.
  Its cost is the one-shot re-arm and the `ntdll`/`AFD_POLL_INFO` surface.

**Recommendation: AFD-readiness shim.**
It is the only option that satisfies the hard constraint "keep the Linux epoll
path byte-for-byte unchanged and do not widen the seam in a way that affects
Linux", while still being real IOCP (the issue title's requirement) and scalable
(no 64-handle `WaitForMultipleObjects` ceiling).

### Rejected: `WSAPoll` / `select` / `WSAEventSelect` + `WaitForMultipleObjects`

Readiness-shaped and simpler, but not IOCP, and capped at
`MAXIMUM_WAIT_OBJECTS` (64) handles per poller thread — a non-starter for a
server runtime and explicitly not what the issue asks for.

## Compatibility

- **Linux/macOS: zero diff.**
  The scheduler-facing ABI, the generic poller core, and the entire epoll arm
  are untouched.
  Every Windows addition is under `#if defined(ZAP_PLATFORM_WINDOWS)` /
  `ZAP_NETPOLL_IOCP`; the Windows-only `PollDesc` fields are `#ifdef`-guarded and
  do not alter the Linux struct layout.
  Proof: the I#80 `netpoller.zap` golden and the `__netpoll_selftest()` builtin
  run **unchanged** on the required Linux `host`/`full` and `arm64` jobs under
  **both** collectors; no golden edited, no exact-heap-byte assertion added
  (standing rule).
- **Windows:** the real IOCP backend links into the program build (dead until
  A4 lifts the TU's remaining POSIX compile walls) and is verified standalone on
  a real `windows-latest` runner (MinGW + boehm) by a new required CI job.
- **Language semantics: none.** No grammar, ZIR, or ABI change; no new language
  surface (sockets are A4).

## Implementation

Follows the A1/A3 precedent (`design/0017`, `design/0019`): ship the real
backend behind the seam (dead until the TU compiles) + a standalone
`windows-latest` harness + a **required** CI job verifying on a real runner.

1. **zaprt.c, netpoller section only.**
   - Add `ZAP_NETPOLL_IOCP` arm to the `np_backend_*` seam and a Windows
     `np_poller_main` (sibling to the epoll arm), as sketched above.
   - Add `#if defined(ZAP_PLATFORM_WINDOWS)` fields to `PollDesc`
     (per-direction `OVERLAPPED`, `AFD_POLL_INFO` buffer, base handle).
   - Reuse `np_wake_dir_locked` / `np_compute_timeout` / `np_expire_deadlines` /
     registry / ABI verbatim.
   - The POSIX (epoll + unsupported-stub) arms stay byte-for-byte unchanged.
2. **A new standalone Windows harness** `crates/codegen/src/zap_np_iocp_test.c`
   (A1/A3 precedent), self-contained enough to compile on windows-latest, that:
   - creates a loopback **socket pair** via raw `WSASocket` + `bind`/`listen`/
     `connect`/`accept` (not the Zap `net` module — A4 is out of scope),
   - registers the read end, parks a reader (single-M discipline so "a parked
     coroutine frees its M" is load-bearing), writes from the peer, and asserts
     the AFD completion wakes the reader with `ZAP_POLL_OK`;
   - asserts a deadline-only wait returns `ZAP_POLL_TIMEOUT`;
   - asserts `PostQueuedCompletionStatus` (`np_backend_kick` /
     `zap_netpoll_wake`) interrupts/latches correctly.
3. **lib.rs:** embed the harness via `include_str!` and add the IOCP backend to
   the Windows link arm of `build_executable` (drop-in; dead until zaprt.c
   compiles on Windows). Non-Windows link byte-identical.
4. **CI:** new **required** job `windows-netpoller` (windows-latest, MSYS2/MinGW
   + `mingw-w64-x86_64-gc`) builds+runs the harness on both `push` and
   `pull_request` triggers; the broader `windows` program-link job stays
   non-blocking until A4.
5. **Narrow the zaprt.c `#error` gate:** mark A2 **DONE**; remaining blockers A4
   (#198: fs/process/sockets/getentropy POSIX includes) + immix `VirtualAlloc`
   (I#218). Document the exact remaining A4 compile gap in the PR body.
6. **Gates:** `cargo fmt --check`; clippy gc-immix + gc-boehm (`-D warnings`);
   `make docker-test` under **both** collectors; required Linux/ARM
   `pull_request` + `push` green; the new Windows job green.

## Open questions (for orchestrator review before implementation)

1. **AFD one-shot re-arm vs. the edge-latch model (main nuance).**
   `IOCTL_AFD_POLL` is one-shot; the poller re-arms after each completion.
   AFD readiness is level-ish, so re-arming a *read* poll while the socket is
   still readable (waiter latched but not yet drained) completes immediately and
   could busy-spin until the consumer drains to `WSAEWOULDBLOCK`.
   Plan: arm a direction's AFD poll only while that direction has interest and no
   outstanding latch — i.e. gate re-arm on `pd->rg || !pd->r_ready` per
   direction — so a latched-but-undrained socket is not re-polled.
   This stays inside the Windows arm (no core change). **Confirm this is the
   right discipline, or prefer always-armed + accept one redundant wake.**
2. **Base-handle unwrap.**
   AFD poll must target the socket's **base** handle (`WSAIoctl`
   `SIO_BASE_HANDLE`, with `SIO_BSP_HANDLE_POLL` as the fallback some LSPs need).
   For the harness's raw `WSASocket` the base handle is the socket itself, so the
   harness proves the mechanism even if exotic LSP layering is a theoretical A4
   edge. **OK to defer LSP-fallback hardening to A4's real socket integration?**
3. **`PollDesc` fd type.**
   The ABI is `int64_t fd`; a Windows `SOCKET` is a pointer-width handle that
   fits in `int64_t`.
   Plan: keep the ABI `int64_t` and cast to `SOCKET` in the Windows backend only
   (no ABI change). **Confirm — vs. introducing a typedef'd handle, which would
   touch the shared signature.**
4. **Scope confirmation.**
   A2 implements the backend + harness + CI only.
   No A4 socket module, no Zap language I/O surface, no immix port.
   The `#error` gate goes to "A4 remains". **Confirm this is the intended A2
   boundary.**
