---
title: Null reference dereference raises a catchable NullReferenceError
status: Locked
issue: 1489
pr: 1500
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `NullReferenceError` for a null (non-`Optional`) reference dereference

**Status:** Locked (maintainer locked the gate 2026-11-25; explicit-check-primary
recommendation adopted). This is the foundational Epic ADR for #1489; it ships
with the #1492 PR.

## Decision (the three gated questions)

### Q1. Primary mechanism — **explicit null-check is the primary, shipping mechanism; the hardware trap (Option A) is a later, opt-in, x86-64-first optimization, not the v1 primary.**

Invert the Epic's assumption. Ship the guarantee on *every* target via an
explicit null-check + branch-to-raise inserted at each in-scope non-`Optional`
reference-deref site, lowered so the raise is a `throwing_call` (`invoke`) that
reaches a landing pad — the same mechanism already proven for built-in faults in
#383/#741/#1319/#1320. The hardware trap stays a *possible future fast path* that
must (a) target x86-64 first, (b) be proven GC-safe under precise Immix, and
(c) **fall back to the explicit check** anywhere the trap cannot be made precise.
Observable semantics are identical under either, so Option A can be added later
without a language change.

The Epic's own hedge already names the explicit check as "the correctness floor
behind the trap." This recommendation keeps that floor and makes it the v1
*ceiling* too, demoting the trap from "primary mechanism" to "optimization."

### Q2. Exception identity and hierarchy

- New built-in type **`NullReferenceError`**, `ZAP_EXC_NULL_REFERENCE_ERROR = 22`,
  **parent `Exception`** (flat, not under a `RuntimeError` bucket).
- Catchable by `except NullReferenceError` and `except Exception`.
- Message shape: a **located** string; the source location is **required**, the
  field name and owning static type are included when known:
  `null dereference reading field 'name' of 'Node' at foo.zap:7`.

### Q3. "What counts as a null deref" surface

Plain **non-`Optional`** reference derefs only. `Optional[T]` keeps its existing
`None`/unwrap codegen untouched and at zero overhead — no new per-deref branch
there. In-scope sites:

| Site | Example | Lowering today |
|------|---------|----------------|
| field read | `n.name` | `Inst::FieldGet` (GEP+load) |
| field store | `n.name = x` | field store (GEP+store) |
| method-call receiver | `n.method()` | receiver load before call |
| index base | `n[i]` | index-get on the base |
| for-iteration base | `for x in n:` | iterator setup on the base |
| builtin on a null ref | `len(n.name)` | reduces to a field read above |

Out of scope: `Optional[T]` unwrap (own path), valid-object index/arith (own
exceptions), a genuine wild pointer / use-after-free (hard abort — see Q1 trap
heuristic). No program that compiles today changes accept/reject status; a crash
becomes a catchable exception.

## Rationale

### Why explicit-check primary, not the trap (the hard part)

The trap (Option A) is **not unsafe in principle** — the design in #1494 is the
standard JVM/Go/.NET implicit-null-check trick: never unwind inside the signal
handler; instead rewrite the faulting `ucontext` PC to a trampoline that throws
in *normal* context. `_Unwind_RaiseException`/`__cxa_throw` are not
async-signal-safe (unwind-table lookup takes locks, may allocate), so the
in-normal-context resume is what keeps it legal. The existing fault handler
(`zap_fault_handler`, zaprt.c) already runs on a `sigaltstack` with
`SA_SIGINFO`, already has `info->si_addr` and the offending `G`/`M`, and already
ignores `uctx` — it is a ready hook point. So the mechanism is feasible.

It is nonetheless the **wrong thing to make the v1 primary**, for four reasons in
descending weight:

1. **Precise-Immix statepoint hazard.** The explicit check raises from a *call
   site*, which is a GC statepoint with a stackmap. The trap injects a throw at
   an *arbitrary faulting load PC*, where LLVM has emitted **no statepoint /
   stackmap** — so precise Immix may be unable to find roots in the faulting
   frame. This is the exact failure class the repo has been bitten by before
   (NULL-reload-under-Immix, #839; the `inst_dest` GC-rooting rule), and it only
   manifests under the LLVM-18 + gc-immix CI job — the hardest gate to run
   (local Docker kernel-panics; CI is the sole gate). Putting the *primary*
   safety mechanism in the one place hardest to validate is the wrong bet.
   Under Boehm (conservative, whole-stack scan) the trap is fine; under the
   default precise collector it is the fragile path.

2. **Two of three legs are hardware-HELD regardless of the gate.** A
   language-wide guarantee needs x86-64 (#1494) **and** aarch64 (#1495,
   `hardware dependency`) **and** Windows VEH (#1496, `hardware dependency`).
   Per house rules #1495/#1496 are HELD pending hardware *independent of this
   decision*. So the trap-primary path literally cannot deliver the guarantee on
   all targets now. The explicit check delivers it on every target today, in
   portable codegen, with no per-arch `ucontext`/VEH work.

3. **The "near-zero happy-path cost" advantage is overstated.** The trap's
   *branch* cost is zero, true — but #1493 requires codegen to stop LLVM
   eliminating/hoisting/speculating the deref and to withhold
   `nonnull`/`dereferenceable`. Inhibiting those enables-optimizations costs
   (LICM, CSE, vectorization on reference-heavy hot loops) is a diffuse,
   hard-to-measure pessimization. The explicit check costs a `test reg,reg; jz`
   that a modern predictor handles essentially for free and that often beats the
   lost vectorization. The headline reason to accept the trap's risk is thin.
   (Codegen today does **not** broadly emit `nonnull`/`dereferenceable` on these
   loads — only a `zap_ptr_nonnull` runtime helper and enum locals exist — so
   LLVM is not currently told these are non-null; #1493 is still needed to pin
   the loads against deletion/hoisting under either mechanism for the trap, and
   to insert the branch for the check.)

4. **#1492 + #1493 already deliver the whole guarantee.** With the type defined
   (#1492) and deref sites lowered with an explicit check to a `throwing_call`
   (#1493), `try/except NullReferenceError` works on all targets — no #1494
   handler required. That collapses the critical path and removes the
   signal-handler, ucontext-rewrite, and per-arch unwind risk from the v1
   guarantee entirely.

**Signal-safety verdict:** the trap is *implementable safely* (normal-context
resume trampoline), but it is *not the safe choice for the primary mechanism*
under precise Immix, and it cannot be portable now. Recommend explicit-check
primary; trap as a later x86-64 optimization with a mandatory explicit-check
fallback and an Immix-stackmap proof as its merge gate.

### Why `NullReferenceError` ⊂ `Exception` (flat)

Zap's hierarchy is CPython-shaped (#882/#1278/#1432/#1311). `RuntimeError` in Zap
(`ZAP_EXC_RUNTIME_ERROR = 10`) sits **flat under `Exception`** and is used as an
intermediate bucket for nothing. CPython's analogue (`None.attr` →
`AttributeError`) is also flat under `Exception`, with no `RuntimeError` layer.
Java/C# park NPE under `RuntimeException`/`SystemException`, but Zap follows the
Python tree, so flat-under-`Exception` is the consistent placement and matches
what #1492 already drafts. The `is_a` walk and
`zap_exception_register_parent`/`zap_exception_parent` table take a one-line arm:
`case ZAP_EXC_NULL_REFERENCE_ERROR: return ZAP_EXC_EXCEPTION;`.

### Why this surface

The footgun class (#461/#564/#1262/#1367/#1184) is uniformly "a non-`Optional`
reference field left null, then *used*." The harm is the use, not the omission
(the `@scoped` deferred-init pattern legitimately leaves a field null until
`__enter__`), so the fix targets the deref, not construction — leaving #1467's
"missing required field" debate moot. `Optional[T]` is excluded because it
already has a typed None/unwrap path; adding a second mechanism there would
duplicate it and add overhead the Epic explicitly forbids.

## Consequences

- Re-shapes the Epic's dependency graph: **#1492 + #1493 become the complete v1
  guarantee** on all targets. #1494 is **rescoped** from "primary mechanism" to
  "x86-64 trap *optimization* with explicit-check fallback," and #1495/#1496
  become trap-only perf legs (still HELD by hardware) rather than correctness
  legs. #1497 conformance proves the guarantee via the explicit-check path and
  is **no longer gated on the x86-64 trap landing**.
- If the maintainer prefers Option A primary anyway: then #1494 must carry an
  explicit **precise-Immix GC-root / statepoint-at-fault-PC validation** as a
  merge gate (stress golden under gc-immix in CI), and must still land the
  explicit-check fallback first so no target regresses while the trap is being
  proven.

## Scope delivered by #1492 (this PR)

Type + hierarchy + message shape only — no deref codegen, no fault/signal
handler, no trap:

- `ZAP_EXC_NULL_REFERENCE_ERROR = 22` in the runtime exception enum, with the
  `zap_exception_parent` arm returning `ZAP_EXC_EXCEPTION` (flat).
- The bare name `NullReferenceError` is a built-in exception usable in a typed
  `except` clause and as a `raise` target (sema `BUILTINS` + `is_exception_type`,
  lowering `exception_type_id_name` → `22`), mirroring #1311's `OSError`
  wiring.
- The located message shape is defined once as a pure builder
  (`null_reference_error_message` over a `NullDerefSite` covering the five
  in-scope sites), which #1493 calls with the statically-known field/owner at
  each deref site.
- Catch behaviour (own type, `Exception`, re-raise type preservation,
  non-matching clause propagation) and message round-tripping are pinned by
  `.zap` goldens and sema/host unit tests.

## What the maintainer LOCKED before #1492/#1493/#1494 can start

1. **Primary mechanism:** explicit-check-primary (adopted) vs. trap-primary.
   This decides whether #1494/#1495/#1496 are correctness legs or optimizations,
   and whether #1497 is gated on the trap.
2. **Exception identity:** name `NullReferenceError`; parent `Exception` (flat);
   `ZAP_EXC_NULL_REFERENCE_ERROR = 22`; located message with field+type when
   known, location required.
3. **Scope surface:** plain non-`Optional` reference derefs only — field read,
   field store, method receiver, index base, for-iteration — with `Optional[T]`
   untouched/zero-overhead.

Sub-issues #1495 (aarch64) and #1496 (Windows VEH) remain **HELD pending
hardware** per house rules regardless of the gate outcome.

Related: #1489 (Epic), #1467, #461/#564/#1262/#1367/#1184 (footgun class),
#741/#1319/#1320 (uncatchable→catchable precedent), #383 (throwing_call pattern),
#882/#1278/#1432/#1311 (exception hierarchy), #841 (EH/LTO -fexceptions),
#869/#223 (aarch64 ucontext/-O2), #839 (Immix NULL-reload class).
