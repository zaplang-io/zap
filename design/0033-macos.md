# Proposal: macOS platform support — I#224 (platform/1.0 Area, macOS gap)

- **Author(s):** macos-224 session (I#224)
- **Last updated:** 2026-10-25
- **Status:** Draft
- **Discussion:** https://github.com/zaplang-io/zap/issues/224
- **Epic:** I#158 (platform/1.0). See `design/0016-platform-1.0.md`,
  `design/0017-windows-coroutine-context-switch.md` (the A1 ctx-switch seam).

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

macOS is the last 1.0-target platform (Linux + macOS + Windows) still missing.
Unlike Windows — which needed a hand-asm context switch, an IOCP completion-model
netpoller, an SEH unwinder and a path/process port across five XL/L sub-tickets —
macOS is POSIX and uses the **Itanium C++ ABI like Linux**, so it is far closer to
Linux than to Windows.
A direct compile of `crates/codegen/src/zaprt.c` on macOS (this investigation,
x86_64, Apple clang 17) shows the runtime already builds under the **precise Immix
collector** with exactly **one** blocker — `<ucontext.h>` `#error`s unless
`_XOPEN_SOURCE` is defined — and a handful of smaller deltas.
The one genuine piece of new runtime mechanism is a **kqueue netpoller backend**
behind the existing `np_backend_*` seam (parallel to epoll/IOCP), which net, http
*and* the signal subsystem all depend on.
There are **no load-bearing surprises**: ucontext works behind the A1 seam (per
the spike's recommendation to keep it there for the first cut), the Immix heap is
`posix_memalign`-based (no `mmap`/`MAP_ANON` port), the x86_64 memory model
sidesteps the parked aarch64 GC bug (I#223), and x86_64 JIT needs no `MAP_JIT`.
This doc records the works-vs-missing audit, the per-area decisions, and the
single-PR scope.

## Background — what is platform-specific, and where macOS already lands

The front-end crates are portable Rust.
Everything OS-specific is the C runtime (`zaprt.c` ~12.2k lines, `zapgc.inc.c`
~1.6k) and the driver link step (`crates/codegen/src/lib.rs`).
macOS compiles as `ZAP_PLATFORM_POSIX` (the `#else` of the `_WIN32` check), so it
takes the POSIX branches throughout — the same ones Linux takes — not the
`ZAP_PLATFORM_WINDOWS` seams.

Audit (verified by compiling `zaprt.c` with `-DZAP_GC_IMMIX` on macOS x86_64):

- **Context switch — WORKS behind the A1 seam, with a feature-macro fix.** The
  `zap_ctx_swap`/`zap_ctx_make` seam (I#195, design/0017) already wraps ucontext
  on POSIX; macOS is POSIX, so it uses the same `ucontext_t` path as Linux. The
  only issue: Apple's `<ucontext.h>` `#error`s ("deprecated ucontext routines
  require `_XOPEN_SOURCE`") unless the macro is defined, and then emits
  deprecation *warnings* (not errors) for `get/make/swapcontext`. Defining
  `_XOPEN_SOURCE` (paired with `_DARWIN_C_SOURCE` to keep BSD extensions —
  `arc4random`, `_NSGetArgv`, the kqueue `EV_*` constants — visible) compiles
  clean; a localized `-Wdeprecated-declarations` pragma around the three seam
  calls keeps it warning-free under `-Wall`. ucontext is deprecated-but-fully-
  functional on macOS; an asm backend is a strictly-optional perf follow-up
  (same status as Linux), not required now.
- **Immix GC — WORKS as-is.** `zapgc.inc.c` allocates blocks with
  `posix_memalign` and the registry/metadata with `calloc`/`malloc` — **no `mmap`,
  no `MAP_ANON`, no `madvise`**. (The `#error` text in `zaprt.c:33` claiming
  "maps heap with mmap" is stale narrative in a *Windows*-only guard and does not
  reflect the code; macOS never trips that guard anyway.) The shadow-stack /
  safepoint machinery is pure C and pthreads. `posix_memalign` and `calloc` are
  identical on macOS. Nothing macOS-specific is needed for the precise collector.
- **Boehm GC — WORKS via the same `-lgc`.** `bdw-gc` is a Homebrew formula
  (`brew install bdw-gc`, built thread-aware); the runtime links `-lgc` exactly as
  on Linux. No source change.
- **Threads / sync — WORKS.** `pthread`, `GC_pthread_create`,
  mutex/cond/once/sigmask are all libSystem on macOS.
- **Preemption / signals / faults — WORKS.** `SIGURG` + `sigaction` +
  `pthread_kill` + `sigaltstack` are all POSIX on macOS. The `Signal` enum's Unix
  numbers are valid. One nicety: `ZAP_HAVE_BACKTRACE` is gated on `__GLIBC__`
  today, but macOS *has* `<execinfo.h>` `backtrace`/`backtrace_symbols_fd` in
  libSystem — enabling it on `__APPLE__` gives fault reports and the CPU profiler
  (I#78) real symbols instead of a no-op.
- **Netpoller — MISSING (the one new backend).** `np_backend_*` has epoll (Linux)
  and IOCP (Windows); macOS currently falls to the `#else` "unsupported" arm, so
  `zap_netpoll_open` returns NULL. Because the signal subsystem (I#77) parks its
  self-pipe *on the netpoller*, and net/http (I#104) do too, a NULL netpoller
  means signals, sockets and http are all dead on macOS. A **kqueue** backend is
  required for a green suite.
- **Misc syscalls — WORK.** `clock_gettime(CLOCK_MONOTONIC/REALTIME)`,
  `sysconf(_SC_NPROCESSORS_ONLN)` (M count / `MAXPROCS`), `fcntl(O_NONBLOCK)`,
  `posix_spawnp`, `waitpid` are all POSIX on macOS. Entropy already branches to
  `arc4random_buf` under `__APPLE__` (I#105). `sys.platform()` already returns
  `"darwin"`, and argv capture already uses `_NSGetArgc/_NSGetArgv`. (These
  `__APPLE__` guards were added defensively by earlier stdlib work; this ticket
  makes the rest of the runtime match.)

### The driver link step

`build_executable` (lib.rs) shells `cc` with `-lstdc++ -lm -lpcre2-8 -lz -lssl
-lcrypto -lpthread -rdynamic` + `pkg-config` CPython embed flags.
On macOS:

- **`-lstdc++` is wrong.** macOS ships **libc++**, not libstdc++; modern SDKs
  (and all Apple-Silicon SDKs) have no `libstdc++.dylib`. The Itanium EH glue the
  runtime needs (`__gxx_personality_v0`, `__cxa_allocate_exception`, `__cxa_throw`,
  `__cxa_begin_catch`, `_Unwind_*`) is provided by **libc++abi**, linked via
  **`-lc++`**. Verified: `-lc++` resolves all four `__cxa*`/`__gxx*` symbols on
  macOS. The fix is `-lstdc++` → `-lc++` on `target_os = "macos"`.
- **`-rdynamic` WORKS** (Apple clang maps it to `-Wl,-export_dynamic`).
- **`-lpthread`, `-lz` WORK** (libSystem / system zlib).
- **`-lpcre2-8`, `-lssl -lcrypto`** come from Homebrew; on an Intel host their
  `-L/usr/local/lib` is already on the default search path, and on Apple Silicon
  `/opt/homebrew/lib`. The CI job adds the Homebrew lib/include dirs so the names
  resolve regardless of prefix.
- **CPython embed** via `pkg-config python3-embed` works (Homebrew `python@3`
  ships `python3-embed.pc`), so `import python` stays enabled on macOS (unlike the
  Windows v1 gate).

## Proposal — decisions by area

### Context switch: ucontext behind the seam (asm deferred)

Keep `ucontext` as the POSIX `zap_ctx_*` backend on macOS, matching Linux and the
A1 spike's explicit first-cut recommendation.
Add, at the very top of `zaprt.c` (before any system header, since feature-test
macros must precede them):

```c
#if defined(__APPLE__)
#define _XOPEN_SOURCE 700   /* un-gate <ucontext.h> (deprecated-but-functional)  */
#define _DARWIN_C_SOURCE 1  /* keep BSD extensions: arc4random, kqueue, _NSGet*  */
#endif
```

and wrap the three ucontext calls in the POSIX seam with a localized
`#pragma clang diagnostic ignored "-Wdeprecated-declarations"` (push/pop, under
`__APPLE__`) so `-Wall` stays clean.
Linux is byte-identical (the macros and pragma are `__APPLE__`-only).
A macOS/BSD asm `zap_ctx_*` backend is a perf follow-up, tracked, not in scope.

### Netpoller: a kqueue backend behind `np_backend_*`

Add a third arm to the backend seam, `#elif defined(__APPLE__)` (extensible to
`__FreeBSD__`/BSD), defining `ZAP_NETPOLL_KQUEUE` and the four seam functions,
mirroring the epoll arm's shape:

- **open:** `kqueue()`; store the fd. Wake mechanism = **`EVFILT_USER`** (an
  identified user event with `EV_CLEAR`), the native macOS analogue of Linux's
  `eventfd` — no self-pipe fd needed. `np_backend_kick` does
  `kevent(kq, &trigger_user_event, ...)` with `NOTE_TRIGGER`.
- **add:** register **two** changelist entries for the fd — `EVFILT_READ` and
  `EVFILT_WRITE` — with `EV_ADD | EV_CLEAR` (edge-triggered, matching the epoll
  `EPOLLET` contract and the "drain to EAGAIN" rule the callers already follow).
  `udata` = the `PollDesc *` (kqueue's per-filter `udata`, the `data.ptr`
  analogue).
- **del:** `EV_DELETE` both filters (ignore `ENOENT`, as when the fd was never
  readable/writable-registered or already closed).
- **poller loop (`ZAP_NETPOLL_KQUEUE` arm of `np_poller_main`):** `kevent()` with
  a `timespec` computed from `np_compute_timeout()` (ms → `timespec`, NULL for
  infinite); for each returned event, dispatch by `filter`: `EVFILT_READ` →
  `np_wake_dir_locked(pd, 1, OK)`, `EVFILT_WRITE` → `(pd, 0, OK)`. `EV_EOF` (peer
  hangup / error, kqueue sets it on both filters) counts as readiness for that
  direction, the kqueue analogue of epoll's `EPOLLHUP|EPOLLERR` → wake-both so the
  parked G observes the condition on its next syscall. The `EVFILT_USER` wake
  event is recognized by `filter == EVFILT_USER` and simply drained. Then
  `np_expire_deadlines()`.
- The poller thread is a **non-mutator** exactly like the epoll/IOCP threads
  (allocates no GC memory, touches only mutex-guarded scheduler fields, started
  with `GC_pthread_create` so Boehm still registers it), so STW / preemption leave
  it blocked in `kevent` with no special handling.
- `PollDesc` needs **no new fields** (unlike IOCP's `sock`/`afd`): kqueue is a
  readiness model over the same `int fd`, so the Linux struct layout is reused
  verbatim — the `#if defined(ZAP_PLATFORM_WINDOWS)` extra-fields block stays
  Windows-only.
- The existing in-runtime `zap_netpoll_selftest()` (pipe park/wake + deadline,
  driven through the real scheduler + channels on a single M) is generalized from
  `#if defined(ZAP_NETPOLL_EPOLL)` to also cover `ZAP_NETPOLL_KQUEUE` — it is pure
  POSIX pipe code — giving macOS the same self-test the `netpoller` golden runs,
  plus a standalone C harness for the CI job (mirroring the Windows A2 pattern).

### GC (both collectors): no macOS-specific code

Immix: `posix_memalign`/`calloc`, portable — no `mmap`/`MAP_ANON` work.
Boehm: `-lgc` from Homebrew's thread-aware `bdw-gc`.
Shadow-stack/safepoint and I#120 evacuation are collector logic, arch/OS-neutral.
No thread-registration delta (bdwgc supports macOS pthreads natively).

### Signals / preemption / unwinder

`SIGURG` preemption, the self-pipe signal thread, `sigaltstack` fault handler and
the Itanium unwinder all use POSIX/Itanium primitives present on macOS.
One change: enable `ZAP_HAVE_BACKTRACE` on `__APPLE__` (execinfo is in libSystem),
so fault crash reports and the CPU profiler symbolicate.
The unwinder is **closer to Linux than Windows**: macOS uses the Itanium C++ ABI,
`__gxx_personality_v0` + `__cxa_*` resolve from libc++abi (`-lc++`), and codegen's
forced DWARF `.eh_frame`/CFI is correct on macOS — **no personality or
table-model change** (the Windows SEH swap does not apply).

### CI + arch — x86_64 required, arm64 best-effort (I#223 does NOT block)

GitHub's `macos-latest` runners are arm64 (Apple Silicon), and I#223 is a parked
**aarch64-only** weak-memory bug in the precise-Immix multicore path (the finding
proves x86_64 is clean: 0 failures over 120 runs at MAXPROCS 4/8/16; arm64 fails
~97% even with evacuation disabled).
Decision, stated plainly so #223 cannot block macOS:

1. **The REQUIRED macOS job runs on `macos-13` (Intel x86_64)** — the full
   workspace suite under **both** collectors, exactly like `full`. x86_64 shares
   Linux's memory model, so it is unaffected by #223 and is expected green. This
   is the honest, load-bearing macOS gate.
2. **An arm64 macOS job (`macos-14`) is added `continue-on-error: true`
   (best-effort)**, running the suite with `MAXPROCS=1` for the GC-stress goldens
   so the known-good single-worker path is exercised on Apple Silicon while the
   #223 multicore race is quarantined. It surfaces any *non-#223* arm64-macOS
   regression without gating the merge. Promoting it to required is a follow-up
   that lands with the I#223 fix.

Rationale for pinning the required job to Intel: it gives a genuinely green,
multicore-GC-exercising macOS gate today, and does not pretend the parked arm64 GC
bug is fixed. If GitHub retires the `macos-13` Intel image, the fallback is to
make the arm64 job required but `MAXPROCS=1`-pinned for the multicore-GC goldens
until #223 lands (documented in the job comment).

## Rationale

- **Why not an asm context switch now?** ucontext is functional on macOS behind
  the A1 seam; the spike recommends keeping it for the first cut. Asm is a perf
  win (no `sigprocmask` per switch) but pure follow-up, and the same deferral
  already stands for Linux.
- **Why kqueue `EVFILT_USER` over a self-pipe?** It needs no extra fd, is the
  idiomatic kqueue wake, and keeps the `PollDesc` struct byte-identical to Linux.
  A self-pipe would also work; `EVFILT_USER` is cleaner.
- **Why `-lc++` not keep `-lstdc++`?** libstdc++ is absent on modern/Apple-Silicon
  macOS; libc++abi provides the identical Itanium symbols. Verified on-host.
- **Why Intel-required / arm64-best-effort** rather than arm64-required with
  quarantine? See the CI section — it is the only split that is both green today
  and honest about I#223. D-0082.

## Compatibility

- **Language semantics: none.** Pure runtime + build + CI.
- **Linux/Windows: byte-identical.** Every change is under `#if defined(__APPLE__)`
  (or `target_os = "macos"` in lib.rs) or a new `#elif` arm that other platforms
  never take. The epoll and IOCP arms are untouched.
- **GC:** both collectors keep their root models; no golden asserts exact heap
  bytes (standing rule), and the switch is behaviorally transparent.
- **Grammar/EBNF:** no change (macOS support is not a language surface).

## Implementation

Single PR, "Closes #224" (runtime + CI are small and interlocked; splitting would
leave a non-building intermediate):

1. `zaprt.c`: `__APPLE__` feature macros at top; ucontext deprecation pragma
   around the seam; `ZAP_HAVE_BACKTRACE` on `__APPLE__`; the **kqueue**
   `np_backend_*` arm + the `ZAP_NETPOLL_KQUEUE` `np_poller_main` + generalize the
   selftest gate.
2. `lib.rs`: `-lstdc++` → `-lc++` on macOS; add Homebrew `-L`/`-I` only if needed
   (CI sets them via env).
3. A standalone kqueue netpoller C harness (`zap_np_kqueue_test.c`) paralleling
   `zap_np_iocp_test.c`, for a cheap required check even on a no-LLVM path.
4. `.github/workflows/ci.yml`: `macos` (macos-13, x86_64, both collectors,
   required) + `macos-arm64` (macos-14, best-effort, MAXPROCS-pinned goldens).
5. `DECISIONS.md`: D-0082 (the CI arch split).

Gates (standing): `cargo fmt`; clippy both collectors `--all-targets -D warnings`;
`make docker-test` both collectors (Linux gate); `zap fmt --check tests/golden`;
the new macOS job green; both CI triggers (push + pull_request) green at run level.

Follow-ups (not in this PR): macOS/BSD asm `zap_ctx_*` backend; promoting the
arm64 macOS job to required once I#223 lands; FreeBSD/OpenBSD (the kqueue arm is
written to extend to them).

## Open issues

- **`macos-13` longevity.** GitHub has signalled Intel-runner retirement; if it is
  gone, the required job moves to arm64 with `MAXPROCS=1`-pinned multicore-GC
  goldens until I#223 is fixed (see CI section). The split is chosen so this swap
  is a one-line runner change, not a redesign.
- **Apple Silicon JIT (REPL I#159).** x86_64 macOS JIT needs no `MAP_JIT`; LLVM
  ORC handles Apple-Silicon W^X/`MAP_JIT` internally (LLVM 15+). The required
  Intel job proves the REPL path; the best-effort arm64 job will flag any
  Apple-Silicon JIT issue without blocking.
