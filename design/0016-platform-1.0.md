# Proposal: Platform coverage (Windows/WASM) & 1.0 launch readiness

- **Author(s):** platform-scoping session (I#158)
- **Last updated:** 2026-10-22
- **Status:** Draft
- **Discussion:** https://github.com/parveshgarg/zap/issues/158

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

I#158 is the EPIC that gates the public launch: the process and platform work
(not language features) a public, installable 1.0 needs.
This doc surveys the platform-specific surfaces in the tree today, breaks the
epic into sequenced, independently-shippable sub-tickets across its six areas
(Windows, WASM, cross-compile/ARM, spec-freeze/semver, release engineering,
community infra), gives each a rough size and ordering, and recommends the
first concrete slice to implement.
This is a planning doc — it does not itself change the compiler or runtime.

## Background — what is platform-specific today

The front-end crates (`syntax`, `sema`, `sema_types`, `ir`, `ir_core`, `mono`)
are portable Rust and carry essentially no OS assumptions.
Everything platform-specific lives in two places: the **C runtime** that
codegen emits and links, and the **driver's link step**.

### The C runtime (`crates/codegen/src/zaprt.c`, ~11.2k lines; `zapgc.inc.c`, ~1.6k)

Deeply POSIX / Linux-x86_64. Concretely:

- **Coroutine context switch = `ucontext`** (`getcontext`/`makecontext`/
  `swapcontext`), with a stated "x86_64 Linux" arch assumption (scheduler I#47,
  reaffirmed by preemption I#172). Green stacks are GC-heap objects; a suspended
  coroutine's `ucontext_t` is scanned as heap. This is the single most load-
  bearing POSIX dependency — no coroutine can switch stacks without it.
- **Threads/sync = `pthread`** throughout (`pthread_create` via
  `GC_pthread_create`, mutex/cond, `pthread_once`, `pthread_sigmask`,
  `pthread_kill`). The M:N scheduler, channels, `sync`, and `threading` (I#175)
  all build on this.
- **Preemption = `SIGURG` + `sigaction`** (I#172): a monitor thread samples each
  M's schedtick and `pthread_kill`s `SIGURG`; the handler yields at the next GC
  safepoint (immix) or direct-switches (boehm). Relies on delivering a signal to
  a specific thread and (boehm) `swapcontext` from a signal handler.
- **Signals + faults = `sigaction` + self-pipe + `sigaltstack`** (I#77): a
  dedicated signal thread parks on the netpoller; fault handler uses
  `backtrace_symbols_fd`. Default handlers map Unix signal numbers (the `Signal`
  prelude enum's discriminants *are* the Unix numbers).
- **Netpoller = `epoll` + `eventfd`** (I#80), behind a clean backend seam
  (`np_backend_open/add/del/kick`); non-Linux currently returns `NULL`
  ("unsupported"). kqueue is noted as the intended readiness-model follow-up;
  **IOCP is a different (completion) model**, see A2.
- **Misc syscalls:** `clock_gettime(CLOCK_MONOTONIC/REALTIME)`,
  `sysconf(_SC_NPROCESSORS_ONLN)` for the M count, `getentropy`/`/dev/urandom`
  (crypto), `<unistd.h>`, `fcntl`/`O_NONBLOCK`.

### The driver link step (`crates/codegen/src/lib.rs`)

- Target machine uses **`TargetMachine::get_default_triple()`** only — codegen
  can only emit for the host. No `--target` / cross-compile path exists.
- Links by shelling out to **`cc`** with POSIX/Unix libraries: `-lgc` (Boehm,
  boehm builds), `-lstdc++` (Itanium unwinder/`__cxa_*`), `-lm`, `-lpcre2-8`,
  `-lz`, `-lssl -lcrypto`, `-lpthread`, `-rdynamic`, plus `pkg-config` CPython
  embed flags. Every one of these is a Unix-ism (library names, `-rdynamic`,
  the unwinder personality, `pkg-config`).

### CI, versioning, community

- **CI** (`.github/workflows/ci.yml`): two jobs, both `ubuntu-latest` — `host`
  (no LLVM) and `full` (LLVM 18 + codegen, both collectors). No Windows, no
  macOS, no ARM runner.
- **Version:** workspace `0.0.1`. `SPEC.md` is a "0.2 draft, living document";
  latest decision is D-0069; `OPEN-QUESTIONS.md` still open.
- **Community:** `.github/` contains only `workflows/`. No `CONTRIBUTING.md`,
  `CODE_OF_CONDUCT.md`, `SECURITY.md`, `CHANGELOG.md`, or issue/PR templates.
  Dual MIT/Apache-2.0 license is already in.

## Proposal — sub-tickets by area

Sizes: **S** ≈ a day, **M** ≈ a few days, **L** ≈ 1–2 weeks, **XL** ≈ multi-week
and risky. All runtime work must pass the standard gates (fmt, clippy under both
`gc-immix` and `gc-boehm`, `make docker-test` both collectors) and verify both
`pull_request` and `push` CI triggers; goldens must not assert exact heap bytes.

### Area A — Windows support

Ordering within A: **A1 → A2/A3/A4 (parallel) → A5-CI**. A5's toolchain half can
start immediately (it unblocks compiling anything at all on Windows).

- **A1. Windows coroutine context switch (replace `ucontext`).** `ucontext` does
  not exist on Windows. Options: native **Fibers** (`CreateFiber`/
  `SwitchToFiber`/`ConvertThreadToFiber`) or a small hand-written asm context
  switch. Must preserve the GC invariant that a suspended coroutine's saved
  context + green stack are scanned (fibers own their stacks — revisit how the
  collector sees them; this interacts with both boehm and precise immix, and
  with I#120 evacuation). **Foundational: A2/A3/A4 are dead on arrival without
  it.** Size **XL**. Highest technical risk in the epic.
- **A2. Windows IOCP netpoller backend.** Implement the `np_backend_*` seam on
  **IOCP**. Note this is a *completion* model (you learn an op finished), not a
  *readiness* model (epoll/kqueue tell you a fd is ready) — the parked-G/latch
  logic and the `zap_netpoll_wait` "ready/timeout/closed" contract need adapting,
  likely via `RIO`/overlapped sockets or an AFD-readiness shim. Depends on A1
  (parking a coroutine requires switching). Size **L**.
- **A3. Windows signals / preemption / faults.** No `sigaction`/`SIGURG`/
  `sigaltstack`. Preemption → `SuspendThread`/`GetThreadContext`/`QueueUserAPC`;
  Ctrl-C/termination → console control handlers; faults → SEH/vectored exception
  handler + `StackWalk64`/`dbghelp`. The `Signal` prelude enum (Unix numbers)
  needs a Windows mapping or a portable abstraction. Depends on A1. Size **L**.
- **A4. Windows path / process / fs / env portability.** Audit the `os`, `sys`,
  `fs`, `process`, `io` stdlib modules and their zaprt.c backing for `/`-paths,
  `fork`/`exec`, `/dev`,`/proc`, permission bits, env handling. Define path
  semantics (separator, drive letters, UTF-16 APIs vs UTF-8). Largely runtime +
  module seam, less GC-entangled than A1–A3. Size **M**.
- **A5. Windows build/link toolchain + CI.** Replace the `cc` + POSIX-lib link
  with an MSVC (`clang-cl`/`link.exe`) or MinGW path; source pcre2/zlib/OpenSSL/
  libgc (vcpkg?); decide the unwinder story (SEH vs the current `-lstdc++`
  Itanium path, which affects I#22/I#40 exceptions and the I#47 spawn landing
  pad); handle CPython embedding on Windows. Add a `windows-latest` CI job. The
  toolchain half can begin before A1; the CI gate lands last. Size **L–XL**.

### Area B — WASM target

Ordering: **B1 (spike/design) → B2 + B3**.

- **B1. WASM feasibility spike + subset definition.** Decide target
  (`wasm32-wasi` for a CLI/playground backend vs `wasm32-unknown-unknown` for the
  browser), and the v1 subset: almost certainly **single-threaded, no coroutines/
  signals/netpoller initially** (no `ucontext`, no pthreads, no epoll in a stock
  WASM sandbox). Output: a short feasibility doc + the explicit "what works / what
  is stubbed" matrix. Size **M**.
- **B2. WASM codegen + GC strategy.** Thread a `wasm32-*` triple through codegen
  (LLVM has a WASM backend) and solve GC: **Boehm does not work on WASM**, and
  precise immix assumes a native stack to scan. Likely a linear-memory bump
  allocator (leak-or-simple-collector) for v1, or an immix variant over linear
  memory. This is the hard part and couples to the collector design. Size
  **L–XL**. Depends on B1 and ideally on C1 (triple plumbing).
- **B3. WASM runtime subset.** A single-threaded zaprt subset: synchronous
  scheduler (no M:N), coroutines either stubbed, trapped, or compiled via
  Asyncify/stack-switching (future). WASI shims for `io`/`fs`/`time`. Size **L**.
  Depends on B2.

### Area C — Cross-compilation + ARM-Linux

- **C1. Cross-compilation story.** Thread an explicit target triple through
  `TargetMachine` (today: host default only), add a CLI `--target`, and select
  the right linker/sysroot/libs per target. This is the enabling plumbing for
  WASM (B2), ARM, and multi-platform release artifacts (E2). Size **M**.
- **C2. ARM-Linux (aarch64) validation + CI.** epoll and pthreads are
  arch-independent, but the `ucontext` + swapcontext-from-signal-handler
  preemption path (I#172) carries an explicit x86_64 note — must be validated on
  aarch64. Add an aarch64 CI job (native ARM runner or QEMU). Immix offset math
  and any SIMD (I#139) also need checking. Size **M**. Independent of A/B; can run
  early as a cheap confidence-builder.

### Area D — Language + stdlib spec freeze & semver policy

- **D1. Spec freeze audit.** Reconcile `SPEC.md` (0.2 draft) with the shipped
  implementation; classify every language and stdlib surface as **stable**,
  **experimental**, or **internal**; drive `OPEN-QUESTIONS.md` to zero for
  anything that would be a stable surface. Produces the authoritative 1.0 surface
  list. Size **M–L**. Depends on nothing technical; depends on D2 defining the
  tiers.
- **D2. Semver + stability policy.** A policy doc + DECISIONS entry: what semver
  means pre-1.0 and at 1.0, the stable/experimental tiering and how a surface
  graduates, breaking-change and deprecation rules, and whether Zap adopts an
  edition mechanism. **This is the contract the word "public 1.0" actually
  refers to.** Size **M** (pure policy, zero platform risk). Blocks D1, E1.

### Area E — Release engineering

- **E1. Versioned releases + changelog process.** Git tag/release conventions, a
  `CHANGELOG.md` (keep-a-changelog), and how the workspace `version` bumps.
  Lightweight and independent. Size **S–M**. Depends on D2 (so "what's a breaking
  release" is defined).
- **E2. Prebuilt binaries + installers.** A release CI workflow producing
  artifacts per platform (linux-x64, linux-arm64, macOS, Windows) + an install
  path (shell installer / Homebrew / Scoop). **Depends on the platform work it
  ships (A5, C1/C2) and on E1.** Size **L**.

### Area F — Community infrastructure

- **F1. Community files.** `CONTRIBUTING.md` (build, CI gates, the design-doc +
  DECISIONS workflow, the coroutine-not-goroutine terminology rule),
  `CODE_OF_CONDUCT.md` (Contributor Covenant), `SECURITY.md` (disclosure
  contact/process), and `.github/ISSUE_TEMPLATE` + `PULL_REQUEST_TEMPLATE.md`.
  **Zero dependencies, ships today**, and is a prerequisite for accepting outside
  contributions. Size **S**.

## Dependency / sequencing map

```
F1  ───────────────────────────────────────────────►  (ship now, standalone)
D2 ──► D1
D2 ──► E1 ──┐
C1 ─────────┼──────────────────────────► E2  (release artifacts)
A1 ──► A2   │                              ▲
A1 ──► A3   ├─ A5(CI) ─────────────────────┤
A1 ──► A4   │                              │
A5(toolchain) ─(can start early)           │
C2 (ARM) ───────────────────────────────── ┘
B1 ──► B2 ──► B3      (WASM; B2 wants C1)
```

Critical path to "installable 1.0 on the platforms that matter": **D2 → D1**
(the contract) in parallel with **A1 → A5 → E2** (Windows is the long pole).
WASM (B) and ARM (C2) are valuable but not launch-blocking and can trail.

## Recommendation — the first concrete slice

The orchestrator framed the first-slice choice as *Windows IOCP netpoller* vs
*spec-freeze/semver*. My recommendation reframes both halves:

1. **Start with the semver + stability policy (D2), paired with community infra
   (F1), as the first implemented slice.** Launch readiness is primarily a
   *commitment* problem, not a coding problem: "make it public / call it 1.0"
   is a promise about stability, and you cannot responsibly ship installers or
   label anything 1.0 before defining what stable means, what can still break,
   and how surfaces graduate. D2 is **M-sized, pure policy, zero platform risk,
   and unblocks D1 and E1**; F1 is **S-sized and ships today**. Highest leverage
   per unit effort, and nothing downstream is wasted if platform priorities
   shift.

2. **The IOCP netpoller is the *wrong* first Windows slice.** On Windows the
   foundational blocker is **A1 (coroutine context switch)** — IOCP (A2),
   preemption/signals (A3), and everything else are dead on arrival until a
   coroutine can switch stacks at all, because the netpoller's whole job is to
   *park and resume coroutines*. So the first *platform-engineering* slice should
   be **A1 (Fibers/asm context switch)**, scoped and spiked in parallel with the
   process track — not A2.

Concretely: implement **D2 + F1 now**; kick off **A1 as a design/spike** (its GC-
interaction questions need answering before any Windows code is committed); let
**C2 (ARM CI)** run as a cheap parallel confidence-builder. Hold A2/A3/A4/A5,
B*, and E2 until A1's approach is chosen.

## Open issues

- **A1/GC:** if Windows uses native Fibers, each fiber owns its stack — how does
  the collector scan a suspended coroutine's stack + saved context, under both
  boehm and precise immix, and how does I#120 evacuation interact? (May favor a
  hand-written asm switch over OS fibers to keep the "green stack is a GC heap
  object" invariant.)
- **A2:** can IOCP's completion model be adapted to the readiness-based
  `zap_netpoll_wait` contract via AFD/`RIO`, or does the netpoller abstraction
  itself need a completion-model variant?
- **A3/A5 unwinding:** the runtime currently relies on the Itanium `__cxa_*`
  personality via `-lstdc++`; Windows uses SEH. How much of the I#22/I#40
  exception machinery and the I#47 spawn landing pad must change?
- **A5 deps:** acceptable Windows dependency-sourcing story for pcre2/zlib/
  OpenSSL/libgc/CPython — vcpkg, bundling, or making some modules optional?
- **B2:** GC on WASM — bump-and-leak for v1, a real collector over linear memory,
  or defer WASM until the collector story is settled?
- **D1:** which currently-shipped surfaces are we willing to freeze as stable vs
  explicitly mark experimental for 1.0 (e.g. `threading` is already flagged
  migration-only; Python interop; SIMD/arrays)?
- **Scope:** are Windows AND WASM both 1.0-blocking, or is 1.0 "Linux + macOS +
  Windows" with WASM as a fast-follow? This decision reshapes the critical path.
- **Org move:** does the public-launch prep (community files, release URLs)
  wait for the `parveshgarg/zap` → `zaplang-io` transfer, or ship under the
  current repo and redirect?
