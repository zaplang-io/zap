---
title: A receiverless def on a struct/enum is an associated function, called as Type.func(...)
status: Locked
issue: 1260
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# Associated functions on struct/enum types

**Status:** Locked

## Decision

Inside a `struct` (or `enum`) body, a `def` whose **first parameter is not
`self`** (including one that declares no parameters) is an **associated
function**. It is callable **only** as `Type.func(...)` — including through an
import, `pkg.Type.func(...)` — and never on an instance. A `def` whose first
parameter *is* `self` remains an **instance method**, callable only on a value
(`value.method(...)`). The two coexist on one type, distinguished solely by the
presence of `self`. There is **no `@staticmethod`/`@classmethod` decorator**.

The canonical use is named / alternate constructors returning the declaring
type:

```zap
struct Message:
    int64 id
    bytes payload

    def query(int64 id) -> Message:          # associated — no self
        return Message(id, b"")

    def from_wire(bytes data) -> Message:     # associated — alternate constructor
        return Message(int64(data[0]), data)

    def to_wire(self) -> bytes:               # instance — has self
        return self.payload

# Message.query(42) / Message.from_wire(b"..") construct; m.to_wire() consumes.
```

The call site is type-checked like a free-function call: arity, positional and
keyword argument types, and `**kwargs` are all validated against the method's
signature (whose parameters are the real parameters — there is no implicit
`self` to drop). An associated function need not return the declaring type.

## Rationale

The parser already accepted a receiverless `def` in a struct body; the gap was
sema/mono/codegen semantics. dns#3 (named constructors `Message.query` /
`Message.from_wire`, blocking dns PR #10) is the motivating use case, and the
generated-SDK precedent `Struct.from_json(...)` already established the
`Type.func(...)` call form.

- **Receiverless `def`, not a decorator.** Python needs `@staticmethod` (no
  receiver) and `@classmethod` (receives the class) because `self`/`cls` are
  positional conventions it cannot see syntactically. Zap's receiver is already
  **explicit** as `self`, so the compiler can tell an associated function from an
  instance method by looking at the first parameter — a decorator would restate
  what the signature already says. And `@classmethod` exists to pass the runtime
  **class object**; Zap is statically typed and monomorphized, the type name is
  known at the call site, and there is no runtime class object to receive, so a
  `cls` parameter would carry nothing. An encountered `@staticmethod` therefore
  stays an unknown decorator (ZAP5032); it is not wired to this feature.

- **Call-site interception, reusing the `Type.X(...)` precedent.** Resolution
  generalizes the existing `check_call` dispatch on the
  `Expr::Attr { base: Expr::Name(Type), name }` callee that already handles the
  D-0046 enum-variant construction `Shape.Circle(2.0)`, the derived decoders
  `Struct.from_json(...)` (D-0059) / `from_csv` / `from_yaml`, and
  `bytes.maketrans(...)`. The associated dispatch runs *first* among these, but
  only fires when `name` is a real **source** method of the type; a derived
  decoder (synthesized, not a source method) and an enum variant are not source
  methods, so they still fall through to their own handlers. The type name must
  not be shadowed by a value binding (then `Type.name` is an attribute read).

- **Imports need no special case.** modlink rewrites `pkg.Type` to the imported
  type's flat mangled name before sema and lowering, so `pkg.Type.func(...)`
  arrives as `Expr::Attr { base: Expr::Name(<prefix>Type), name: func }` and
  resolves through the identical path — mirroring how `shapes.Shape.Circle(...)`
  already works across modules.

- **Lowers as a namespaced free function, no new ZIR.** An associated function
  lowers to the same mangled symbol `Type__func` as any method, but with **no
  receiver**: the call site dispatches through the ordinary free-function call
  path (defaults, keyword reordering, variadic / `**kwargs` assembly all reused),
  and the function is emitted with `self_param == false` so its first parameter
  uses the by-value ABI instead of a method's by-reference receiver ABI. The one
  subtlety the implementation must honor is that the method metadata helpers
  (`method_param_info`, the `self_param` flag) unconditionally assume a leading
  `self`; an associated function keeps its **full** parameter list and is emitted
  without the receiver flag and without a tier-1-enum itable box thunk (it is
  never in an itable).

## Consequences

- New diagnostics (call-site, exact spans): **ZAP4085** — an associated function
  called on an instance (`m.query(..)`) → call it as `Message.query(...)`;
  **ZAP4086** — an instance method called without a receiver
  (`Message.to_wire()`) → call it as `<message>.to_wire()`; **ZAP4087** — a known
  type named with an unknown associated function (`Message.nope()`), replacing
  the misleading `ZAP3001 "unknown name 'Message'"` the value path used to emit.
  Wrong arity / argument type / keyword reuse the free-call errors (ZAP4020 /
  ZAP4021 / ZAP4022 / ZAP4023).

- The field constructor `Type(...)`, instance methods, the derived
  `from_json`/`from_csv`/`from_yaml` decoders, and D-0046 enum-variant
  construction are all preserved and coexist with associated functions on the
  same type.

- **Generic structs are out of scope for v1.** Resolution is through a bare type
  **name** (`Expr::Name`); a generic instantiation `Box[int64].make(...)` has an
  `Expr::Index` base and is not dispatched as an associated call. Associated
  functions on generic types (and the type-parameter substitution an associated
  constructor returning `Box[T]` would need) are a documented follow-up.

See also D-0046 (enum-variant construction `Type.Variant(...)`), D-0059 (the
`Struct.from_json` derived-decoder associated path), and dns#3 (the named-
constructor use case).
