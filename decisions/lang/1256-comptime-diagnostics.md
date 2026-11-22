---
title: comptime.error + derive/instantiation backtrace diagnostics (E2)
status: Accepted
issue: 1256
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# comptime.error + derive/instantiation backtrace diagnostics (E2)

**Status:** Accepted — records the *implemented shape* of issue #1256 (epic
#1249's HQ4 diagnostics layer), the last engine issue of E2. Builds on
#1250 (the evaluator raises the errors), #1251 (unroller), #1253 (kind-switch),
and reuses the stable `ZAPnnnn` code scheme (#239) and the line/col + caret
renderer (#571).

## Context

The comptime engine (`crates/comptime`, epic #1249) already fails cleanly: every
fault is a span-attributed `CtError`, never a panic (#1250, decision 2b). HQ4
asks for the layer that turns such a failure into the diagnostic a programmer
reads, and gives a library author a way to author their own. The hard
constraint (maintainer): the authoring-tier tolerance for `comptime`/`reflect`
applies **only** to the authoring side — it does **not** relax the
*derive-consumer* message. The ordinary programmer who writes `@derive(Json)`
must see an average-programmer-grade error pointing at **their own field**,
never a comptime-internal symbol.

## Decision

### 1. `comptime.error(at, msg)` — the library-author abort primitive

A new evaluator builtin (the Zig `@compileError` model): `comptime.error(msg)`
aborts comptime evaluation pointing at the call site; `comptime.error(at, msg)`
points at `at`, a `Span` obtained from a reflection record's new **`.span`**
accessor (`field.span`, `variant.span`, `attr.span`). It always aborts — never
returns — with a `CtError` of the new kind `CtErrorKind::UserError`: *not* a
fault in the fragment (the code ran correctly and chose to fail), so it renders
as the author's own text (`ZAP7002`), never the "not available at comptime"
wording. A new `Value::Span` carries the span; it has no runtime representation
(never materializes into emitted code), exactly like the other reflection
records.

### 2. A new const-eval error category: `ZAP7xxx`

The `ZAPnnnn` scheme (#239) reserved `ZAP7xxx–9xxx` for "future phases
(linker, **const-eval**, …)". Comptime evaluation *is* const-eval, so `ZAP7xxx`
is now the const-eval category (`Category::ConstEval`), with three codes:

- **`ZAP7001`** "comptime evaluation error" — an engine fault/cap
  (`NotAvailable` / `Value` / `Budget`).
- **`ZAP7002`** "error raised by comptime code" — a library `comptime.error`.
- **`ZAP7003`** "cannot derive trait for type" — the dedicated, consumer-grade
  derive-failure code (HQ4's "cannot derive X for T").

`ZAP8xxx–9xxx` stay reserved. Adding codes is a MINOR, backward-compatible
change (STABILITY.md); all three ship Experimental pre-1.0.

### 3. The derive / instantiation backtrace + rendering (`crate::diagnostics`)

- `DeriveBacktrace` is an ordered list of `Frame { label, span }`. Frames are
  stored in **descent order** (outermost `@derive` consumer first, the context
  nearest the failure last) — how a generator naturally descends through the
  unroller (#1251) and kind-switch (#1253).
- `render_ct_error` renders the primary error (coded via `error_code`, at its
  span, with the source line + caret), then one `required because of …` note
  per frame, walked **inner-first** (nearest the failure, climbing to the derive
  site) — Rust's "required because …" model.
- `render_derive_error` renders the **consumer-grade** header "cannot derive
  `X` for `T`" (`ZAP7003`) pointing at the user's offending field, with the
  backtrace and the root cause as notes. The header never names a
  comptime-internal symbol.
- `render_generator_fault` renders a **dual-span** diagnostic: a fault *inside*
  a generator cites both the generator line and the instantiation site
  ("instantiated here"), so a generator bug is a clean compile error, never a
  crash. The never-panic guarantee is the evaluator's (#1250); this layer only
  renders what it returns.

A reusable `render_note` is added to `zapc_diag::render` (a located note with no
error code) so the backtrace/dual-span notes reuse the one line-index + caret
implementation.

## Scope deferred (owned here)

- **No parser surface.** Like the rest of E2 (and E1 #1257), `comptime.error` /
  `.span` are host-driven over parsed/constructed AST; the `comptime`/`reflect`
  keyword surface is the deferred follow-up. GRAMMAR.ebnf untouched.
- **Backtrace wiring into a real generator** lands with E3 (`@derive`); E2 ships
  the data model + rendering, host-tested, the same split as #1255's
  `SpecializationCache`.
- **Multi-file / richer-label backtrace polish** continues into E7 (#1248); v1
  renders every frame against one compilation unit's source.

## Consequences

No behavior change for existing programs: compiler-internal, host-tested
(`crates/comptime/tests/diagnostics.rs`, 18 cases). `crates/comptime` gains a
dependency on the leaf crate `zapc_diag` (no cycle, still LLVM-free). The
`ZAP7xxx` range is now claimed for const-eval.
