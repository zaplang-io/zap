---
title: Monomorphization consumes sema's inferred types instead of re-deriving them
status: Locked
issue: 1169
pr:
date: 2026-11-20
domain: lang
supersedes:
superseded-by:
---

# Monomorphization consumes sema's inferred types instead of re-deriving them

**Status:** Locked

## Decision

When monomorphization (`zapc_mono`) needs the type of a generic constructor /
call argument to pin a type parameter and its own best-effort inferer
(`rewrite_expr`) cannot derive one, it falls back to the type **sema already
inferred** for that expression. Concretely:

- Sema's analysis mode records every expression's `(span, Ty)` (the I#228
  `AnalysisFacts.expr_types`); this is surfaced as a span-keyed `TypeMap`
  (`analyze_types`).
- The `TypeMap` and a `Ty → ast::Type` bridge (`Ty::to_ast_type`) live in
  `zapc_sema_types` — the leaf crate that both `zapc_sema` and
  `zapc_ir`/`zapc_mono` depend on — so monomorphization reads sema's inferred
  types as **data**, without depending on the sema *checker*.
- The driver produces the map once (`zapc_sema::analyze_types`, same diagnostics
  as `analyze`) and threads it through both lowering entry points: `zap ir`
  (`lower_module_opts_with_types`) and `zap build`/`run` (through
  `zapc_codegen::build_executable`). The REPL/JIT and the `@test` harness pass
  an empty map and keep the monomorphizer's own inference unchanged.
- `rewrite_expr` becomes a thin wrapper over `rewrite_expr_inner` that
  substitutes the sema type **only** when the inner result is `Unknown`. The
  per-form `CTy` arms are **kept as a fast path**, not retired.

## Rationale

A recurring class of `ZAP6001` ("cannot infer type argument 'T'") reports —
#1117 (unary), #1128 (binary), #1131 (subscript/slice/tuple/comprehension),
#1132 (built-in call returns), #1162 (module constants like `math.inf`) — all
had one root cause: `zapc_ir`/`zapc_mono` deliberately do not depend on sema, so
monomorphization re-derived each argument's type with a partial, hand-rolled
inferer. Every expression form sema already types had to be re-implemented
arm-by-arm, and each missing arm surfaced as `ZAP6001` on the flagship generic
`Optional`. The gap never converged — each new stdlib function or constant was a
fresh failure.

Three options were weighed (RFC: discussion #1164):

1. **Consume sema's inferred types** (this decision) — collapses the whole class
   at once and makes sema the single source of truth for argument types.
2. A **built-in signature table** the mono inferer consults — still a parallel
   re-derivation; closes #1132/#1162 but not the general case, and drifts from
   sema.
3. **Status quo** — keep adding arms and leaning on explicit `Optional[...]`
   annotations; never done, poor ergonomics.

Option 1 was chosen: it is the only one that stops the class recurring. The
per-arm fixes #1117/#1128/#1131 are superseded in spirit — they remain as a
**fast path** (and their regression tests as coverage) rather than being
deleted, because once sema backfills every gap the arms are a harmless
optimization and removing them is pure churn with real risk.

## Consequences

- **`zapc_ir`/`zapc_mono` gain a data-only dependency on `zapc_sema_types`**, not
  on the sema checker crate. The "ir does not depend on sema" invariant is
  refined to "ir does not depend on the sema checker's *logic*": it reads an
  inferred-type table. The "GC-correctness as a theorem" framing is intact —
  precise per-instance layout/descriptor derivation is independent of *how* a
  type argument was inferred.
- **Pure augmentation.** A type the mono arms already produce, and the bare-`None`
  sink, are untouched, so no previously-inferring site changes; an argument sema
  also cannot type stays `Unknown` and is still rejected (fail-closed).
- **Synthetic types are excluded from the bridge (#1190).** Sema types a bare
  generator expression as the shared synthetic iterator `$genexpr`, a generator
  function call as `$gen:<name>`, and carries interface fat-pointer layouts as
  `$iface:<name>` — all `$`-prefixed, unspellable, deliberately element-erased
  names (I#67/D-0048, #596). `Ty::to_ast_type` returns `None` for any `Named`
  whose name begins with `$`, so these never flow into monomorphization: without
  the guard the backfill minted a bogus `Optional$$genexpr` instance over the
  synthetic type (an untested, likely-unsound struct/enum layout) instead of the
  intended fail-closed `ZAP6001`. The guard is a narrow leading-`$` check, so a
  mangled instance name such as `Optional$int64` (whose `$` is internal) still
  bridges and the inference wins above are unaffected. Supporting a generator
  *inside* a generic is a separate, unimplemented design decision (what would
  `Optional[<generator>].unwrap()` yield, and is its layout sound?) and would
  need its own RFC.
- **Span-keyed.** The map is keyed by expression span. `with_prelude` preserves
  user-expression spans and the driver passes the same parsed module to sema and
  to lowering, so the keys line up. A span is only *locally* unique — multi-file
  span collisions are a known caveat; keying on a stable node id is a future
  hardening.
- Closes #1132 and #1162; resolves the inference half of the family. The
  dict-*comprehension* lowering gap (#1154) is separate (a lowering-protocol
  gap, not inference) and is unaffected.
- Delivered across #1165 (sema type map), #1166 (shared carrier + bridge),
  #1167 (mono/driver consumption). `DECISIONS.md` stays frozen; this ADR is the
  normative record, with discussion #1164 holding the debate.

<!-- Related: RFC discussion #1164; the inferer lives in
     crates/mono/src/lib.rs (`rewrite_expr`), the carrier + bridge in
     crates/sema_types/src/lib.rs (`TypeMap`, `Ty::to_ast_type`). -->
