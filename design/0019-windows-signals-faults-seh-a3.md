# Proposal: Windows signals / preemption / faults + Itanium→SEH exception port — I#197 (A3)

- **Author(s):** win-signals-seh session (I#197)
- **Last updated:** 2026-10-22
- **Status:** Draft (design — pending orchestrator sign-off before implementation)
- **Discussion:** https://github.com/parveshgarg/zap/issues/197
- **Epic:** I#158 (platform/1.0), Area A3. See `design/0016-platform-1.0.md`,
  the A1 spike `design/0017-windows-coroutine-context-switch.md`, and the A5
  toolchain `design/0018-windows-toolchain-a5.md`.

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

A3 is the last big Windows runtime blocker: it makes full Zap programs — with
`throw`/`catch`, async preemption, and fault diagnostics — run on Windows.
Three POSIX subsystems have no Windows equivalent and must be ported behind the
existing platform seam (`ZAP_PLATFORM_WINDOWS`/`ZAP_PLATFORM_POSIX`, I#199):

1. **Exceptions / unwinding** — codegen forces the *Itanium* personality
   (`__gxx_personality_v0`); Windows x64 is table-based SEH.
2. **Async preemption** (I#172) — `SIGURG` + `sigaction` + `pthread_kill`.
3. **Faults + diagnostics** (I#77) — `sigaction` + `sigaltstack` + a self-pipe
   parked on the netpoller + `backtrace_symbols_fd`.

This doc makes the two **load-bearing, cross-platform-blast-radius** decisions
the orchestrator asked to settle *before* implementation:

> **Exceptions:** target the **MinGW-w64 SEH model**, which keeps LLVM's
> `landingpad` IR, the `__cxa_*` runtime glue, and `-lstdc++` **unchanged**.
> The *only* change is making the personality-function **name**
> target-conditional (`__gxx_personality_v0` on POSIX, `__gxx_personality_seh0`
> on `*-windows-gnu`). Linux/macOS codegen and runtime are **byte-for-byte
> unchanged** — the strongest possible form of the "no regression" constraint.

> **Preemption:** reuse I#172's design verbatim. Under **immix** (precise) the
> handler only sets `m->preempt`; the yield already happens cooperatively at
> `zap_safepoint()`, so Windows immix preemption is a **plain flag store from
> the monitor — no thread suspension at all**. Only **boehm** (conservative)
> needs the async switch; its Windows analogue is a monitor-driven
> `SuspendThread` + `GetThreadContext`/`SetThreadContext` **hijack**, guarded
> identically to the Unix `copreempt` handler.

It also **reports two clean-boundary dependencies** rather than pulling their
scope in (per the orchestrator's instruction): the #77 signal *dispatch* is
built on the netpoller (IOCP = A2/#196), and **immix does not compile on
Windows** until its `mmap`→`VirtualAlloc` port lands (collector-owned, not A3).

## Background — what exists today (grounded in the tree)

### Exceptions (I#22/I#40, I#47 spawn landing pad)

- **Codegen** (`crates/codegen/src/lib.rs`):
  - `:528` declares `__gxx_personality_v0` as a variadic module function and
    stores it under that key in `self.fns`.
  - `:1230` sets `uwtable(2)` (asynchronous unwind tables) on **every** function;
    `:1243` sets the personality on any function with a `LandingPad`.
  - `:2122` (user `try`/`except` landing pad) and `:2644`/`:2676` (the I#47 spawn
    thunk's base landing pad) both emit `landingpad {ptr,i32} catch ptr null`
    (a single **catch-all** clause) and name the same personality.
- **Runtime glue** (`crates/codegen/src/zaprt.c:1210-1280`): `zap_panic` boxes a
  pointer in `__cxa_allocate_exception` and `__cxa_throw`s it with a hand-built
  `std::type_info` whose vtable is libstdc++'s
  `_ZTVN10__cxxabiv117__class_type_infoE[2]`; `zap_eh_begin_catch` recovers the
  object via `__cxa_begin_catch`; `zap_eh_end_catch` balances it.
- **Link** (`lib.rs:164`): `-lstdc++` on every platform (the unwinder is libgcc;
  the personality + `__cxa_*` are libstdc++). `zaprt.c` is compiled **as C** by
  `cc`/MinGW at program-link time (it is `include_str!`'d, not built by cargo).

**The crucial observation.** Of all of the above, exactly **one token** is
Itanium/DWARF-specific: the *name* `__gxx_personality_v0`.

- `uwtable(2)` is **target-agnostic**: the LLVM backend emits DWARF `.eh_frame`
  on ELF and SEH `.pdata`/`.xdata` on COFF from the same attribute. (The spike's
  phrase "force DWARF `.eh_frame`" is imprecise — nothing in codegen forces a
  DWARF *section*; it requests unwind tables and the backend picks the format.)
- The `landingpad` + `catch ptr null` representation is **also what Clang itself
  emits** when it compiles C++ `try`/`catch` for `x86_64-w64-windows-gnu` — MinGW
  x64 uses the *SEH model* but keeps the Itanium-style `landingpad` IR (LLVM
  classifies `__gxx_personality_seh0` as `EHPersonality::GNU_CXX`, which is the
  non-funclet, `landingpad`-based lowering). It is **MSVC** (`__CxxFrameHandler3`,
  `_CxxThrowException`) that needs funclet IR (`catchpad`/`cleanuppad`) — a much
  larger codegen change, and explicitly out of scope (spike: MinGW-w64 first).
- MinGW-w64's libstdc++ provides `__cxa_allocate_exception`/`__cxa_throw`/
  `__cxa_begin_catch`/`__cxa_end_catch`, the `__cxxabiv1::__class_type_info`
  vtable symbol, **and** `__gxx_personality_seh0` — the Itanium C++ *library* ABI
  is identical across the two unwind models; only the personality differs.

### Preemption (I#172)

`zaprt.c:2537-2610`. A monitor OS thread samples each worker M's `schedtick`
every ~10 ms; an unchanged tick means the same coroutine held the M for the
slice. The monitor **sets `m->preempt = 1`** and then `pthread_kill`s `SIGURG`
(`:2602-2603`). The `SIGURG` handler (`zap_preempt_handler`, `:2539`) splits by
collector:

- **immix (default):** the handler body under `ZAP_GC_IMMIX` is *empty* beyond
  `m->preempt = 1` — it just records the request; the actual yield happens at the
  next `zap_safepoint()` (`zapgc.inc.c:438`), which checks `m->preempt` and calls
  `copreempt()` where the shadow chain is consistent (GC-safe). Safepoints are
  emitted at fn entry and loop back-edges, so a tight loop reaches one within an
  iteration.
- **boehm (fallback):** no cooperative safepoint exists, so the handler
  **direct-switches** (`copreempt()`) at the interrupted PC — guarded by (a) the
  interrupted SP lying inside the coroutine's green-stack range (not the native
  scheduler stack mid-`switch_to_g`), (b) `g->no_preempt == 0` (no lock/alloc
  held), and (c) `!g_boehm_in_stw` (not inside a Boehm stop-the-world, bracketed
  by `GC_set_on_collection_event`, `:2526`).

### Faults / diagnostics (I#77)

`zaprt.c:4397-4457`. `zap_fault_handler` runs on a `sigaltstack` for
SIGSEGV/FPE/BUS/ILL, writes the signal name + faulting address + offending
coroutine id/state with async-signal-safe `write(2)`, optionally
`backtrace_symbols_fd` (`-rdynamic`, I#78), then restores `SIG_DFL` and
re-raises so the process dies with the correct status / core dump.

The broader **signal dispatch** (`:4340-4590`): a dedicated signal thread masks
the handled signals everywhere else, its async handler `write()`s the signo to a
**self-pipe**, and the signal thread **parks on `zap_netpoll_wait`** (the
netpoller, epoll today) to dispatch in normal context — fan-out to `notify`
channels, arm `cancel_on` scopes, run defaults (SIGQUIT→dump, SIGUSR1→GC,
SIGUSR2→profiler). `zap_signal_init()` is emitted at the top of **every** `main`.
The `Signal` prelude enum (`crates/syntax/src/parser.rs:62`) has discriminants
that **are the Unix signal numbers** (SIGHUP=1 … SIGTERM=15 … SIGTSTP=20).

### What MinGW-w64 gives for free (winpthreads)

MinGW-w64 ships **winpthreads**, so `pthread_create`/`mutex`/`cond`/`once`,
`nanosleep`, and `clock_gettime` all work unchanged (A5 links `-lpthread`).
What does **not** exist: `sigaction`, `SIGURG`/`SIGSEGV`-as-`sigaction`,
`sigaltstack`, `pthread_kill`, `pthread_sigmask` (a winpthreads no-op), the
self-pipe+`fcntl(O_NONBLOCK)` idiom, `backtrace`, and `<sys/mman.h>`. So the port
is **not** "rewrite threading" — it is "replace signal delivery, fault trapping,
and the personality name."

## Proposal

### 1. Exception model — target-conditional personality, nothing else

**Decision: MinGW-w64 SEH model; the only change is the personality name.**

**Codegen (`lib.rs`).** Compute `is_windows` once from
`TargetMachine::get_default_triple()` (exactly as `build_executable` already
does, `:142`) and thread it into `Cg`. Replace the four hardcoded
`"__gxx_personality_v0"` sites with a single `self.personality_name` chosen at
setup:

```rust
let personality_name = if is_windows { "__gxx_personality_seh0" }
                       else          { "__gxx_personality_v0" };
```

- `:528` declares that name; `:1243`, `:2122`, `:2644`, `:2676` look it up by the
  same key. `uwtable(2)` stays on every function (correct on both ELF and COFF).
  `landingpad`/`catch ptr null` stays. The module's target triple is already the
  Windows triple when the compiler runs on a Windows host, so the LLVM backend
  emits SEH `.pdata`/`.xdata` automatically — this is precisely what Clang does
  for `x86_64-w64-windows-gnu`.

**Runtime glue (`zaprt.c`).** **Unchanged.** `__cxa_allocate_exception`/
`__cxa_throw`/`__cxa_begin_catch`/`__cxa_end_catch` and the
`_ZTVN10__cxxabiv117__class_type_infoE` type_info all resolve from MinGW's
libstdc++ (already linked `-lstdc++`). `zap_panic`/`zap_eh_begin_catch`/
`zap_eh_end_catch`/`zap_uncaught` compile and link verbatim. The file stays
pure C; no C++ translation unit is introduced.

**Why not re-express the glue as portable C++ `throw`/`catch` (the spike's
option)?** That option exists only to drop the Itanium-*symbol* dependency for a
future **MSVC** target. On MinGW the `__cxa_*` symbols exist, so the
re-expression buys nothing for v1 while adding risk: to be meaningfully portable
to MSVC it must be paired with funclet-based codegen (`catchpad`/`cleanuppad`)
and the `__CxxFrameHandler3` personality, which is a large codegen change this
ticket does not need. **Recommendation: defer the C++ re-expression and MSVC to
a follow-up ticket; ship MinGW v1 with the personality-name change.** This keeps
A3 small and the Linux path at a literal zero diff.

**The one thing to verify on the runner** (per the spike): an exception raised
and caught across frames that live on a hand-switched green stack. SEH consults
the TIB stack-bounds (`gs:[0x08]`/`[0x10]`/`[0x1478]`) during unwinding, and the
A1 switch (`zap_ctx_win_x64.c`) installs/restores exactly those on every switch —
so an unwind that begins on a green stack sees consistent bounds. This is the
load-bearing interaction between A1 and A3 and is the primary new Windows golden.

### 2. Preemption (I#172 port) — flag for immix, hijack for boehm

The **monitor thread, `schedtick` sampling, and `m->preempt` store are portable**
(winpthreads + `nanosleep`) and stay as-is. Only "how the target yields" changes,
and it changes per collector exactly as the Unix path already splits:

**immix (precise, the default): a plain flag store — no suspension.** The Unix
monitor already sets `m->preempt = 1` *before* `pthread_kill` (`:2602`), and the
immix handler does nothing but record that flag; the yield is cooperative at the
next `zap_safepoint()`. **The `SIGURG` kick is therefore redundant for immix** —
it only shortens latency by nudging a thread out of a blocking call, and a
compiled loop polls a safepoint within one iteration regardless. So on Windows,
immix preemption is **just the monitor's `m->preempt = 1` store**, with no signal
and no `SuspendThread`. This is fully GC-safe (same safepoint machinery) and is
the ideal outcome.

**boehm (conservative, fallback): monitor-driven hijack.** boehm has no
cooperative safepoint, so the Unix handler switches at the interrupted PC. The
Windows analogue runs in the **monitor** (there is no async handler to run on the
target thread):

1. Guard `!g_boehm_in_stw` (reuse the existing `GC_set_on_collection_event`
   bracket — bdwgc's own Win32 STW also uses `SuspendThread`, so we must never
   suspend a thread the collector is suspending).
2. `SuspendThread(m->thread_handle)` then `GetThreadContext(..., CONTEXT_FULL)`
   (the `GetThreadContext` return synchronizes the suspend).
3. Apply the **same three guards** as the Unix handler, now read from the fetched
   context: `ctx.Rsp` inside `[g->stack, g->stack + g->stack_size)`,
   `g->no_preempt == 0`, `m->current && !m->current->locked_m`.
4. If safe, **hijack**: push the current `Rip` as a return address onto `Rsp`
   (the green stack), set `Rip` to a `zap_copreempt_trampoline`, `SetThreadContext`,
   `ResumeThread`. The trampoline runs `copreempt()` on the coroutine's own green
   stack (Rsp is already there, so the saved context is scanned correctly), then
   `ret`s to the original Rip when the coroutine is later resumed. This mirrors
   Go's `runtime.preemptM` on Windows and CoreCLR thread hijacking.
5. If not safe, `ResumeThread` and let the monitor retry next interval (same
   deferral semantics as the Unix `no_preempt` path).

**In-flight guard — re-injection must be impossible, not merely improbable.**
The Unix SIGURG path has no re-injection race *for free*: a signal handler
auto-masks its own signal (no `SA_NODEFER` in `zap_preempt_install`), and
`copreempt` does not return until the coroutine is resumed — so SIGURG stays
blocked across the **entire** in-flight window (handler → `copreempt` → parked →
resumed → handler returns). Windows has no such automatic masking: nothing stops
the monitor from `SuspendThread`-injecting a **second** trampoline while the
first is still on the green stack (its prologue, or after `copreempt` cleared but
before it switched). That nests frames and is a real, timing-dependent race (a
slow/descheduled worker can still be mid-trampoline when the next tick fires —
timer spacing alone does **not** prevent it), not merely a test artifact. So the
hijack takes a **per-M `in_hijack` flag** and `InterlockedCompareExchange`-sets
it before suspending; while a hijack is in flight the monitor refuses to inject
another, so re-injection is impossible regardless of timing. `copreempt` (the
trampoline target) clears the flag once the coroutine has yielded — exactly as a
Unix handler *returning* unblocks SIGURG. This is implemented in the primitive
(`zap_win_preempt_try`, `zap_win_a3.c`) and verified by the harness; the
production monitor passes `&m->in_hijack` and `copreempt` clears
`cur_m->in_hijack`.

**Trampoline alignment.** The injected trampoline is entered with `rsp =
interrupted_rsp - 8` (so its final `ret` restores `rsp` bit-for-bit). Since the
interrupted `rsp` is only 8-aligned, the entry `rsp` is ≡ 0 **or** 8 (mod 16);
the trampoline must tolerate both. It saves the frame base in a callee-saved
register, `andq $-16` aligns the call site explicitly, then restores the exact
base — a fixed pad would mis-align the `call` for one of the two cases and `#GP`
in the callee's aligned SSE spills (the first on-runner failure, caught by the
harness and fixed).

This needs a Win32 thread **HANDLE** per M (`SuspendThread` takes a handle, not a
`pthread_t`); store it in `M` next to `m->thread`, obtained via
`pthread_gethandle`/`_beginthreadex` or `OpenThread(GetThreadId(...))` at M
startup. The monitor loop structure, interval, and `last[]` tick array are
unchanged.

**SIGPROF (I#78 profiler) is the same shape** and is *explicitly deferred* — the
profiler is Linux/x86-64-only (I#78 design). On Windows the profiler monitor
would reuse this exact `SuspendThread`+`GetThreadContext`+`CaptureStackBackTrace`
sampling, but that is a profiler follow-up, not A3. A3 only ensures the preempt
path compiles with the profiler `#ifdef`'d out on Windows.

### 3. Faults / diagnostics (I#77 port) — a Vectored/Unhandled exception filter

Hardware faults in Zap are **always fatal** today (faults→`zap_panic` is a
deferred I#22 follow-up), so the fault handler is a *crash reporter*, not a
recovery path. Windows surfaces faults as SEH exceptions, not signals.

- **Install** `SetUnhandledExceptionFilter(zap_fault_filter)` (runs *after* SEH
  frame handlers find no catch — the clean analogue of "no handler → report →
  `SIG_DFL`"). A top-priority `AddVectoredExceptionHandler(1, …)` is the
  alternative but would also intercept any first-chance SEH we might later use for
  faults→panic; `SetUnhandledExceptionFilter` keeps the door open and matches
  current semantics (faults are terminal). **Recommendation: unhandled-exception
  filter for v1.**
- **Report** from the filter's `EXCEPTION_POINTERS`: map `ExceptionCode`
  (`EXCEPTION_ACCESS_VIOLATION`→"SIGSEGV-equiv", `EXCEPTION_INT_DIVIDE_BY_ZERO`/
  `_FLT_*`→"SIGFPE", `EXCEPTION_ILLEGAL_INSTRUCTION`→"SIGILL",
  `EXCEPTION_STACK_OVERFLOW`→"stack overflow") to the same human text
  `zap_fault_handler` prints; faulting address = `ExceptionRecord->
  ExceptionInformation[1]` for an access violation; offending coroutine from
  `cur_m->current` exactly as today.
- **Backtrace** via **dbghelp** (`-ldbghelp` already linked, A5): `SymInitialize`
  once at init, then `CaptureStackBackTrace` (or `StackWalk64` for a full walk
  using the filter's `CONTEXT`) + `SymFromAddr`/`SymGetLineFromAddr64`. dbghelp
  `Sym*` is **not thread-safe** (serialize with the signal mutex) and should not
  allocate heavily, but a Windows filter runs in a far more permissive context
  than a Unix async-signal handler — it is an ordinary call on the faulting
  thread, so the async-signal-safety constraints that shaped I#77 do not apply.
- **Return** `EXCEPTION_CONTINUE_SEARCH` so the OS default handler still produces
  the crash dump / nonzero exit (the "re-raise `SIG_DFL`" analogue).
- **Stack-overflow headroom:** `SetThreadStackGuarantee(64*1024)` on the main and
  worker threads reserves stack for the filter (the sigaltstack analogue). On a
  *green* stack, overflow is the A1 soft-redzone `zap_panic("stack overflow")`
  path (`zaprt.c:2973`), not a hardware fault, so the guarantee only matters for
  native threads.

### 4. Signal surface — map the enum to console control events

Windows console control events are the only "signal-like" delivery: via
`SetConsoleCtrlHandler`, the OS spawns a thread to run the handler for
`CTRL_C_EVENT`, `CTRL_BREAK_EVENT`, `CTRL_CLOSE_EVENT`, `CTRL_LOGOFF_EVENT`,
`CTRL_SHUTDOWN_EVENT`. Map the portable `Signal` enum's *mappable subset*:

| `Signal` (Unix no.) | Windows source |
|---|---|
| `SIGINT` (2)  | `CTRL_C_EVENT` |
| `SIGQUIT` (3) | `CTRL_BREAK_EVENT` (closest console analogue) |
| `SIGTERM` (15)| `CTRL_CLOSE_EVENT` / `CTRL_SHUTDOWN_EVENT` / `CTRL_LOGOFF_EVENT` |

All other discriminants (SIGHUP, SIGUSR1/2, SIGPIPE, SIGALRM, SIGCHLD, SIGCONT,
SIGTSTP, SIGABRT) have **no console analogue**. Decision: on Windows,
`signal.notify`/`@on_signal`/`cancel_on` for an unmappable signal returns the
existing **inert channel / no-op registration** (`zaprt.c:4578`, "subsystem
unavailable") rather than a compile error — the `Signal` enum is a portable
cross-platform type and defensive code may name SIGUSR1 unconditionally. The
default *diagnostic* triggers (SIGQUIT→dump, SIGUSR1→GC, SIGUSR2→profiler) have
no console trigger; they remain reachable via env vars / `zap inspect` (which on
Windows uses `GenerateConsoleCtrlEvent` for Ctrl-Break only) and are documented
as such. v1 Windows signal behavior: **CTRL_C/CTRL_BREAK/CTRL_CLOSE deliver
SIGINT/SIGQUIT/SIGTERM to `notify`/`cancel_on`, with a default graceful
shutdown** — enough for a well-behaved CLI.

### 5. Dispatch architecture on Windows — decoupled from the netpoller (A2)

The #77 dispatch thread parks on `zap_netpoll_wait` (the self-pipe registered
with epoll). Windows netpoller = IOCP = **A2/#196, out of this ticket.** Rather
than pull A2 in, split the Signals section so **Windows dispatch does not use the
netpoller at all**:

- `SetConsoleCtrlHandler`'s handler runs on an OS-spawned thread; it posts the
  mapped signo to a Win32 condition variable + SRW lock (or a winpthreads
  `pthread_cond_t`, which exists) — **not** a self-pipe.
- A dedicated Windows **signal-dispatch thread** waits on that condvar and runs
  the identical normal-context dispatch (`zap_sig_dispatch`: fan-out to channels,
  arm cancel_on, defaults). `zap_signal_init()`/`zap_signal_shutdown()` start/stop
  this thread on Windows instead of the self-pipe+netpoller pair.

This keeps A3 fully independent of A2: a Windows Zap program that does no network
I/O never needs the (unimplemented) IOCP netpoller, and signals still work.

## Compatibility

- **Linux/macOS: zero diff — the strongest form of the constraint.**
  - *Codegen:* the personality name is `__gxx_personality_v0` for every
    non-Windows triple, so the emitted IR/asm is identical; `uwtable(2)`,
    `landingpad`, and `catch ptr null` are untouched.
  - *Runtime:* every Windows addition is under `#if defined(ZAP_PLATFORM_WINDOWS)`;
    the POSIX branches (`signal.h`, `sigaction`, `sigaltstack`, the self-pipe, the
    `SIGURG` handler, `ucontext` via the A1 seam) are byte-for-byte unchanged.
  - *Proof:* the existing exception goldens (`exceptions_{catch,propagate,
    uncaught}`), the preemption golden (`preemption.zap`), and the signal goldens
    (`signal_notify`, `signal_default_usr1`, `scope_cancel_signal`,
    `signal_selftest`) run **unchanged** on the required Linux `host`/`full` and
    `arm64` jobs under **both** collectors. No golden is edited; no exact-heap-byte
    assertion is added (standing rule).
- **Windows:** full programs with `throw`/`catch` link and run (under boehm — see
  the immix boundary below); preemption fires; faults are reported.
- **Language semantics: none.** No grammar change, no new ZIR, no ABI change
  beyond Windows-only internal symbols. The `Signal` enum surface is identical;
  only its Windows *runtime behavior* is the documented mappable subset.

## Scope boundaries — reported, not pulled in (per orchestrator)

1. **A2 (IOCP) — decoupled, not required.** §5 removes the netpoller dependency
   from Windows signal dispatch. A3 does **not** implement or require IOCP. Network
   I/O on Windows remains A2/#196.
2. **immix on Windows — blocked on a collector port, not A3.** `zapgc.inc.c:30`
   includes `<sys/mman.h>` and allocates GC memory with `mmap`; immix therefore
   **does not compile on Windows** until a small `mmap`/`munmap`/`mprotect`→
   `VirtualAlloc`/`VirtualFree`/`VirtualProtect` port lands. That file is the
   **collector allocator** (owned by the GC track, not the zaprt.c/exception lane
   this session holds solo). Consequently **Windows v1 runs under boehm** (the A1
   note already verified boehm-only on Windows), and A3's boehm `SuspendThread`
   hijack is the v1-critical preempt path. immix's Windows preemption is already
   correct *in code* (the collector-agnostic flag-at-safepoint path) and will work
   the moment the allocator is ported. **Recommendation: a separate small GC-track
   ticket for the immix `VirtualAlloc` port; A3 does not touch `zapgc.inc.c`.**
   "Both collectors where applicable" therefore means: Windows goldens run under
   **boehm**; the immix equivalents keep running on Linux/ARM and prove the
   collector-agnostic codegen (personality, landingpad) is unchanged.
3. **MSVC + the C++ EH re-expression — follow-up.** §1 ships MinGW only; MSVC
   needs funclet codegen and is a later ticket.
4. **Faults→`zap_panic`** (catch a SIGSEGV/divide-by-zero as a Zap exception)
   stays the deferred I#22 follow-up on all platforms.
5. **CPU profiler on Windows** (I#78) stays deferred; A3 only `#ifdef`s it out.

## Implementation plan (after sign-off)

Behind the existing A5 seam; all Windows code under `ZAP_PLATFORM_WINDOWS`.

1. **Codegen personality** (`lib.rs`): thread `is_windows` into `Cg`; one
   `personality_name` field; swap the four sites. (Smallest, highest-leverage
   change; type-checked by the Linux build.)
2. **zaprt.c Windows preemption:** per-M Win32 thread handle; Windows monitor
   arm (immix = flag only; boehm = suspend/getcontext/guard/hijack/resume);
   `zap_copreempt_trampoline` (tiny asm or naked thunk, sibling to
   `zap_ctx_win_x64.c`). Keep the POSIX monitor/handler untouched.
3. **zaprt.c Windows faults:** `SetUnhandledExceptionFilter` + dbghelp backtrace +
   `SetThreadStackGuarantee`; same report text as `zap_fault_handler`.
4. **zaprt.c Windows signal dispatch:** `SetConsoleCtrlHandler` → condvar →
   dispatch thread; the `Signal`→console map; inert-fallback for unmappable
   signals; Windows `zap_signal_init`/`_shutdown` not touching the netpoller.
5. **Narrow the `zaprt.c` `#error` gate:** mark A3 DONE (exceptions/preemption/
   faults/signals); remaining Windows blockers become A2 (IOCP) + A4 (fs/env/
   getentropy/CPython) + the immix `VirtualAlloc` port.
6. **CI:** promote the A1 `windows-ctxswitch` job (already MinGW + boehm-gc) to a
   **required full program link+run under boehm**; install the MSYS2
   `mingw-w64-x86_64-{pcre2,zlib,openssl,boehm-gc}` runtime libs (the A5 open
   issue). Add Windows goldens: an exception thrown and caught **across a
   coroutine switch** (exercises SEH + the A1 TIB bounds), a **caught fault**
   report (a dedicated non-golden test, as I#77 does — death-by-signal has no
   exit code), and a **preemption** fixture (the `MAXPROCS=1` tight-loop shape)
   under boehm.
7. **Gates:** `cargo fmt --check`; clippy under **both** `gc-immix` and
   `gc-boehm` (`-D warnings`); `make docker-test` under **both** collectors; the
   required Linux `pull_request` **and** `push` triggers green plus the Windows
   jobs.

## Open issues (for orchestrator/user sign-off)

- **Q1 — exception model.** Confirm **MinGW SEH via the personality-name change
  only**, deferring the C++ `throw`/`catch` re-expression and MSVC. This is the
  zero-Linux-diff path and the reason the exception half is *small*. (If MSVC is
  wanted in 1.0, it is a separate, larger funclet-codegen ticket.)
- **Q2 — immix on Windows.** Agree that the `mmap`→`VirtualAlloc` port is a
  **separate GC-track ticket** and Windows v1 is **boehm-only**, so A3 implements
  the boehm `SuspendThread` hijack and does **not** touch `zapgc.inc.c`? (The
  alternative — port immix now to get trivial flag-based preemption — is more
  coverage but crosses into collector-owned code and still wants the boehm path
  for the designated fallback.)
- **Q3 — fault interception.** Confirm `SetUnhandledExceptionFilter` (report →
  `CONTINUE_SEARCH` → OS default, matching today's terminal semantics) over a
  first-chance VEH.
- **Q4 — signal surface.** Confirm the mappable-subset + inert-fallback policy
  (no compile error for SIGUSR1 etc. on Windows) and the default
  CTRL_C→graceful-shutdown behavior.
- **Q5 — boehm/Win32 STW interaction.** bdwgc's Win32 STW and our monitor both
  use `SuspendThread`. The `g_boehm_in_stw` bracket is the guard; is there any
  additional bdwgc-on-Windows ordering (e.g. `GC_set_suspend_signal` is POSIX-only;
  Win32 bdwgc uses its own suspend) to validate on the runner? Flagged as the
  riskiest runtime interaction to test first.
