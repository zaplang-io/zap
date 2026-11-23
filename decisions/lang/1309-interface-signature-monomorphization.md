---
title: Monomorphization rewrites interface method signatures
status: Locked
issue: 1309
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# Monomorphization rewrites interface method signatures

**Status:** Locked

## Decision

Monomorphization now rewrites the parameter and return type annotations of a
**non-generic** interface's method signatures through the same `rewrite_type`
path a struct field or a function signature uses: a generic instance named there
(`Optional[int64]`) is renamed to its mangled concrete declaration
(`Optional$int64`) and that instance is requested onto the instantiation
worklist. A generic interface (one with its own type parameters) is still emitted
verbatim, because its signatures mention the interface's own parameters, which
`rewrite_type` cannot distinguish from a nullary generic to instantiate.

As a result, the ZIR interface-metadata builder resolves each method's slot
signature to the concrete instance type, so the value returned by a dynamic
interface call (`reader.end_time()`) keeps its instantiated `Optional[int64]`
struct type and a method on that value (`…​.is_some()`, directly or through an
`auto` binding) lowers.

## Rationale

Before this, mono's top-level pass handled `struct`, `enum`, `function`, and
module-global `let` items but let `Item::Interface` fall through its catch-all
`other => out.push(other.clone())` arm — the interface was emitted **unchanged**.
IR lowering then resolved the method's un-mangled `Optional[int64]` annotation
through `resolve_ty`'s final `Named` branch, which looks up only the base name
and *ignores the type arguments*; with no concrete `Optional` declaration under
that bare name the return resolved to `Void`, so a chained method call saw a
non-struct receiver and failed with `error[ZAP6001]: method call on a non-struct
value`. Sema had already accepted the program and the equivalent ordinary
function (`zap ir`) lowered — only the interface slot signature lacked the
rewrite every other signature position already received.

Rewriting the signatures (rather than special-casing `Optional` in `resolve_ty`,
or teaching the interface builder to mangle on the fly) keeps a single source of
truth: the instance renaming and the worklist request are exactly what structs
and functions already do, so any generic type usable as a struct field is now
usable as an interface method's parameter/return with no further work, and the
instance is guaranteed to be emitted.

Restricting the rewrite to non-generic interfaces is deliberate: a generic
interface's method signature names the interface's own type parameters (`T`),
which are not instances to mangle; rewriting one would request a bogus
`Optional$T`-style instance. Generic interfaces therefore keep their prior
behavior, so the change is a pure augmentation on the concrete case that #1309
exercises.

## Consequences

- An interface method may freely return or accept a monomorphized generic
  (`Optional[T]`, a user `Box[int64]`, `Optional[list[int64]]`, …); the result of
  a dynamic (fat-pointer / itable) interface call keeps that concrete type, and
  inherited refined-supertype slots (#830/D-0139) carry it too.
- No new ZIR `Inst`/`Elem`/`Expr`/`Ty` variant and no new exhaustive-match arm:
  the fix is entirely in the mono AST→AST pass (`crates/mono/src/lib.rs`), so
  there is no new GC-rooting (`inst_dest`) entry and no runtime change.
- Out of scope / unchanged: generic interfaces (passed through verbatim, as
  before) and the structural-conformance / bound-satisfaction checks, which are
  untouched. A generic *parameterized* interface returning a concrete generic
  instance is a separate follow-up if it arises.
- This is the interface-return sibling of the test-harness inference gaps
  (#1308/#1310): those lose sema's `TypeMap` in the native `@test` runner, a
  different mechanism, fixed separately.

<!-- Related: the mono signature rewrite lives in crates/mono/src/lib.rs
     `rewrite_interface_nongeneric` / `rewrite_type`; the ZIR interface builder
     and the failing receiver check are in crates/ir/src/lower.rs. The sema
     TypeMap backfill this does NOT rely on is decisions/lang/1169-mono-consumes-sema-types.md. -->
