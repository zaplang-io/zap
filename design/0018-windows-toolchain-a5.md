# Proposal: Windows build/link toolchain + CI — I#199 (A5)

- **Author(s):** win-toolchain session (I#199)
- **Last updated:** 2026-10-22
- **Status:** Draft (implementation — the toolchain half of A5)
- **Discussion:** https://github.com/parveshgarg/zap/issues/199
- **Epic:** I#158 (platform/1.0), Area A5. See `design/0016-platform-1.0.md`
  and the A1 spike `design/0017-windows-coroutine-context-switch.md`.

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

A5 is the Windows build/link toolchain and CI.
Its toolchain half can land before the runtime port (A1 #195, A3 #197), and this
change lands exactly that: the driver link step is now **target-aware** and emits
the MinGW-w64 link line on a Windows target, the C runtime gains the **A1
context-switch seam** (`zap_ctx_swap`/`zap_ctx_make`) plus a platform split that
isolates the POSIX-only pieces, and a **non-blocking `windows-latest` CI job**
reports progress without blocking merges.
Crucially this is a **no-op refactor on Linux/macOS** — the seam is the existing
`ucontext` behind a thin wrapper, proven by the full gate suite under both
collectors.
A Zap *program* does not link on Windows yet: the runtime still needs A1, A3 and
A2/A4.
Those gaps are now **compile-visible and documented** (an authoritative `#error`
gate in `zaprt.c`) rather than silently broken, and the seam + link line are the
exact drop-in points for #195 / #197.

## Background — what A5 inherited

`design/0016-platform-1.0.md` (Area A) and `design/0017-...` survey the two
platform-specific surfaces: the C runtime `crates/codegen/src/zaprt.c` (deeply
POSIX — `ucontext`, `pthread`, `SIGURG`/`sigaction`, `epoll`/`eventfd`,
`getentropy`, `backtrace`) and the driver link step `crates/codegen/src/lib.rs`
(host-triple-only, `cc` + POSIX libs + `-rdynamic` + `pkg-config` CPython).
The A1 spike recommended **MinGW-w64 first** (minimises the Itanium→SEH exception
change vs MSVC) and **A5-toolchain first, then A1 switch, then A3/A5 unwinder**.

## Proposal — what this change does

### 1. Target-aware driver link step (`crates/codegen/src/lib.rs`)

`build_executable` now computes the target triple and branches on it.
Because codegen still emits for the host triple only, "Windows target" means
"the Zap compiler running on a Windows (MinGW-w64) host"; the branch is on the
**triple string**, so the Windows arm is still type-checked by the Linux/macOS
build (the `full` CI job compiles it even though it never takes it).

- **Linker driver:** `cc` by default, overridable with `$ZAP_CC` (e.g.
  `x86_64-w64-mingw32-gcc`). Useful on every platform; added for Windows.
- **Common libs (both platforms):** `-lstdc++ -lm -lpcre2-8 -lz -lssl -lcrypto
  -lpthread` and the collector selector (`-DZAP_GC_IMMIX` or `-lgc`). MinGW-w64
  provides all of these under the same `-l` names (MSYS2
  `mingw-w64-x86_64-{pcre2,zlib,openssl,boehm-gc}`, `winpthreads`, `libstdc++`).
- **POSIX-only extras:** `-rdynamic` (ELF dynsym; no PE/COFF equivalent) and the
  `pkg-config` CPython embed flags are emitted **only off Windows**.
- **Windows-only extras:** `-lws2_32` (Winsock, for the A2/#196 IOCP netpoller),
  `-lbcrypt` (CNG `BCryptGenRandom`, the Windows `getentropy` replacement, A4),
  `-ldbghelp` (`StackWalk64` for the A3/#197 fault backtrace). These are wired
  **ahead** of the runtime that uses them; the linker ignores an unreferenced
  library, so listing them now is harmless and A2/A3 need not touch the link
  line.

### 2. Runtime platform split + A1 context-switch seam (`zaprt.c`)

- **Platform detection** (top of file): `_WIN32` → `ZAP_PLATFORM_WINDOWS`, else
  `ZAP_PLATFORM_POSIX`. `<ucontext.h>` is now included only under POSIX.
- **A1 seam** (just before the scheduler): the scheduler no longer calls
  `ucontext` directly. It goes through
  ```c
  typedef /* ucontext_t (POSIX) | struct{void*sp;} (Windows) */ zap_ctx_t;
  void zap_ctx_make(zap_ctx_t *c, void *stack, size_t n, void (*entry)(void));
  void zap_ctx_swap(zap_ctx_t *from, zap_ctx_t *to);
  ```
  On POSIX these wrap `getcontext`+`makecontext` and `swapcontext` verbatim — a
  **zero-behaviour-change** refactor. The `G.ctx` / `M.sched_ctx` fields became
  `zap_ctx_t`; the five call sites (`switch_to_g`, `g_bootstrap`, `galloc`, and
  the three yield points) now call the seam. The GC invariant is unchanged: a
  `zap_ctx_t` is still stored inside the GC-allocated G and scanned as heap under
  Boehm; the immix shadow-head save/restore and `set_bottom` bracketing are
  untouched.
  On Windows the seam is declared (so #195's asm translation unit drops in) with
  the single-saved-SP `zap_ctx_t` the spike recommends.
- **Authoritative Windows gap gate:** when `ZAP_PLATFORM_WINDOWS`, `zaprt.c`
  fails the compile with an `#error` that enumerates every remaining gap (A1/A2/
  A3/A4) and its ticket. This is the "compile-visible, not silently broken"
  requirement: the gaps are listed at one honest location rather than discovered
  as a cascade of missing symbols, and no half-working runtime can slip through.

  `zaprt.c` is compiled by `cc`/MinGW only at **program-link** time (it is
  `include_str!`'d, not compiled by `cargo build`), so this gate does **not**
  affect the compiler build or the Windows CI job below; it bites only a
  developer who tries to link a Zap program on Windows — exactly the A1/A3 gap.

### 3. Non-blocking `windows-latest` CI job (`.github/workflows/ci.yml`)

A `windows` job, `continue-on-error: true`, named
**"Windows (in progress, A1/A3 pending)"**.
It builds and tests the **default members** (the portable front-end crates, no
LLVM, no C-runtime link) on a native Windows host and prints the MinGW-w64
`gcc` version.
The Linux `host`/`full` and `arm64` jobs are **unchanged and still required**.
Promote this to a required full-codegen+link job once A1/A3 land.

## Compatibility

- **Linux/macOS: none.** The seam is `ucontext` behind a wrapper; the link line
  off Windows is byte-for-byte what it was. Proven by `cargo fmt`, clippy under
  both `gc-immix` and `gc-boehm`, and `make docker-test` under both collectors
  (the golden end-to-end + coroutine/GC tests exercise the seam).
- **Windows:** a Zap program still cannot link (runtime port incomplete). This
  change makes that state explicit and wires the toolchain; it does not regress
  anything because nothing linked on Windows before.
- **Goldens:** no exact-heap-byte assertions touched.

## Implementation / what drops in next

**For #195 (A1) — the real context switch:**
1. Add `crates/codegen/src/ctx_win_x64.S` (or `.c` with the inline asm from
   `design/0017-win-ctxswitch-spike/ctx_win_x64.c`) implementing
   `zap_ctx_swap`/`zap_ctx_make` for Windows; add it to the files written +
   compiled in `build_executable` on a Windows target. The seam signatures and
   `zap_ctx_t` shape are already in place.
2. Optionally swap the POSIX seam to the System V asm backend for the
   per-switch `sigprocmask` win (spike step 5) — the seam makes this local.
3. Remove / narrow the `zaprt.c` Windows `#error` as each subsystem is ported.

**For #197 (A3) + the A5 unwinder slice — exceptions on Windows:**
1. Make codegen target-conditional: on Windows do not force DWARF `.eh_frame`
   and set the Windows C++ personality (spike §"Exceptions / unwinding scope").
2. Re-express the `__cxa_*` EH glue as portable C++ `throw`/`catch` of a
   `ZapException` so it works on MinGW and MSVC.
3. Port preemption (`SuspendThread`/`QueueUserAPC`), faults (vectored/SEH +
   `dbghelp`, `-ldbghelp` already on the link line), and console control
   handlers; map the `Signal` prelude enum.

**For #196 (A2):** implement the `np_backend_*` seam on IOCP (`-lws2_32` already
on the link line); it already returns "unsupported" on non-Linux, so it is not a
compile wall.

**For #198 (A4):** `getentropy` → `BCryptGenRandom` (`-lbcrypt` already linked),
path/fs/process portability, and the CPython-on-Windows decision (gated off in
the link step for v1).

## Open issues

- **MinGW-w64 runtime-lib sourcing on CI.** The Windows CI job does not yet
  install the MSYS2 `mingw-w64-x86_64-*` runtime libraries, because `zaprt.c`
  does not compile on Windows yet (A1/A3), so there is nothing to link them
  into. The A1/A3 sessions should add that install step when they promote the
  job to compile the runtime.
- **MSVC as a second target.** MinGW-w64 is first (per the spike) to minimise the
  exception-ABI change; MSVC (`clang-cl`/`link.exe`) is the longer-term native
  story and is out of this ticket's scope.
- **CPython on Windows** remains deferred (A4); `import python` is unsupported on
  Windows v1.
