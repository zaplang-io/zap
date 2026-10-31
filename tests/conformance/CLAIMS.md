# SPEC.md conformance claim index (I#376, epic #374)

This is the backbone of the conformance suite: a checklist that maps the
**normative claims of [`SPEC.md`](../../SPEC.md)** to executable conformance
cases. Each claim has a stable id (`AREA-N`); each row is either

* **covered** — pinned by the named case in this directory
  (`<case>.zap` + `<case>.expected`); the case's `# claim:` directive cites the
  same id, and [`conformance.rs`](../../crates/driver/tests/conformance.rs)
  enforces that this index and the fixtures mirror each other exactly; or
* **TODO** — a real normative claim not yet pinned. This is expected: this first
  cut deliberately covers the semantic danger zones (truthiness, boolean
  short-circuit + value, numeric `//`/`%`/overflow, `str()` rendering, chained
  comparison, dict `get`/iteration, built-in-fault catchability, tuples,
  function args) and their neighbors. **Broader SPEC.md coverage is incremental
  follow-up** — add a case, flip the row to `covered`, point it at the case.

A few TODO rows are flagged **(impl gap …)**: SPEC.md specifies the behavior but
the implementation does not match it yet (the divergences the #375 differential
tester found). They are tracked here as deliberate TODOs so they are not
mistaken for covered, and are not pinned as passing cases until the gap closes.

Status counts are printed by `cargo test -p zapc conformance` (see
`claims_index_is_consistent`).

## Truthiness (§5, D-0022)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| TRUTH-1 | §5 | zero (`0`, `0.0`) is falsy; every non-zero number is truthy | covered | truthiness_numbers |
| TRUTH-2 | §5/§9 | an empty string is falsy; a non-empty string is truthy (#385) | covered | truthiness_string |
| TRUTH-3 | §5 | an empty list/dict is falsy; a non-empty one is truthy (#382) | covered | truthiness_container |
| TRUTH-4 | §4.3 | `None` is falsy | TODO | — |

## Boolean operators (§5, D-0022)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| BOOL-1 | §5 | `and`/`or` short-circuit left-to-right (#334) | covered | bool_short_circuit |
| BOOL-2 | §5 | `and`/`or` yield a strict `bool`, not the operand (divergence) | covered | bool_strict_value |
| BOOL-3 | §5 | `not` yields a strict `bool` | TODO | — |

## Numeric operators (§5, §4.1, D-0024, D-0019)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| NUM-1 | §5 | `/` is true division and always yields a float (#384) | covered | numeric_true_division |
| NUM-2 | §5 | `//` floors toward negative infinity (D-0024) | covered | numeric_floor_division |
| NUM-3 | §5 | `%` is floored modulo — sign of the divisor (#381) | covered | numeric_floored_modulo |
| NUM-4 | §4.1 | default `int` is int64 and overflow wraps (divergence) | covered | numeric_int_wraps |
| NUM-5 | §5 | `**` is exponentiation | TODO | — |
| NUM-6 | §5 | bitwise and/or/xor/not/shift operators on signed integers | TODO | — |
| NUM-7 | §4.1 | widening implicit, narrowing explicit (D-0020) | TODO | — |
| BIG-1 | §4.1 | bigint arithmetic and comparisons retain arbitrary precision; conversions are explicit and range-checked | covered | bigint_arithmetic |
| CPLX-1 | §4.1 | imaginary literals, complex arithmetic, equality, and rendering use the documented complex value model | covered | complex_arithmetic |

## `str()` / value rendering (§9, D-0026)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| STR-1 | §9 | `str(x)` renders int/float/bool; integral float keeps `.0` (#384) | covered | str_scalar_rendering |
| STR-2 | §9.1 | string index→char, slice, reverse, negative index, substring `in` | covered | str_codepoint_ops |
| STR-3 | §9.1 | `+` concatenates strings | TODO | — |
| STR-4 | §9.2 | string methods: upper/lower/strip/split/join/find/replace | TODO | — |
| STR-5 | §9.3 | f-string interpolation renders fields via `str()`/`format` | TODO | — |
| STR-6 | §9.3 | `format(x, spec)` honors the format mini-language | TODO | — |
| BYTES-1 | §4.2 | `bytes` is byte-based; `b[i]` is an int64 in `0..=255` | TODO | — |
| BYTES-2 | §4.2 | `str(bytes)` decodes UTF-8, raising on invalid | TODO | — |

## Comparison (§5, §9.1)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| CMP-1 | §5 | chained `a < b < c` is Python-chained, not left-assoc (#386) | covered | comparison_chained |
| CMP-2 | §5 | a chained comparison evaluates each operand once (#386) | covered | comparison_eval_once |
| CMP-3 | §9.1 | string ordering is lexicographic by codepoint (D-0025) | covered | comparison_string_lex |
| CMP-4 | §5 | `==`/`!=` are built-in for scalars and strings | TODO | — |
| OP-IN-1 | §5/§9 | `in` tests dict-key / substring membership | TODO | — |
| OP-IN-2 | §5 | `x in <list>` tests list membership | covered | list_membership |
| IDX-1 | §9.1 | negative index counts from the end for lists too | covered | list_negative_index |
| SLICE-1 | §8 | list slice yields a copy; array slice a view (D-0030) | covered | list_slice |

## Dicts & Optional (§18, #281/#299/#299, D-0056)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| DICT-1 | §18 | `d.get(k)` returns `Optional[V]` — `Some`/`None`, non-aborting (#281) | covered | dict_get_optional |
| DICT-2 | §18 | dict iterates keys; `values()`; `k in d`; order unspecified (#299) | covered | dict_iteration |
| DICT-3 | §18 | `d.items()` yields `(k, v)` pairs | TODO | — |
| DICT-4 | §18 | `d[missing]` is a catchable fault (not `get`) | TODO | — |

## Exceptions (§10, D-0007)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| EXC-1 | §10 | a built-in fault (index-out-of-range) is catchable by `except` (#383) | covered | exception_builtin_fault |
| EXC-2 | §10 | `finally` runs during unwinding; execution continues after | covered | exception_builtin_fault |
| EXC-3 | §10 | divide-by-zero is a catchable built-in fault | covered | exception_divide_by_zero |
| EXC-4 | §10 | member access on `None` throws (D-0007) | covered | exception_none_access |
| EXC-5 | §10 | a `raise`d user exception is caught by a matching `except` | TODO | — |
| EXC-6 | §10 | an uncaught exception unwinds and exits non-zero | TODO | — |

## Tuples (§4.8, D-0049)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| TUP-1 | §4.8 | tuples destructure positionally | covered | tuple_unpack |
| TUP-2 | §4.8 | RHS fully evaluated before any target is stored (swap) | covered | tuple_unpack |
| TUP-3 | §4.8 | multiple return values are a tuple return | covered | tuple_unpack |
| TUP-4 | §4.8 | `t[i]` needs a compile-time-constant index | TODO | — |

## Functions & arguments (§7, D-0038)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| FN-1 | §7 | default arguments are evaluated per call (divergence) | covered | function_default_per_call |
| FN-2 | §7 | any parameter may be passed by name | covered | function_keyword_defaults |
| FN-3 | §7 | positional args precede keyword args; defaults may be omitted | covered | function_keyword_defaults |
| FN-4 | §7 | a variadic `TYPE... name` is received as a list | covered | function_variadics |
| FN-5 | §7 | lambda expressions | TODO | — |
| FN-6 | §7 | first-class function values + closures (D-0067) | TODO | — |
| GENR-1 | §7 | generators via `yield` follow the iteration protocol | TODO | — |

## Comprehensions (§8, D-0026)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| COMP-1 | §8 | list comprehension builds a list; `if` clause filters | covered | comprehensions |
| COMP-2 | §8 | dict comprehension `{k: v for ...}` | TODO | — |

## Control flow (§6)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| CTRL-1 | §6 | `if`/`elif`/`else` selects exactly one branch | covered | control_flow |
| CTRL-2 | §6 | `while` loops while its condition is truthy | covered | control_flow |
| CTRL-3 | §6 | `for i in range(a, b)` iterates the half-open `[a, b)` | covered | control_flow |
| CTRL-4 | §6 | `for x in coll` iterates a collection's elements | TODO | — |
| ITER-1 | §6 | iteration protocol: `__iter__` + comma-ok `__next__` (D-0048) | TODO | — |
| DOWHILE-1 | §6 | `do` runs its body once before the `while` test (D-0039) | TODO | — |
| MATCH-1 | §6 | `match`/`case` structural patterns; enum exhaustiveness (D-0040) | TODO | — |
| WHEN-1 | §6 | `when target.*` keeps only the first matching arm (D-0079) | TODO | — |

## Declarations, scalars, lexical (§1–§4.1, D-0003/D-0021/D-0023)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| DECL-1 | §3 | the two decl forms are `TYPE name = v` and `auto name = v` | TODO | — |
| DECL-2 | §3 | reassignment uses no type and no `auto` | TODO | — |
| DECL-3 | §3 | bindings are mutable by default; `const` is immutable (D-0006) | TODO | — |
| SCAL-1 | §4.1 | scalars have value semantics (copied on assign/pass, D-0023) | TODO | — |
| REF-1 | §4.3 | list/dict/struct instances are passed by reference (D-0023) | TODO | — |
| ENTRY-1 | §2 | compiled programs run from `main`; executable module-scope statements are rejected; a module docstring is allowed (D-0021/D-0090) | covered | entry_main |
| LEX-1 | §1 | definitions may follow uses — no forward declarations (D-0021) | TODO | — |

## Structs, interfaces, enums, generics (§4.4–§4.7)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| STRUCT-1 | §4.4 | structs are reference types with `self` methods (D-0015) | TODO | — |
| VSTRUCT-1 | §4.4 | a `value struct` is copied by value (D-0051) | TODO | — |
| IFACE-1 | §4.5 | interfaces are structural, satisfied implicitly, checked at compile time | TODO | — |
| ENUM-1 | §4.6 | tier-1 enum: distinct type, `.name`, iterable (D-0018) | TODO | — |
| ENUM-2 | §4.6 | tier-3 data-carrying variants destructured by `match`, exhaustive (D-0046) | TODO | — |
| OPT-1 | §4.6 | prelude `Optional[T]` is the canonical optional type (D-0065) | TODO | — |
| GEN-1 | §4.7 | built-in containers are generic (`list`/`dict`/`set`/`array`) | TODO | — |

## Operators overloading, modules, concurrency, cleanup (§5, §11–§13)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| OP-3 | §5 | user types overload operators via dunder methods (D-0047) | TODO | — |
| WITH-1 | §11 | `with` closes the resource on block exit, even on exception (D-0027) | TODO | — |
| CHAN-1 | §12 | channels: send/receive, comma-ok receive, `close` (D-0031) | TODO | — |
| SCOPE-1 | §12 | `scope:` joins spawned coroutines; a child failure cancels siblings (D-0044) | TODO | — |
| MOD-1 | §13 | names starting with `_` are private, enforced at compile time (D-0028) | TODO | — |
| MOD-2 | §13 | whole-module import by short name; no `from X import Y` (D-0052) | TODO | — |

## Decorators & arrays (§15–§16)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| ARR-1 | §15 | numeric array: element-wise ops + scalar broadcast; slices are views (D-0011) | TODO | — |
| DEC-1 | §16 | `@derive(...)` generates field-by-field code at compile time (D-0036) | TODO | — |
| DEC-2 | §16 | multiple decorators stack bottom-up | TODO | — |
