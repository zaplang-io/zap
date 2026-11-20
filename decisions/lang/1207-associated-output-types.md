---
title: Associated / output types on interfaces (`type Out`), projected as `T.Out`
status: Accepted
issue: 1207
pr:
date: 2026-11-20
domain: lang
supersedes:
superseded-by:
---

# Associated / output types on interfaces (`type Out`), projected as `T.Out`

**Status:** Accepted (implemented — sema realization; see Consequences for staging)

Third slice of the generic-interfaces epic #1204, the optional one
(`design/0012-generics.md` Fork B Deferred: "associated types"). Builds on
generic-interface conformance (#1205) and parameter-referencing bounds (#1206).
It is the cleaner long-term surface behind
[lang/1199-operator-capability-interfaces](1199-operator-capability-interfaces.md)'s
two-parameter `RMul[L, Out]` form; #1199 does **not** depend on it.

## Decision

An interface may declare an **associated type** with a bare `type Name` line in
its body, and reference it in its method signatures exactly like a type
parameter:

```zap
interface Producer[In]:
    type Out
    def make(self, In seed) -> Out
```

- **Binding is structural.** A concrete type conforms when it provides every
  method with the associated type replaced by one consistent concrete type — the
  associated type is *bound* to whatever the conformer's corresponding method
  returns (or takes). The associated type is not written by the conformer; it is
  read off its method set, exactly as the method parameter/return types already
  are for structural conformance (D-0016).
- **Projection `T.Out`.** Bounded generic code names the associated type of a
  bounded type parameter with a dotted **type projection** `T.Out` — which the
  grammar already parses as a dotted type name (the module-qualified-type path,
  I#283). `def build[T: Producer[f64]](T f, f64 s) -> T.Out: return f.make(s)`
  constrains `T` to a producer-from-`f64` and names the produced type without a
  second type parameter.

A single associated type bound by a single producing method is the MVP surface;
see Consequences for what is deferred.

## Rationale

- **No second type parameter.** The #1199 reflected capabilities use
  `RMul[L, Out]` because the result type otherwise has nowhere to live; an
  associated `type Out` lets the capability be `RMul[L]` and name its result,
  which is the Rust/Swift associated-type ergonomics.
- **Reuses machinery already built.** Binding is the #1205 structural
  substitution read in reverse (the associated type is the free variable the
  conformer pins); the projection reuses #1206's call-through-bound resolution
  and the dotted-name type the parser already produces — so no new AST type node
  and no new conformance algorithm.
- **Structural, not declared, binding** keeps interfaces structural end to end: a
  type never writes `type Out = …`; it simply has the methods, matching how Zap
  interfaces work everywhere else.

## Consequences

- Grammar: an interface body accepts `type Name` lines; `InterfaceDef` carries
  `assoc_types`. (`docs/GRAMMAR.ebnf` updated.)
- sema: `InterfaceInfo.assoc_types`; an interface method may name an associated
  type (kept as a type variable, like a generic parameter); a call through a
  bounded parameter whose interface method returns the associated type types as
  the projection `T.Out`; a `T.Out` annotation resolves to the same projection,
  so definition-site bodies type-check.
- mono: at instantiation the projection `T.Out` is resolved to the concrete
  argument's binding — the return type of the concrete method that the
  interface declares as the associated type's producer — and substituted into
  the function's signature before codegen (so codegen never sees a projection).
- **Deferred:** an associated type referenced by more than one method with a
  consistency check across them; associated types with a declared default or
  bound (`type Out: Display`); and migrating the prelude operator capabilities
  from `RMul[L, Out]` to `RMul[L]` + `type Out` (a separate follow-up, not
  required by #1199). Multiple bounds per parameter and where-clauses remain
  deferred from #1206.
- No change to `DECISIONS.md` (frozen); this ADR is the record.
