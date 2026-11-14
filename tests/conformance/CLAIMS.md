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
| NUM-8 | §5 | float `%` takes the sign of the divisor (floored), consistent with int `%` (#751) | covered | numeric_float_modulo |
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
| STRLIT-1 | §9 | string escapes `\x`/`\u`/`\U` decode to the codepoint (`\xHH` is a raw byte in `bytes`); `\N{}` is a separate unimplemented form (#781) | covered | string_hex_escapes |
| BYTES-1 | §4.2 | `bytes` is byte-based; `b[i]` is an int64 in `0..=255` | TODO | — |
| BYTES-2 | §4.2 | `str(bytes)` decodes UTF-8, raising on invalid | TODO | — |

## string → value conversions (§9, #456)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| CONV-1 | §9 | `int(s)` on a malformed string raises a *catchable* `ValueError` (#456) | covered | conversion_int_valueerror |
| CONV-2 | §9 | an uncaught `int(s)` `ValueError` prints a clean message and exits 1, not an abort (#456) | covered | conversion_int_valueerror_uncaught |

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
| DICT-5 | §18 | inline `d.get(k).unwrap_or(default)` chains without an annotated local (#497) | covered | dict_get_inline_optional |

## Exceptions (§10, D-0007)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| EXC-1 | §10 | a built-in fault (index-out-of-range) is catchable by `except` (#383) | covered | exception_builtin_fault |
| EXC-2 | §10 | `finally` runs during unwinding; execution continues after | covered | exception_builtin_fault |
| EXC-3 | §10 | divide-by-zero is a catchable built-in fault | covered | exception_divide_by_zero |
| EXC-4 | §10 | member access on `None` throws (D-0007) | covered | exception_none_access |
| EXC-5 | §10 | a typed `raise` exposes its message through `as e` | covered | exception_typed_dispatch |
| EXC-6 | §10 | an uncaught exception unwinds and exits non-zero | TODO | — |
| EXC-7 | §10 | typed clauses dispatch in order, match subclasses, and preserve cleanup/propagation | covered | exception_typed_dispatch |

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
| FN-7 | §7 | a bare `*` makes following params keyword-only (D-0095, I#496) | covered | function_keyword_only |
| FN-8 | §7 | a `V **name` keyword-collector gathers extra keywords into `dict[str, V]` (#601) | covered | function_kwargs |
| FN-9 | §7 | `*expr`/`**expr` spread call arguments into positional / keyword params (#597) | covered | function_starred_args |
| FN-5 | §7 | lambda expressions | TODO | — |
| FN-6 | §7 | first-class function values + closures (D-0067) | TODO | — |
| GENR-1 | §7 | generators via `yield` follow the iteration protocol | TODO | — |

## Comprehensions (§8, D-0026)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| COMP-1 | §8 | list comprehension builds a list; `if` clause filters | covered | comprehensions |
| COMP-2 | §8 | dict comprehension `{k: v for ...}` | TODO | — |
| COMP-3 | §8 | comprehensions take multiple `for` clauses and interleaved `if` filters (#599) | covered | comprehension_multi_clause |
| COMP-4 | §8 | a generator expression `(x for x in xs)` is a lazy iterator, not a list (#596) | covered | genexpr |
| DSP-1 | §4 | `*expr` splices into a list/set/tuple display; `**expr` merges a dict, last-wins (#598) | covered | display_unpacking |

## Control flow (§6)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| CTRL-1 | §6 | `if`/`elif`/`else` selects exactly one branch | covered | control_flow |
| CTRL-2 | §6 | `while` loops while its condition is truthy | covered | control_flow |
| CTRL-3 | §6 | `for i in range(a, b)` iterates the half-open `[a, b)` | covered | control_flow |
| CTRL-4 | §6 | `for x in coll` iterates a collection's elements | TODO | — |
| CTRL-5 | §6 | `range(a, b, step)` honors a negative step (counts down); step 0 raises `ValueError` (#759) | covered | numeric_range_step |
| CTRL-6 | §6 | the conditional expression `a if c else b` evaluates only the chosen branch (#776) | covered | control_flow_ternary |
| ITER-1 | §6 | iteration protocol: `__iter__` + comma-ok `__next__` (D-0048) | TODO | — |
| DOWHILE-1 | §6 | `do` runs its body once before the `while` test (D-0039) | TODO | — |
| MATCH-1 | §6 | `match`/`case` structural patterns; enum exhaustiveness (D-0040) | covered | match_structural |
| WHEN-1 | §6 | `when target.*` keeps only the first matching arm (D-0079) | TODO | — |

## Declarations, scalars, lexical (§1–§4.1, D-0003/D-0021/D-0023)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| DECL-1 | §3 | the two decl forms are `TYPE name = v` and `auto name = v` | TODO | — |
| DECL-2 | §3 | reassignment uses no type and no `auto` | TODO | — |
| AUGSUB-1 | §3 | augmented assignment to a subscript or field target (`xs[i] += v`, `o.f += v`), evaluated once (#750) | covered | aug_assign_target |
| DECL-3 | §3 | bindings are mutable by default; `const` is immutable (D-0006) | TODO | — |
| SCAL-1 | §4.1 | scalars have value semantics (copied on assign/pass, D-0023) | TODO | — |
| REF-1 | §4.3 | list/dict/struct instances are passed by reference (D-0023) | TODO | — |
| ENTRY-1 | §2 | compiled programs run from `main`; executable module-scope statements are rejected; a module docstring is allowed (D-0021/D-0090) | covered | entry_main |
| LEX-1 | §1 | definitions may follow uses — no forward declarations (D-0021) | TODO | — |

## Structs, interfaces, enums, generics (§4.4–§4.7)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| STRUCT-1 | §4.4 | structs are reference types with `self` methods (D-0015) | TODO | — |
| STRUCT-2 | §4.4 | struct methods honor default + keyword arguments like free functions (D-0038, I#481) | covered | method_default_args |
| VSTRUCT-1 | §4.4 | a `value struct` is copied by value (D-0051) | TODO | — |
| IFACE-1 | §4.5 | interfaces are structural, satisfied implicitly, checked at compile time | TODO | — |
| ENUM-1 | §4.6 | tier-1 enum: distinct type, `.name`, iterable (D-0018) | covered | enum_tier1_iteration |
| ENUM-2 | §4.6 | tier-3 data-carrying variants destructured by `match`, exhaustive (D-0046) | TODO | — |
| OPT-1 | §4.6 | prelude `Optional[T]` is the canonical optional type (D-0065) | TODO | — |
| GEN-1 | §4.7 | built-in containers are generic (`list`/`dict`/`set`/`array`) | covered | collection_elements |
| SET-1 | §4.6 | `set` is Stable; `set[T]()` builds an empty set and `set(iterable)`/`set[T](iterable)` build a deduplicated set from any iterable (#608/#609) | covered | set_constructors |

## Operators overloading, modules, concurrency, cleanup (§5, §11–§13)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| OP-3 | §5 | user types overload operators via dunder methods (D-0047) | TODO | — |
| WAL-1 | §5 | the walrus `(name := expr)` evaluates `expr` once, binds `name` in the enclosing scope, and yields the value, in `if`/`while` conditions and comprehension filters (#600, D8) | covered | walrus |
| WITH-1 | §11 | `with` closes the resource on block exit, even on exception (D-0027) | TODO | — |
| CHAN-1 | §12 | channels: send/receive, comma-ok receive, `close` (D-0031) | TODO | — |
| SCOPE-1 | §12 | `scope:` joins spawned coroutines; a child failure cancels siblings (D-0044) | covered | scope_cleanup |
| SCOPE-2 | §12 | `scope(timeout=<ms>)` cancels and drains children, preserves cleanup, raises `ScopeTimeout` to the caller, and composes orthogonally with the failure policy (a child failure outranks the timeout) | covered | scope_timeout |
| SCOPE-3 | §12 | scope grace bounds an uncooperative drain after the deadline, then severs the non-draining child (detach + leak) and raises a catchable `ScopeTimeout` — the process is never force-exited (#825) | covered | scope_timeout_grace |
| SCOPE-4 | §12 | `ScopeTimeout` reports every coroutine still outstanding at the deadline, not a sample | covered | scope_timeout_pending |
| SCOPE-5 | §12 | `spawn` supports named functions, user-struct method calls, and first-class function values, capturing receiver/arguments/closure eagerly in the parent; a `scope:` joins them all (D-0118) | covered | spawn_forms |
| SCOPE-6 | §12 | the method and function-value `spawn` forms compose with `scope(timeout=…)`: on the deadline the children are cancelled and `ScopeTimeout` reports each by its identity — a method as `Struct.method`, a function value by its callee name (D-0118) | covered | spawn_forms_timeout |
| SCOPE-7 | §12 | a `scope(timeout=T, grace=G)` whose children never reach a cancellation point SEVERS them at the grace deadline (detach + leak, like `spawn detached`; never force-unwound/freed) and raises a catchable `ScopeTimeout` — the process stays alive and never `_exit`s (#825, D-0142) | covered | scope_timeout_sever |
| SCOPE-8 | §12 | a bare `scope(timeout=T)` with no `grace=` is always enforceable: at T it severs a non-draining child immediately (implicit grace=0) and raises `ScopeTimeout`; it never hangs the join and never `_exit`s (#825, D-0142) | covered | scope_timeout_nograce |
| SCOPE-9 | §12 | one endpoint whose child ignores cancellation times out (severed orphan, returns an error) while a concurrent healthy endpoint runs to completion — one faulty request never takes the process down (#825, D-0142) | covered | scope_timeout_server |
| MOD-1 | §13 | names starting with `_` are private, enforced at compile time (D-0028) | TODO | — |
| MOD-2 | §13 | whole-module import by short name; no `from X import Y` (D-0052) | TODO | — |

## Decorators & arrays (§15–§16)

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| ARR-1 | §15 | numeric array: element-wise ops + scalar broadcast; slices are views (D-0011) | covered | array_contract |
| ARR-2 | §15 | numeric array: N-D shape + multi-axis indexing, row views, shape attrs, full reductions (#500, D-0164) | covered | array_nd_contract |
| ARR-3 | §15 | numeric array: element-wise broadcasting — trailing-dim alignment, size-1 stretch, rank extension; incompatible shapes raise (#501, D-0165) | covered | array_broadcast_contract |
| ARR-4 | §15 | numeric array: full dtype matrix (f32/f64, i8/i16/i32/i64, u8/u16/u32/u64, bool) + NEP-50 promotion; per-width wrap (#503/#890, D-0166) | covered | array_dtype_matrix_contract |
| ARR-5 | §15 | numeric array: constructors zeros/ones/full (int or tuple shape), arange (half-open, zero step raises), linspace (num samples, ends inclusive) (#504, D-0167) | covered | array_constructors_contract |
| ARR-6 | §15 | numeric array: element-wise math `math.sqrt`/`exp`/`log`/`sin`/`cos`/`tan`/`fabs` map over an array -> new float array (int promotes to f64) (#505, D-0168) | covered | array_math_contract |
| ARR-7 | §15 | numeric array: true division `/` (int operands promote to f64, float keep width; zero divisor -> IEEE inf/nan, no raise) + non-commutative scalar-on-left `s - a` / `s / a` (#506, D-0169) | covered | array_division_contract |
| ARR-8 | §15 | numeric array: reductions sum/min/max/mean/prod as methods, optional axis= -> rank-(n-1) array (dtype preserved, mean float; empty sum/prod/mean = 0/1/nan, empty min/max + bad axis raise ValueError) (#507, D-0170) | covered | array_reductions_contract |
| ARR-9 | §15 | numeric array: strided slice views — a[::2]/a[::-1] (1-D stepped/reversed), m[lo:hi] (N-D axis-0, rank-preserving), m[:, j] (sliced axis in a multi-axis index); all alias the parent, arithmetic over a strided operand compacts first (#502, D-0171) | covered | array_strided_contract |
| ARR-10 | §15 | numeric array: matrix multiply `@` / `dot` on 1-D/2-D (1-D·1-D scalar dot, 2-D·2-D matrix, mixed 1-D/2-D vector; `@=` in-place; NEP-50 dtype; shape mismatch -> ValueError) (#509, D-0172, PEP 465) | covered | array_matmul_contract |
| ARR-11 | §15 | numeric array: shape ops — reshape/transpose/`.T` (views), flatten (copy), concatenate/stack (copies, optional axis=); size/shape/axis fault -> catchable ValueError (#511, D-0174) | covered | array_shape_ops_contract |
| ARR-12 | §15 | numeric array: boolean-mask & fancy (integer-array) indexing + where — comparisons and bitwise combiners build masks, `a[mask]`/`a[idx]` gather a copy, `a[mask]=`/`a[idx]=` scatter, where(cond,x,y) selects and where(cond) is nonzero, multi-advanced plus mixed basic/advanced follow NumPy's transpose rule, array truthiness and out-of-bounds raise catchable ValueError/IndexError (#510, D-0175) | covered | array_fancy_mask_contract |
| ARR-13 | §15 | numeric array: the reduction kernels (sum/min/max/mean/prod, whole-array and axis=) are vectorized with multiple independent accumulators plus a scalar tail — a performance change only; integer reductions stay bit-identical at any size and axis reductions preserve per-element accumulation order for every dtype (#911, D-0177) | covered | array_reduce_vec_contract |
| ARR-14 | §15 | numeric array: the broadcast element-wise kernel vectorizes its innermost axis (contiguous operand vector-loads, stretched operand splats) instead of a per-element scalar loop — a performance change only; a broadcast of any shape (row/column vector, outer product, N-D, mixed dtype) yields the same results as the scalar reference (#896, D-0178) | covered | array_broadcast_vec_contract |
| ARR-15 | §15 | numeric array: the matrix-multiply floor kernel (`@`/`dot`, #509/D-0172) is vectorized via a contiguous `ikj` order over materialized buffers instead of a scalar column-walking triple loop — a performance change only; products are bit-identical to the scalar reference for every dtype and shape (own-kernel FLOOR, #913, D-0180) | covered | array_matmul_floor_contract |
| DEC-1 | §16 | `@derive(...)` generates field-by-field code at compile time (D-0036) | TODO | — |
| DEC-2 | §16 | multiple decorators stack bottom-up | TODO | — |
