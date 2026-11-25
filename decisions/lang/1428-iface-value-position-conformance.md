---
title: "Generic-interface conformance enforced at value positions (Ty::Iface carries the instantiation)"
status: Accepted
issue: 1428
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Generic-interface conformance at value positions (#1428)

**Status:** Accepted — a dedicated `Ty::Iface(String, Vec<Ty>)` sema type variant
carries a generic interface's type arguments at value positions, so conformance is
checked against the real instantiation. Builds on #1205 (generic-interface
structural conformance, `conforms_with_args`) and #1382 phase 1 (the built-in
generic `Iterator[T]` / `Iterable[T]`). Unblocks #1382 phases 2–3 (`iter`/`next`)
and #1390 (`map`/`filter`). Part of epic #1387.

## The defect

Generic-interface conformance was **not enforced at value positions**. A
parameter / return / typed binding annotated `SomeIface[Arg]` resolved to
`Ty::Named("SomeIface")`, and `Ty::Named(String)` carries **no** type arguments,
so `[Arg]` was dropped at resolution. Argument binding compared the argument's
`Ty` against the parameter's resolved `Ty` via `coercible`, whose interface arm
called `self.conforms(s, i)` = `conforms_with_args(s, i, &[])` — with **empty**
type arguments.

With empty arguments a generic interface's method signature keeps its parameter /
return as the unsubstituted type variable (`Ty::Named("E")`), which no concrete
conformer matches. So a struct that *does* conform at the right instantiation was
wrongly rejected:

```zap
interface Container[E]:
    def get(self) -> E

struct IntBox:
    int64 v
    def get(self) -> int64:
        return self.v

def take(Container[int64] c):   # value-position generic-interface annotation
    print(1)

def main():
    take(IntBox())              # rejected before this change, though IntBox
                                # conforms to Container[int64]
```

The structural machinery already existed and was exercised by unit tests
(`conforms_with_args("IntBox", "Container", &[Ty::Int64])` is `Ok`); nothing
threaded the type arguments from a value-position annotation to it. The limitation
affected **every** generic interface (the #1205 family), not just the Iterator
protocol.

## Decision: option 1 — a dedicated `Ty::Iface` variant

The locked choice (maintainer, 2026-11-25) is **option 1**: a new sema type
variant `Ty::Iface(String, Vec<Ty>)` that carries the interface name and its
concrete type arguments.

The core reason over the lighter option 2 (resolve conformance against the AST
annotation at the binding site, keeping the declared `Type` alongside the resolved
`Ty`): an interface-typed **value** must carry its type arguments *onward*, so a
later `next(it)` / projection / `map` / `filter` can recover the exact element
type. Option 2 gives the binding site enough to check conformance once but leaves
the interface-typed value with no precise type to flow — which is the whole point
of unblocking #1382 phases 2–3. A dedicated variant (rather than extending the
pervasive `Ty::Named` to hold arguments, which would touch every `Ty::Named`
match) confines the churn to new match arms — the same manageable, CI-caught blast
radius as the just-shipped `tuple[T]` (`Ty::VarTuple`) variant.

## What changed

`crates/sema_types/src/lib.rs`

- New variant `Ty::Iface(String, Vec<Ty>)`, with arms in `display`
  (`Iface[A, B]`), `to_ast_type` (bridges back to `Type::Named { name, args }`),
  and `default_type` (element-wise over the arguments). `assignable` needs no new
  arm: the `from == to` fast path admits an identical instantiation and the
  `_ => false` tail rejects a differing one.

`crates/sema/src/lib.rs`

- `resolve_type` produces `Ty::Iface(name, resolved_args)` when a value-position
  annotation names a **generic interface** with arguments (distinguished from
  structs/enums via the interface registry); a bare (non-generic) interface or a
  struct/enum name stays `Ty::Named`.
- `coercible` gains `(Ty::Named(s), Ty::Iface(i, args)) => conforms_with_args(s,
  i, args).is_ok()`, so conformance is checked against the real instantiation. The
  existing collection-value arms (`list` / `dict` value) recurse through
  `coercible`, so a nested `list[Container[int64]]` element is enforced too.
- `subst_ty` substitutes inside the arguments, so a nested type variable
  (`Container[list[E]]`) is carried through conformance substitution.
- `conformance_detail`, `ty_mentions_interface`, and
  `verify_elem_expr_conformance` gain `Ty::Iface` arms, so a rejection names the
  instantiation (`Container[str]`) alongside the per-method reason, and a
  collection-literal element is re-verified against a generic-interface element.
- The argument (`ZAP4021` positional / `ZAP4022` keyword) and return-type
  (`ZAP4003`) mismatch diagnostics now append `conformance_detail`, so a
  value-position rejection names **both** the method and the instantiation
  (`let`/typed-binding diagnostics already did).

IR lowering (`crates/ir/src/lower.rs`) needs **nothing new**: `resolve_ty` works
on the AST `Type`, and its `other =>` arm already erases an interface annotation —
regardless of its arguments — to the interface's fat-pointer `Ty::Struct(fat_sid)`
(the `{data, itable}` layout, D-0016). `to_ast_type` reproduces the same
`Type::Named { name, args }`, so an interface-typed value flows to the identical
representation. The variant is consumed only by sema and (via `to_ast_type`) by
monomorphization; codegen never sees a sema `Ty`, so there is no codegen match
churn.

## Scope

- Value-position conformance passes the instantiation's type arguments to
  `conforms_with_args` at argument binding, typed-`let` / binding assignment, and
  return-type checking.
- A refined **generic** interface requires its (non-generic) supertype's methods
  too (D-0139/#806). A *generic supertype*'s arguments are still not substituted
  across the refinement edge — unchanged from #1205, and moot while the refined
  capability supertypes are non-generic; a documented follow-up.
- Bare (no-argument) generic-interface *values* remain out of scope exactly as
  before: a value annotated with the bare interface name carries no arguments to
  substitute (bounds, not bare values, are the realized surface for generics).

## Tests

- `crates/sema/src/lib.rs` (`iface_value_*`): argument / return / variable
  positions accept the right instantiation and reject the wrong one; a nested type
  variable (`-> list[E]`); a two-parameter interface (per-parameter, swapped
  rejects); a refined generic interface; a collection-value-position element; and
  the negative diagnostics name the method and the instantiation.
- `crates/driver/tests/error_codes.rs` (`generic_interface_value_position_codes`):
  the end-to-end `check_source` path accepts `Container[int64]` and pins the
  wrong-instantiation rejection to `ZAP4021`.
