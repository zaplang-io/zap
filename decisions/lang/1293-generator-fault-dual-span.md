---
title: "A generator bug renders as a dual-span fault, never a crash (E3d follow-up)"
status: Accepted
issue: 1293
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# Dual-span generator-fault diagnostics (E3d follow-up)

**Status:** Accepted — a follow-up to the E3d derive-diagnostics wiring
([1293-derive-diagnostics-wiring](1293-derive-diagnostics-wiring.md)), completing
the spec's "a generator bug is a clean compile error naming both places, never a
panic" case (#1293).

## Context

E3d routed a library-authored `comptime.error` abort to the consumer `ZAP7003`
("cannot derive `Trait` for `Type`") form. But an **engine fault** inside a
generator — a *bug in the library's own generator code*, e.g. an out-of-range
`reflect.field(value, comptime f.index + 100)` — is not the consumer's mistake,
and dressing it as "cannot derive" misattributes the blame. #1256 ships
`render_generator_fault` for exactly this: a dual-span form citing the generator
line and the derive ("instantiated here") site. E3d left it unused because the
instantiation span was not captured.

## Decision

Distinguish an engine fault from a user abort on the derive path and route it to
the dual-span form:

- `DeriveErrorInfo` carries an `instantiation_span: Option<Span>` — the derived
  type's declaration — set by `enrich_derive_error` from the active derive
  context while a generator is specialized.
- the driver, for a comptime failure whose cause is **not** a user
  `comptime.error` (`cause_is_user == false`) and that occurred under a derive
  (an instantiation span is present), renders through
  `zapc_comptime::render_generator_fault(src, file, &cause, generator_span,
  instantiation_span)` — the fault's own span is the generator line, and the
  type's declaration is "instantiated here". A `comptime.error` abort still takes
  the consumer `ZAP7003` form; a comptime failure outside a derive still renders
  as a plain comptime error.

## Result

```
error[ZAP7001]: error in comptime generator: comptime field index 100 is out of range for `User` (1 field(s))
 --> prog.zap:4:15
         print(reflect.field(value, comptime f.index + 100))
               ^~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
 = note: instantiated here
 --> prog.zap:10:1
     struct User:
     ^~~~~~~~~~~~
```

The compiler exits with a diagnostic naming both the library's buggy line and the
user's derive site, never a panic. Verified host-side (`zap ir`):
`crates/driver/tests/zap_cli.rs` (an out-of-range `reflect.field` under a derive
→ the dual-span form, asserting no panic and no "cannot derive" header) and a
`render_derive_failure` unit test (`src/lib.rs`).

## Scope

The remaining E3d follow-ups are unchanged: the multi-level `required because of`
chain for nested containers, and the rich render on the LLVM `build`/`run` path.
