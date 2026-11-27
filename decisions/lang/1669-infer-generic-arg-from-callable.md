---
title: Infer a generic argument from a bare callable reference
status: Locked
issue: 1669
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# Infer a generic argument from a bare callable reference

**Status:** Locked

## Decision

A **bare top-level function name used in value position** carries that function's
complete callable type `fn(params) -> ret` (the first-class function value of
D-0067 / #163). The monomorphizer now uses that synthesized signature when
inferring a generic function's type parameters, so a generic call pins its
parameter from a function reference exactly as it already does from a typed local
binding:

```zap
def accept[Inner](Inner previous):
    print("called")

def callback(Context ctx, Namespace ns):
    print(ctx.name)

def main():
    accept(callback)        # Inner = fn(Context, Namespace) -> void  (was ZAP6001)
```

Previously the inferer saw a bare name only as a *variable*; a name that resolved
to a function instead became an untyped value, so a generic parameter typed as a
bare type variable (`Inner previous`) had nothing to unify against and lowering
failed with `ZAP6001: cannot infer type argument 'Inner' ...; write it explicitly
(I#161)`. The only workaround was an explicit typed local
(`fn(Context, Namespace) handler = callback; accept(handler)`).

## Scope

- Applies to any argument position of an ordinary generic **free-function** call,
  and to the compiler-synthesized decorator adapter the #1647 generic seam passes
  into an imported returning decorator (`__zm1_demo__command(greet$adapter, …)`) —
  the adapter is an ordinary top-level function, so its signature is synthesized
  the same way. Fixes the imported-decorator inference regression in #1669 and
  unblocks zaplang-io/cli#4 before the library's generic factory body runs.
- The **complete** signature is used — parameter types *and* the return type — so
  a parameter pinned only by the return-type sink (`accept[Inner](Inner) -> Inner`)
  resolves too.
- A **local binding shadows** a same-named function: the environment is consulted
  first, so `int64 value = 7; accept(value)` infers `Inner = int64`, not the
  `value()` function's type.
- **Excluded:** a *generic* function reference (no single concrete callable type to
  offer as a value) and a function with an un-annotated parameter (no type to
  form) both stay untyped — a genuinely un-pinnable parameter still reports
  ZAP6001, so the path never masks a real inference failure.

## Rationale

- A bare function name is already a first-class value (#163); giving it its true
  function type at the inference site is the minimal, non-special-cased fix — it
  adds information the inferer was discarding rather than a new rule.
- The fix lives in the monomorphizer's own best-effort inferer (not sema's
  `TypeMap`), so it holds wherever lowering runs, including host-only builds and
  the imported-package path that has no local sema context to lean on.
- Dynamic invocation by runtime function value and reflection over function shapes
  are explicitly **not** required (function shapes stay opaque under the E1
  reflection ADR, #1257); this is purely compile-time type-directed inference.

## Consequences

- `crates/mono/src/lib.rs`: `rewrite_expr_inner`'s `Expr::Name` arm falls back to a
  new `fn_value_type(name, span)` — the function's `fn(params) -> ret` built from
  its declared parameter and return types — when the name is not a local variable;
  `unify` already handles `Type::Func`, so `infer_fn_targs` pins the parameter with
  no further change.
- Independent of normalizing the resulting specialized inputs (#1668) and of the
  omitted-decorator-name follow-up (#1665).

<!-- Related: lang/1647-uniform-generic-seam.md, #163/D-0067 (first-class function
values), #1257 (E1 reflection: opaque function shapes), #1668, zaplang-io/cli#4. -->
