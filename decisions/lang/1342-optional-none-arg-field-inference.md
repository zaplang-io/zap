---
title: A call argument is typed from the callee's parameter/field type
status: Locked
issue: 1342
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# A call argument is typed from the callee's parameter/field type

**Status:** Locked

## Decision

When the monomorphizer rewrites a call, each argument of a **non-generic**
callee is now rewritten with that callee's declared parameter/field type as its
expected-type sink, instead of with no expected type. Concretely
(`Mono::call_arg_expected`, consumed in `rewrite_call`):

- A bare-name **non-generic struct constructor** (`Box(..)`) supplies its field
  types, matched to arguments by position and by keyword name.
- A **non-generic free function** (`take(..)`) supplies its parameter types, up
  to (not including) the first variadic / `**kwargs` parameter.
- Everything else contributes no expected type and is unchanged: a **generic**
  callee (its parameter/field types may mention type parameters — left to the
  existing `instance_args` / `infer_fn_targs` inference, which already consumes
  the whole-call `expected`), method calls, enum-variant and indexed callees, a
  spread argument (`*x` / `**x`), and any argument on or past a variadic slot.

As a result a payload-less generic-enum variant argument (`Optional.None`) pins
its type argument from the callee, the way a `return` already does from the
function's return type.

## Rationale

`take(Optional.None)` and `Box(Optional.None)` failed to lower with `ZAP6001`
("cannot infer type argument 'T' of generic 'Optional'") even though the
parameter / field declared the exact `Optional[R]` instance. The monomorphizer
rewrote every argument with `expected = None` (`rewrite_call`), so a payload-less
`Optional.None` — which carries no value to infer `T` from — had nothing to pin
it. The same `Optional.None` already inferred in `return` position (which passes
`cur_ret`) and in a type-annotated binding (which passes the declared type), and
bare `None` already worked everywhere (it lowers through the null-sink path).
Only the call-argument and constructor-field positions were missing the sink.

The fix supplies the missing sink at those positions. It is scoped to
non-generic callees because their parameter/field types are concrete; a generic
callee's are not, and feeding an unsubstituted parameter type as an argument's
expected type would be wrong — those callees already infer their type arguments
from the argument types plus the whole-call expected, so they are left untouched.

Blast radius was measured before locking: lowering the entire `tests/golden` +
`tests/conformance` + `tests/diff` corpus (844 programs) through
`analyze_types → lower` produces the **same** diagnostics with and without this
change (zero new errors), and the full host suite (sema, mono, ir, driver) is
unchanged. The change only ever *adds* an expected type, so it can only turn a
previously-unpinnable argument into a pinned one (or surface a genuine
argument/parameter type conflict, of which the corpus has none).

## Consequences

- `Optional.None` (and any payload-less user generic-enum variant, e.g.
  `Maybe.Nothing`) lowers as a non-generic function argument and a non-generic
  struct-constructor field, by position or by keyword, for a non-first argument,
  and read back through the field. `Optional.Some(x)`, the `return` and
  annotated-binding positions, and bare `None` are unchanged.
- With no expected type available (e.g. `auto x = Optional.None`), the argument
  is genuinely un-annotatable and the `ZAP6001` "cannot infer type argument"
  diagnostic still fires — the fail-closed boundary is preserved.
- Generic callees, method calls, and variadic/spread positions are untouched; a
  method with an `Optional[T]` parameter taking a bare `Optional.None` is a
  possible follow-up, not addressed here.

<!-- Related: [lang/1169-mono-consumes-sema-types](1169-mono-consumes-sema-types.md), [lang/1329-for-loop-element-type-binding](1329-for-loop-element-type-binding.md), [lang/1337-sema-field-read-typing](1337-sema-field-read-typing.md). Found while fixing #1329/#1339. -->
