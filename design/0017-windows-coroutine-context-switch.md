# Proposal: Windows coroutine context switch (replace `ucontext`) — I#195 (A1) spike

- **Author(s):** win-ctxswitch spike session (I#195)
- **Last updated:** 2026-10-22
- **Status:** Draft (spike / recommendation — not an implementation)
- **Discussion:** https://github.com/parveshgarg/zap/issues/195
- **Epic:** I#158 (platform/1.0), Area A1. See `design/0016-platform-1.0.md`.

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

I#195 is the foundational Windows blocker: Zap's M:N coroutine scheduler switches
stacks with POSIX `ucontext` (`getcontext`/`makecontext`/`swapcontext`), which
does not exist on Windows, and A2 (IOCP), A3 (signals/preemption) and A4 (fs) are
all dead on arrival until a coroutine can switch stacks at all.
This spike answers the hard questions *before* any Windows code lands and makes a
definitive recommendation:

> **Use a small hand-written assembly context switch over runtime-owned
> (GC-allocated) green stacks — NOT native Windows Fibers.**

This is the only option that preserves Zap's load-bearing GC invariant
("a suspended coroutine's stack + saved registers are reachable and scannable
the same way on every platform and under both collectors") and keeps the
conservative `gc-boehm` fallback working on Windows.
Fibers hide their stack and a suspended fiber's saved register file, which
quietly breaks conservative scanning.
A runnable System V prototype and a compile-checked Windows x64 prototype
(`design/0017-win-ctxswitch-spike/`) de-risk the mechanism.

## Background — what exists today

### The current switch (`crates/codegen/src/zaprt.c`)

The scheduler is the Go-style M:P:G design from `design/0005-concurrency.md`
(ticket #47, reaffirmed by preemption #172):

- A **G** (coroutine) is a GC-heap object holding a fixed 128 KiB green stack
  (`g->stack`, a `GC_MALLOC`/`zap_ptrbuf` buffer) plus a saved `ucontext_t ctx`
  *inside the G*.
- `galloc()` builds the context with `getcontext` + `makecontext(&g->ctx,
  g_bootstrap, 0)`, pointing `uc_stack` at `g->stack`.
- `switch_to_g()` brackets `swapcontext(&m->sched_ctx, &g->ctx)` with a
  `GC_set_stackbottom` update (`set_bottom`) and, under immix, a shadow-stack
  head save/restore.
- `g_bootstrap()` runs on the green stack, calls the user function, marks the G
  `G_DEAD`, and `swapcontext`s back to the scheduler.
- There is an explicit **"x86_64 Linux" arch assumption** in the source
  (`zaprt.c:1811`).

### The GC invariant this must preserve — and why it differs per collector

This is the crux of the whole ticket, so it is worth stating precisely.
Reading the runtime, the "scan a suspended coroutine" requirement resolves
**differently under the two collectors**:

**`gc-boehm` (conservative, the fallback).** `zaprt.c:1798-1808`: each green
stack is a `GC_MALLOC` pointer-bearing object *scanned as heap*, and the saved
`ucontext_t` (callee-saved registers + SP) lives inside the GC-allocated G, also
*scanned as heap*.
So a parked coroutine's roots — including any heap pointer live only in a
callee-saved register — are found with **no stack maps and no safepoints**,
purely because both the stack buffer and the register save-area are GC heap.
A *running* coroutine's stack is instead scanned as the owning M thread's stack
via `GC_set_stackbottom` (bdwgc's documented coroutine support).

**`gc-immix` (precise, the default and the future).** `zapgc.inc.c:13-14`:
"Roots are found PRECISELY via a per-coroutine shadow stack (so a parked
coroutine's roots are just its saved chain)."
The machine stack is **never scanned** in immix mode; `g->shadow_head` holds the
parked coroutine's root chain, and the green-stack buffer is an *atomic*
(non-scanned) allocation kept alive only by the `G.stack` field.
I#120 evacuation (`zapgc.inc.c:125-130`) moves objects but **pins** the G and its
green stack ("marked BEFORE any evacuation runs"), and fixes up pointers found
through the shadow chain + global roots — never by walking the machine stack.

The consequence (developed in the Rationale) is that **immix doesn't care how the
stack is allocated**, but **boehm fundamentally needs the runtime to own the stack
memory and the register save-area so it can scan them as heap.**

### Exceptions / unwinding (I#22/I#40, I#47 spawn landing pad)

`zaprt.c:1160-1217` + `crates/codegen/src/lib.rs:455-472,1166-1183,2019-2107`:

- Codegen emits LLVM `invoke`/`landingpad` and names the **Itanium** personality
  `__gxx_personality_v0`, linked via `-lstdc++`; the actual unwinder
  (`_Unwind_*`) comes from libgcc.
- `zap_panic` raises a boxed C++ exception via `__cxa_allocate_exception` /
  `__cxa_throw`; landing pads recover it via `__cxa_begin_catch`.
- Codegen forces **asynchronous DWARF unwind tables (`.eh_frame`/CFI) on every
  function** (`setNeedsUnwindTableEntry`) so the unwinder can walk frames with no
  landing pad of their own.
- The **I#47 spawn landing pad** is each coroutine's base frame: an uncaught
  exception in a coroutine is caught at the spawn trampoline, so unwinding never
  crosses the `makecontext` bootstrap into the scheduler.

### The link step (`crates/codegen/src/lib.rs:128-160`)

Shells out to `cc` with `-lstdc++ -lm -lpcre2-8 -lz -lssl -lcrypto -lpthread
-rdynamic` + `pkg-config` CPython flags, and targets the host triple only.
Every one of these is a Unix-ism; A5 (#199) owns the toolchain swap.

## Proposal

### Mechanism: a hand-written asm context switch over GC-allocated stacks

Replace `ucontext` with a tiny per-arch assembly routine, keeping the *exact*
"green stack is a runtime-owned buffer + saved context reachable from the G"
model that exists today.
A "context" becomes a single saved stack pointer; the register save-area lives on
the coroutine's own (runtime-owned, GC-scanned under boehm) stack — the
Boost.Context / libco model.

Concretely, introduce a thin seam so `zaprt.c` stops calling `ucontext` directly:

```c
typedef void *zap_ctx;                        /* a saved stack pointer */
void    zap_ctx_swap(zap_ctx *from, zap_ctx to);        /* <- was swapcontext */
zap_ctx zap_ctx_make(void *stack, size_t size,          /* <- was get/makecontext */
                     void (*entry)(void), struct G *g);
```

- **Linux/macOS x86_64 & aarch64:** implement `zap_ctx_swap`/`zap_ctx_make` in
  asm (or keep `ucontext` behind the seam initially — the seam is the deliverable
  that unblocks Windows; swapping Linux to asm is an optional, strictly-faster
  follow-up since `swapcontext` does a `sigprocmask` syscall per switch).
- **Windows x86_64:** implement the seam in asm as prototyped
  (`design/0017-win-ctxswitch-spike/ctx_win_x64.c`). The switch saves/restores
  the Windows nonvolatile set and swaps the TIB stack-bounds fields (below).

The `G` struct's `ucontext_t ctx` field becomes a `zap_ctx ctx` (one pointer).
Under boehm it is still scanned as heap; the register save-area it points at
lives on the green stack, which is also scanned as heap — invariant preserved
verbatim.
`switch_to_g`, `g_bootstrap`, `galloc`, and the two yield sites
(`zaprt.c:2878,2903,3184`) call the seam instead of `swap/get/makecontext`; the
`set_bottom` bracketing and immix shadow-head save/restore are **unchanged**.

### Windows x64 ABI specifics the switch must handle

Validated by assembling the prototype for `x86_64-pc-windows-msvc`:

1. **Extra nonvolatile GPRs.** Windows adds **RDI and RSI** to the callee-saved
   set `{RBX, RBP, RDI, RSI, R12–R15}` (System V does not save RDI/RSI).
2. **Nonvolatile SSE registers.** **XMM6–XMM15 are callee-saved on Windows**
   (System V has none) — 160 bytes the switch must save/restore.
3. **TIB stack-bounds fields.** Windows keeps the running thread's stack extent
   in the TIB, read via `GS`: **StackBase `gs:[0x08]`**, **StackLimit
   `gs:[0x10]`**, **DeallocationStack `gs:[0x1478]`**. SEH unwinding, guard-page
   stack growth, and some Win32/CLR calls read these; Boost.Context swaps them on
   every jump, and a documented CLR crash exists when DeallocationStack is stale
   (trac ticket 8544). The switch must install the incoming coroutine's bounds
   and restore the outgoing ones. System V needs none of this.
4. **Shadow space / alignment.** The 32-byte caller-allocated home/shadow space
   and 16-byte alignment are a property of how the bootstrap *calls* the user
   function, not of the switch itself; `zap_ctx_make` lays out the initial frame
   so the trampoline is entered with the right alignment (the SysV prototype does
   exactly this; the Windows `zap_ctx_make` is the piece still needing on-Windows
   validation).
5. **Guard pages.** We deliberately keep the v1 fixed stack with a soft overflow
   check (`ZAP_STACK_REDZONE`, `zaprt.c:1840`); a real `PROT_NONE`/guard page
   inside a scanned buffer would break boehm conservative scanning (already noted
   for Linux). Hardware guard/growable stacks remain deferred to the precise-GC
   era on all platforms. Because our stacks are not OS-grown, the TIB StackLimit
   we install is simply the low end of the fixed buffer.

### GC answer (definitive)

| | native Fibers | **hand-asm + runtime stacks (recommended)** |
|---|---|---|
| **immix (precise, default)** | Works — roots come from `g->shadow_head`, not the machine stack; stack allocator is irrelevant. | Works — identical shadow-chain path, unchanged. |
| **boehm (conservative, fallback)** | **Breaks.** A fiber's stack is OS-allocated and opaque (not a GC object → not scanned), and a *suspended* fiber's saved register file is stored opaquely by the OS → callee-saved roots are invisible. Would need `GC_add_roots` over each fiber's whole committed range (over-scan, and the base/limit must be discovered) *and* still cannot reach the saved registers. | Works — green stack is `GC_MALLOC` (scanned as heap) and the register save-area sits on that same scanned stack, exactly as today. |
| **I#120 evacuation** | Orthogonal (shadow-chain + pinning), so fine *for immix*, but inherits boehm's breakage. | Orthogonal and unchanged; G + green stack already pinned. |
| **cross-platform uniformity** | Two different GC-invariant stories (Linux heap-stack vs Windows opaque-fiber-stack) to maintain and test. | One model on every platform and both collectors. |

**Recommendation: hand-written asm.**
It is the only choice that keeps `gc-boehm` working on Windows and keeps a single
GC invariant across platforms.
Fibers' one genuine advantage — the OS maintains TIB/SEH/guard pages for you — is
a bounded, well-understood ~3-field TIB swap in the asm path, already prototyped;
it does not justify forking the GC model or dropping the conservative fallback
(which is exactly the fallback you want on the newest, least-proven port).

### Exceptions / unwinding scope (answers Q3)

The context-switch mechanism and the unwinder are **separable**; A1 can ship the
switch while the exception-personality swap is scoped into **A3/A5 (#197/#199)**.
What changes on Windows x64:

- **Personality + tables.** DWARF `.eh_frame`/CFI and `__gxx_personality_v0` are
  the *Itanium* model. Windows x64 is **table-based SEH** (`.pdata`/`.xdata`
  unwind info) with a different personality. With a clang-cl/LLVM Windows target,
  LLVM emits SEH unwind info automatically, and the C++ personality becomes
  `__CxxFrameHandler3` / `__GCC_specific_handler` depending on the runtime. The
  codegen that *forces* `.eh_frame` and *names* `__gxx_personality_v0`
  (`lib.rs:1166,468`) must become target-conditional: on Windows, set the
  Windows personality (or let the LLVM target default apply) and do not force
  DWARF tables.
- **`__cxa_*` glue.** `zap_panic`/`zap_eh_begin_catch` call `__cxa_allocate_
  exception`/`__cxa_throw`/`__cxa_begin_catch`. Under MinGW these still exist
  (libgcc/libstdc++); under MSVC they do not — the equivalent is the MSVC C++
  EH ABI (`_CxxThrowException` + `__CxxFrameHandler`). The cleanest portable
  option is to keep the current "box a pointer and throw a C++ exception"
  strategy but let the C++ compiler/runtime on each target provide throw/catch
  (i.e. compile the EH glue as actual C++ `throw`/`catch` of a `ZapException`
  class, rather than naming Itanium `__cxa_*` symbols directly). That removes the
  Itanium-symbol dependency and works on both MinGW and MSVC.
- **The I#47 spawn landing pad is unaffected in structure.** It remains each
  coroutine's base catch frame; what changes is only the personality/tables the
  landing pad is lowered against. Because unwinding is driven per-frame on the
  coroutine's own stack and never crosses the bootstrap, the switch mechanism
  imposes no additional unwinder constraint — *provided* the unwind info for the
  frames on a green stack is correct for the target. SEH table-based unwinding
  walks via `.pdata` keyed on the instruction pointer, which is independent of
  which stack the frames sit on, so hand-switched green stacks are fine.
- **One real interaction to verify on Windows:** SEH consults the TIB stack
  bounds during unwinding. This is *why* the asm switch must maintain
  StackBase/StackLimit/DeallocationStack — with those installed, an unwind that
  begins on a green stack sees consistent bounds. This is the single most
  important thing to test once a Windows toolchain exists.

**Scope estimate:** the personality/table swap is **A5 toolchain + A3 work, size
M**, mostly target-conditional codegen (a handful of `lib.rs` sites) plus
re-expressing the EH glue in portable C++. It does **not** block landing the A1
switch itself behind the seam.

## Rationale

### Why not native Fibers

Covered in the GC table: Fibers break conservative boehm scanning of suspended
coroutines (opaque stack + opaque saved registers) and force two divergent
GC-invariant stories.
Additional friction: `ConvertThreadToFiber` per worker M, fiber-local-storage
management, and lazily-committed guard-page stacks we explicitly do not want
interacting with conservative scanning.
Fibers would be *viable if we were immix-only forever* — but boehm is the
documented fallback and the safer collector for a brand-new port.

### Why not keep `ucontext` via a shim

There is no `ucontext` on Windows (not even a stable MinGW one that handles our
stack-ownership model), and third-party `ucontext` shims re-introduce exactly the
opacity problems we are trying to avoid.
A ~40-line asm routine per arch is less code and fully under our control (needed
anyway for the TIB handling and the aarch64 C2 work).

### Why the seam helps Linux too (bonus, out of scope)

`swapcontext` issues a `sigprocmask` syscall on every switch; a hand-asm switch
does not.
Replacing the Linux path behind the same seam is a free latency win for the
hot scheduler path, but it is explicitly **not** part of A1 — the seam is
designed so Linux can keep `ucontext` unchanged while Windows gets asm.

## Compatibility

- **Language semantics: none.** This is a pure runtime swap, exactly like the
  #47 scheduler rewrite — every `zap_*` entry point keeps its signature and
  codegen/lowering are untouched by A1 (the EH-personality change is separate and
  also language-invisible).
- **GC: none.** Both collectors keep their current root model; the invariant is
  preserved verbatim under the recommended approach.
- **Goldens:** no exact-heap-byte assertions (per the standing rule); the switch
  is behaviourally transparent.

## Implementation (follow-on plan for the real #195 and #197)

**#195 (A1) — the real implementation, once a Windows toolchain exists (needs
A5-toolchain half first):**

1. Introduce the `zap_ctx_swap`/`zap_ctx_make` seam in `zaprt.c`; replace the 4
   `ucontext` call sites and the `G.ctx` field. Keep Linux/macOS on `ucontext`
   behind the seam (zero behaviour change) to land the refactor on green CI.
2. Add the x86_64 asm backends: System V (done in prototype) and Windows
   (prototype assembles; finish `zap_ctx_make` initial-frame layout + TIB
   StackBase/StackLimit setup, validate on Windows).
3. Verify the GC invariant on Windows under **both** collectors: a parked
   coroutine holding the only reference to a heap object survives a collection
   (boehm: scanned-as-heap; immix: shadow chain), and an I#120 evacuation cycle
   with a parked coroutine keeps it pinned.
4. Soft stack-overflow redzone check wired to the installed TIB StackLimit.
5. (Optional, separate PR) switch Linux to the asm backend for the perf win.

**#197 (A3) + A5 unwinder slice — exceptions on Windows:**

1. Make codegen target-conditional: on Windows do not force DWARF `.eh_frame`;
   set the Windows C++ personality (or rely on the LLVM target default).
2. Re-express the `__cxa_*` EH glue as portable C++ `throw`/`catch` of a
   `ZapException` so it works on MinGW *and* MSVC without naming Itanium symbols.
3. Verify an exception raised and caught across frames living on a hand-switched
   green stack unwinds correctly (the key test — exercises SEH + the TIB bounds
   the A1 switch installs).

**Ordering:** A5-toolchain (compile *anything* on Windows) → A1 switch → A1 GC
verification → A3/A5 unwinder → then A2 IOCP / A4 fs unblock.

**What could NOT be verified in this spike (no Windows toolchain on the dev
host):** the Windows prototype was compile-checked (assembles to a correct COFF
object, confirmed by disassembly) but not linked or run; the `zap_ctx_make`
initial-frame layout for Windows, the exact `<winnt.h>` TIB offsets, SEH
unwinding across a green stack, and the EH-glue personality swap all require a
`windows-latest` runner (A5 / #199) to validate.

## Open issues

- **MinGW vs MSVC** for the first Windows port decides the EH story (MinGW keeps
  `__cxa_*`/libgcc and is the lower-friction first target; MSVC is the
  longer-term "native" story). Recommend **MinGW-w64 first** to minimise the
  exception-ABI change, then MSVC. This choice belongs to A5 (#199).
- **aarch64 Windows** (and aarch64 generally, C2 #204) needs its own asm backend;
  the seam accommodates it but it is out of A1's x86_64 scope.
- **Exact TIB offsets** must be re-derived from `<winnt.h>`; the values here are
  the documented x64 `NT_TIB` layout, not toolchain-extracted.
