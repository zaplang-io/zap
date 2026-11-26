---
title: Type-check enum instance-method call arguments
status: Locked
issue: 1642
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Type-check enum instance-method call arguments

**Status:** Locked

## Decision

An enum **instance-method** call `recv.method(args)` type-checks its arguments:
each positional/keyword argument must be `coercible` to the declared parameter
type, and a generic method enforces type-parameter consistency across its
arguments (#1622). Both a plain argument mismatch and a generic inconsistency are
rejected with `ZAP4021` before lowering.

```zap
enum Color:
    Red
    Green

    def plain(self, int64 n) -> int64:
        return n

    def take[T](self, T x, T y) -> T:
        return x

def main():
    auto c = Color.Red
    c.plain("not an int")   # error[ZAP4021]: argument 1 has type str, expected int64
    c.take(1, "hello")      # error[ZAP4021]: `T` is bound to int, "hello" is str
```

Previously an enum instance-method call was typed through a separate, fully loose
receiver path (`struct_info` deliberately excludes enums, so the struct
instance-method path never ran for them) and fell to the `Unknown` fallback: its
arguments were inferred with **no** type check at all. `c.plain("not an int")`
compiled, and `c.take(1, "hello")` additionally monomorphized `Color__take$int64`
with a string in an `i64` slot — the same miscompile #1622 fixed for struct
methods.

## Mechanism

The enum instance-method call path (receiver `bt` is a `Ty::Named` registered in
`self.enums`) now resolves the method signature from `EnumInfo.methods` and checks
the call, with the implicit `self` parameter (`sig.params[0]`) dropped so a
call's positional arguments map to the remaining parameters by index:

- **Per-argument type check.** A positional argument `i` is checked `coercible`
  against `sig.params[i + 1]`; a keyword argument is checked against its named
  parameter via `check_keyword_arg`. A failure is `ZAP4021` (keyword: `ZAP4022`),
  matching the free-function checker.
- **Generic consistency.** `check_generic_arg_consistency` runs keyed
  `"Enum.method"` over the inferred argument types. `record_generic_method`
  already records enum methods' generic surface (PR #1629, #1622), with `self`
  dropped, so a call's positional arguments line up with the stored parameters.

Arity stays **loose** — an extra or missing argument is not flagged here, and the
variadic / `**kwargs` collector parameter is never treated as a fixed positional —
matching the struct instance-method path and the other receiver paths. A
receiverless `def` called on an instance is still an associated-function misuse
(`ZAP4085`), and a `@deprecated` enum method still warns, both mirroring the
struct path. An unknown method keeps the loose `Unknown` fallback.

### Generator enum methods

A tier-3 (data-carrying) enum method may be a generator (I#449). `register_generators`
previously rewrote only the *struct* method table's return type to the synthetic
iterator `$gen:Enum__method`; the enum method table kept the method's declared
(unit) return, which the old loose path never read. Now that the enum call path
reads the return type straight off `EnumInfo.methods`, `register_generators` also
rewrites the enum method table, so `Box.Hold(10).nums(5)` still types as an
iterator and drops into `for`/comprehensions.

## Scope and the struct/enum asymmetry

- **Generic type-parameter consistency** on enum methods completes the enum
  follow-up #1622 explicitly deferred (that ADR recorded the generic surface for
  enum methods but left the enum call path not consulting it).
- **Per-argument `coercible`** is enforced here for enum instance methods. The
  struct instance-method path does **not** currently do a per-argument `coercible`
  check — it runs only the generic-consistency pass and otherwise leaves arguments
  loose (to preserve the method-chain operator typing that path drives `&`/`|`/`~`
  dispatch from). So after this change an enum instance method rejects a plain
  argument mismatch that the equivalent struct method still accepts. This matches
  issue #1642's stated expectation (`c.plain("not an int")` must be `ZAP4021`), but
  the asymmetry is intentional-for-now and tracked: bringing struct instance
  methods to the same per-argument checking is a follow-up, kept separate here
  because the struct path feeds method-chain operator typing and is the subject of
  the sibling return-binding change (#1643).

## Consequences

- `crates/sema/src/lib.rs`: the method-call path gains an enum-receiver branch
  that resolves `EnumInfo.methods`, checks each argument (`coercible` /
  `check_keyword_arg`), and runs `check_generic_arg_consistency("Enum.method", …)`;
  `register_generators` gains an `Item::Enum` arm rewriting generator enum
  methods' return type to their synthetic iterator.
- An enum instance-method call with a mismatched or generically-inconsistent
  argument is now `ZAP4021`, located at the offending argument; a consistent call
  and numeric widening still compile; arity stays loose.
- **Deferred:** per-argument `coercible` for struct instance methods (the
  struct/enum asymmetry above); binding a generic method's return type (#1643).

<!-- Related: #1622 (generic method consistency this extends to enums), #1545,
#1623 / #1643 (generic method return binding). -->
