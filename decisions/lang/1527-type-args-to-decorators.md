---
title: Concrete type arguments to library-defined decorators (first-class type values)
status: Locked
issue: 1527
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Concrete type arguments to library-defined decorators (first-class type values)

**Status:** Locked

## Decision

A **bare primitive type name in value position** (a call/decorator argument,
a binding, a `return`) is a **first-class type value** of type `Type[T]`, where
`T` is the named type. So a concrete type can be passed to a library-defined
decorator exactly as the approved Click-style CLI surface (zaplang-io/cli#3)
writes it:

```zap
import cli

@cli.command
@cli.option("--count", int64, default=1)
@cli.argument("name", str)
def greet(int64 count, str name):
    print(name)
```

`int64` and `str` here are type values, passed as ordinary positional arguments
and flowing through the nested-call desugar the stacked imported returning
decorators generate (#1509/#1519), alongside named metadata such as `default=`.
Before this change a bare type name in value position was `error[ZAP3001]:
unknown name 'int64'` (or, for the conversion-builtin names `str`/`bool`/…, an
unlowerable loose value) — so a library function could not receive a type.

This is **mechanism A** of the three the issue reserved:

- **A — first-class type values (chosen).** A type name evaluates to a value a
  library parameter receives.
- **B — `reflect.type_info(T)` at the call site (rejected).** The library would
  read the type from a comptime reflection call rather than a value argument;
  #1257 `TypeInfo` is comptime-only with no runtime form, so it cannot be a
  decorator argument that a decorator builds its `Command` from at run time.
- **C — type-in-subscript generic parameter (rejected).** Threading the type as
  an explicit generic type argument (`@cli.option[int64]("--count")`) is neither
  the Click-style surface the issue specifies nor able to carry the type as
  *data* the library stores on its command object.

### Scope — primitive type values only (v1)

The type values are the **primitive scalar** types, every spelling normalizing to
one canonical type: `int`/`int64`/`i64`, `int32`/`i32`, `uint8`/`u8`,
`uint16`/`u16`, `uint32`/`u32`, `uint64`/`u64`, `usize`, `float`/`float64`/`f64`,
`float32`/`f32`, `bool`, `str`, `bytes`, `char`, `bigint`, `complex` (the single
source is `sema_types::primitive_type_value_ty`). The container families
(`list`/`dict`/`set`/…) and **user struct/enum type values are deferred**: a bare
user-type name in value position is a genuine *type-vs-constructor* ambiguity
(`Command` could mean the type value or a reference to its constructor), which the
reproduction and cli#3 do not need. Resolving that ambiguity is a separate
decision, not guessed here.

### Runtime representation — a minimal `int64` type-id token

The central design question is how a type value is represented so a library can
distinguish `int64` from `str` from `bool`. The decision is **minimal**:

- **Statically** (sema) a type value is `Ty::TypeVal(T)`, carrying the
  represented type. Two type values are **invariant and exactly distinguishable**
  — `Type[int64]` is not assignable to `Type[str]`, and (unlike the values) a
  type value gets **no numeric widening**, so `Type[int32]` is not `Type[int64]`.
  A non-type argument in a `Type[T]` position is a precise type error (`ZAP4021`).
- **At run time** a type value erases to a **plain `int64` type-id constant** — a
  small stable token (`int64`=1, `int32`=2, `uint8`=3, `uint16`=4, `uint32`=5,
  `uint64`=6, `usize`=7, `float64`=8, `float32`=9, `bool`=10, `str`=11,
  `bytes`=12, `char`=13, `bigint`=14, `complex`=15). There is **no new IR type,
  no GC root, and no runtime reflection graph** — a full `TypeInfo`
  materialization is E2-sized (#1250, #1257) and explicitly out of scope. A
  `Type[T]` parameter lowers to an ordinary `i64` slot.

Keeping the represented type purely as a compile-time distinction (the `Type[T]`
parameter) while erasing to one scalar token at run time is what makes the
representation minimal yet statically rich. In v1 the token is **carried, not
inspected**: there is no user-facing builtin to read a type value's id back
(generic monomorphization on the library's own type parameter is how a library
specializes behavior per type). A reader builtin is a documented follow-up.

### Flow through the decorator desugar

No change to the #1509/#1519 desugar is needed. A type value is an ordinary
`Expr::Name` argument, so it rides the nested-call composition the stacked
imported decorators already build, and monomorphization binds a generic library
parameter from it (`option[T](str, Type[T] kind, …)` bound at `T = int64`, via
the existing `to_ast_type` → `unify` path).

## Rationale

- **Reuse `Expr::Name`, not a new AST/IR node.** A bare primitive type keyword
  already parses to `Expr::Name`, and sema/lowering give that node type-value
  meaning (a value use only — the `str(x)`/`int64(x)` conversion *call* forms are
  matched structurally in `check_call` before inference, so they are untouched).
  Every other pass treats it as the ordinary `Name` it already handles — no
  feature-gated exhaustive-match or GC-root churn. The runtime scalar means no
  new `ir_core::Ty`/`Inst` variant either, so none of the LLVM-only-CI failure
  modes apply.
- **Additive.** A bare primitive type name in value position was an error before
  (`ZAP3001`, or an `Unknown` that failed to lower with `ZAP6001`), so no
  runnable program changes behavior.
- **Minimal runtime, invariant static type.** A library needs to *distinguish*
  the types, not to walk their structure. One `int64` token distinguishes them;
  the richer `Type[T]` distinction lives at compile time where it costs nothing
  at run time and keeps a wrong `default=` against a *concretely* typed parameter
  a static error.

## Consequences

- `sema_types::Ty` gains `TypeVal(Box<Ty>)`; `display` (`Type[T]`), `to_ast_type`
  (bridges to the `Type[T]` annotation so mono binds `T`), and `assignable` (exact
  match + `Unknown` wildcard) cover it. `sema` resolves the `Type[T]` annotation
  and infers a bare primitive name as a type value. IR `resolve_ty` maps
  `Type[..]` to `i64` and lowers a type-value name to its id constant.
- **A non-type argument in a `Type[T]` position, and a type value of the wrong
  type, are rejected statically** (`ZAP4021`), the distinguishability the issue
  requires. A **concrete** `Type[int64]` parameter paired with an `int64`
  parameter also type-checks a mismatched `default=` (rejected).
- **Deferred:** auto-rejecting a mismatched `default=` for a *generic*
  `option[T](Type[T] kind, T default)` signature. Zap's generic type-parameter
  consistency is currently loose program-wide — `def take[T](T x, T y)` accepts
  `take(1, "hello")` today — so binding `T` from the type value and enforcing it
  against `T default` is a property of generic checking in general, orthogonal to
  type values, and is left to a separate change rather than worked around here.
  The surface still compiles and runs; it simply does not yet flag a bad generic
  default.
- **Deferred:** user struct/enum type values (the type-vs-constructor ambiguity),
  container type values, and a builtin to read a type value's id at run time.

<!-- Related: #1509, #1519 (the decorator-object-binding surface this builds on);
#1257 / epic #1248 (comptime reflection — the comptime `TypeInfo` this does NOT
materialize at run time); unblocks zaplang-io/cli#4. -->
