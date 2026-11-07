# Test-widening plan (epic #374)

Grounded in [`cpython-coverage-comparison.md`](cpython-coverage-comparison.md)
(merged via PR #726) and the B1–B9 suspected-bug notes on issue #374. Snapshot:
`origin/main` @ `046c3d1` (2026-11-07).

**Status: PROPOSAL — awaiting maintainer sign-off. No test fixtures written yet.**
This document is the deliverable; the test PR waits for approval.

The plan has **two deliberately separated dimensions**:

* **Dimension A — suspected-bug coverage** (§4): one pinning test per B1–B9 and
  the other suspected defects, *as test-writing, never fixes*.
* **Dimension B — walled-garden gaps** (§5): under-/un-tested behaviors from the
  coverage matrix, independent of any known bug, enumerated by area and
  prioritized.

---

## 1. The golden rule, restated for this plan

Per `AGENTS.md` ("Golden rule for core language and stdlib defects") and the
`language-first-no-workarounds` memory:

1. These are **tests**, not fixes. This plan does **not** implement, continue, or
   open a fix for any core language/stdlib defect.
2. A test must **never bake in buggy behavior**. Where a behavior is a confirmed
   core defect, the correct-behavior test is **split out of the green PR** and
   either (a) filed as a separate, separately-assigned core issue whose fix PR
   carries the repro (bug-first rule), and/or (b) recorded in `CLAIMS.md` as an
   `(impl gap #NNN)` TODO row — which the conformance harness already supports and
   which keeps the green PR green while making the gap measurable.
3. Where a suspected bug is **unconfirmed**, the plan says how to confirm it (a
   differential/`zapc run` probe in CI) *before* deciding whether its test is
   green-as-is, a known-failure, or decision-gated.
4. The maintainer decides who fixes a core defect and when. Nothing here assigns
   a fix.

---

## 2. How the existing test machinery works (so the plan fits it)

Four executable surfaces. Counts at the snapshot: **377 golden**, **54
conformance**, **35 differential** `.zap` fixtures, plus Rust unit tests.

| Surface | Add a case by | Oracle | Pins | Gate |
|---|---|---|---|---|
| **Golden** `tests/golden/` | drop `name.zap` + `name.expected` (+ optional `.stderr` substring, `.exit`, `.env`); `golden.rs` auto-discovers | author's `.expected` | exact stdout/stderr/exit; **the home for Zap-only constructs** (`match`, `.contains()`, `raise "str"`, `try…else`, slicing forms CPython lacks) | full CI (both GC collectors) |
| **Differential** `tests/diff/` | drop one `name.zap` with a `# diff: match` or `# diff: diverge <reason>` first-line directive; CPython side is *derived* by `zap_to_python`. **Must be valid in BOTH languages.** `diff_cpython.rs` | **CPython** (`python3`, run both, diff stdout+exit) | Zap↔Python parity over the shared subset; `diverge` asserts they *differ* and cites a `D-00xx` or `BUG #NNN` | rides `cargo test --features zapc/gc-*` in full/arm64 CI (needs LLVM + python3) |
| **Conformance** `tests/conformance/` | `name.zap` + `name.expected` (+ `.exit`/`.stderr`) with `# claim: ID` directive, **and a matching `CLAIMS.md` row** citing the same id; round-trip enforced both ways by `conformance.rs` | SPEC.md + author `.expected` | Zap's OWN normative behavior, **including deliberate divergences**; `(impl gap #NNN)` TODO rows record specified-but-unimplemented behavior without a passing fixture | full CI |
| **Negative / diagnostics** `crates/driver/tests/error_codes.rs` | add `assert_code(src, "ZAPnnnn")` via the `check_source` MCP surface; asserts a code is **present**, not message text | the compiler's diagnostic codes | that bad input is rejected with the right stable code — the "walled garden" for WON'T-DO / DEFERRED syntax | host `cargo test` (no LLVM) |

Also present, **not** a semantic-coverage surface: **doctests** (`zap test --doc`,
`doctest.rs`) — harness + 2 fixtures only; **Rust unit tests** (~1000 `#[test]`
across crates: lexer/parser/sema/lowering shapes/formatter). Lexing, literal, and
precedence *parse-shape* cases belong in the syntax crate unit tests; *runtime
value* cases belong in golden/diff.

**The differential generator** (`diff_cpython.rs::gen`) emits only bounded
int/bool programs today: `int_expr` (`+ - *`, unary minus, leaves 0–9),
`bool_expr` (`and/or/not` over comparisons), chained `comparison`, `str_expr`,
ternary, and a comprehension-count. Widening it (§6) re-runs thousands of
CPython-checked cases for free.

**Two structural facts the plan exploits:**

* **D and C don't compose.** Differential is the only independent oracle but
  covers 35 hand cases + an int/bool fuzzer; conformance checks Zap against
  *itself*, so it can pin a wrong-but-consistent output (B1 is exactly that).
* **Goldens hold most language coverage but aren't indexed.** `CLAIMS.md` only
  sees conformance cases, so ~12 shipped features (§7) are invisible to the
  "measurable coverage" count.

---

## 3. House style for every case added

Exhaustive, not minimal — tests are intent documentation and a walled garden
(`AGENTS.md`). Concretely:

* Each case carries a header comment: what it pins, the `SPEC §`/`D-00xx`/issue,
  and (for `diverge`) *why* it differs.
* Render `float`/`bool`/`char` through `str(...)` in conformance (deterministic);
  reduce observations to scalars/short strings; stay order-independent for
  `dict`/`set` (counts/sums/membership, never a sequence).
* Cover empty / one / many / boundary / error for every method or operator — not
  just the happy path.
* Negative cases assert the **diagnostic code**, never the message text.

---

## 4. Dimension A — suspected-bug coverage (B1–B9)

**Disposition key:**
`GREEN` = pins correct behavior and passes today → goes in the green PR ·
`CONFIRM` = model/source-read only, needs a differential probe before deciding ·
`KNOWN-FAIL` = confirmed core defect; correct-behavior test is split out (file a
core issue + record a CLAIMS `(impl gap)` row; the fix PR carries the repro) ·
`DECISION` = behavior may be an intended divergence; needs a `DECISIONS.md` entry
before any test can be written.

| ID | Behavior | Root layer | Disposition | Action in this plan |
|---|---|---|---|---|
| **B1** | `str(float)`/`print`/f-string/JSON use `%g` (6 sig digits): `str(1/3)`→`0.333333` | runtime (`zaprt.c`), lowering, `json.rs` | **CONFIRM → KNOWN-FAIL** | Probe `print(1/3)` vs `python3`. On confirm: file a core issue (float shortest-round-trip rendering; **note the silent JSON precision loss**), add `CLAIMS` `STR-float (impl gap #NNN)` row, and **hold** the correct-behavior conformance corpus + the float generator's `str(float)` observations until the fix lands. Do **not** pin `0.333333`. |
| **B2** | `int ** negative` → integer floor (`2**-1`→`0`), not float `0.5` | lowering (`zap_ipow`) | **DECISION** | Possibly a deliberate static-typing consequence. Needs a `DECISIONS.md` (NUM-5) ruling. If *divergent*: conformance case pinning Zap's result + a `# diff: diverge D-01xx` case. If *bug*: core issue + KNOWN-FAIL. **No test until the ruling.** |
| **B3** | `x == None`/`x != None` type-checks then dies ZAP6001 in lowering | lowering (`lower.rs::expr`) | **KNOWN-FAIL** (confirmed, prior #611 finding) | SPEC §5 says it works → core defect, not a divergence. File/confirm the core issue (seam is `lower_is_none`), add `CLAIMS (impl gap)` row. Correct-behavior conformance case held until fix. Do **not** add a negative test pinning ZAP6001 (would bake in the defect). |
| **B4** | `<<`/`>>` lower to raw LLVM shift, count unmasked → poison at count ≥64 / negative | lowering (`lower.rs` Shl/Shr) | **CONFIRM** (split) | Probe `1<<64`, `1<<-1`, `-1>>100`. Two separable facts: (a) the **UB/poison** on out-of-range count is a core defect → KNOWN-FAIL + core issue; (b) negative-count-raises-`ValueError` is CPython parity → conformance/diff once (a) is fixed. The `1<<64` *value* differs by D-0019 (int64 wraps) → that part is a `# diff: diverge` pin, not a bug. |
| **B5** | `zap_fmt_flt` 96-byte / `zap_fmt_int` 72-byte buffers silently truncate huge format specs (`{1e100:f}`, `{x:.60f}`) | runtime (`zaprt.c`) | **CONFIRM → KNOWN-FAIL** | Probe the two specs. On confirm: core issue (buffer sizing/dynamic), `CLAIMS (impl gap)` row; correct-behavior format cases held. |
| **B6** | `len(str)` returns **byte** length while index/slice/iter are codepoint-based → `len("héllo")==6`, `s[len(s)-1]` wrong | lowering (`zap_str_len` vs `zap_str_len_cp`) | **KNOWN-FAIL** (documented pending #284) | Core gap already tracked by #284. Add `CLAIMS STR-2b (impl gap #284)` row + a held correct-behavior case (`len("héllo")==5`, non-ASCII `s[-1]`). Do **not** pin `==6`. The **ASCII** codepoint cases (B-dimension) are green now and go in. |
| **B7** | No `AssertionError` anywhere; what `assert` raises / whether `except AssertionError` resolves is unspecified | sema (`BUILTINS`), SPEC §10 | **CONFIRM → DECISION** | Probe: does `assert_fail` produce a catchable, named error? What does `except AssertionError` do? Likely a SPEC §10 omission → a decision (add the type, or document absence in `PYTHON_DIFFERENCES.md`). Test deferred to the ruling; the existing `assert_fail` golden stays. |
| **B8** | SPEC §6 text implies `for/while/try … else` exist; parser has none | SPEC text ↔ parser | **GREEN (doc-fix + negative)** | Two safe actions, both green: (i) **docs** — correct the SPEC §6 sentence to match the parser (rides the PR); (ii) **negative corpus** — `error_codes.rs` cases pinning that `for…else`/`while…else`/`try…else` are rejected with their ZAP code (pins current absence = walled garden, not baking in a bug). The *feature* decision (add `else` or not) is separate (§8). |
| **B9** | `float(s)` on malformed input "faults" (SPEC §9) while `int(s)` raises catchable `ValueError` (#456) | runtime/lowering | **CONFIRM** | Probe `try: float("x")`. If **catchable `ValueError`** → GREEN conformance case `CONV-3` (CPython parity) goes in the PR. If it **faults uncatchably** → core issue + KNOWN-FAIL. Decision hinges entirely on the probe. |

**Other suspected defects named in the doc (§10):** `INT64_MIN // -1` and
`% -1` (x86 `idiv` trap — probe whether `zap_ifloordiv` guards it), deep-recursion
outcome (clean diagnostic vs hang), `d[k]` mutation-during-iteration, `1.0/0.0`
(IEEE inf/nan vs `ZeroDivisionError`). Each: **CONFIRM** first; route to GREEN
(parity/divergence pin) or KNOWN-FAIL per the probe.

**Confirmation probes (§4 + §10) are a prerequisite gate.** They need `zapc run`
+ `python3`, i.e. LLVM — unavailable on this host (Docker down). Options: (a) a
tiny probe-only PR of ~12 `tests/diff/` cases marked `# diff: diverge BUG
<unfiled>` that CI runs and reports, then triaged; or (b) run them inside the test
PR's CI and read the failures. **Recommend (a)** — it turns "source-read" into
"CI-observed" cheaply and feeds every disposition above. This is the one piece
that should land *before* the main green PR.

---

## 5. Dimension B — walled-garden gaps (bug-independent), by area

Ordered within each area by *(bug-hiding odds × program reach ÷ cost)*. Every
item here pins behavior Zap **already implements correctly** (or must-reject
behavior), so it is **green** and belongs in the single PR, except where flagged
`FEATURE-GAP` (needs a keep/defer decision first, §8) or `[B-dep]` (entangled
with a Dimension-A defect, so its *correct-behavior* half waits).

### B-1. Operators & precedence (highest value — pure-expression, trivially diffable)
* **Precedence/associativity table** (diff, ~1 file, ≥40 exprs): `2**3**2`,
  `-2**2`, `not a == b`, `a or b and c`, `a < b | c`, `1 << 2 + 3`,
  `a if b else c if d else e`, `-x ** -y`, `a - b - c`, `a / b / c`, `a // b * c`.
* **All 13 augmented-assignment ops** on int/float/str (conformance + golden):
  `+= -= *= /= //= %= **= <<= >>= &= |= ^=`; **evaluate-once** of
  `xs[f()] += 1` and `obj.a += 1` (counter); `list += list` aliasing (pin D-0047
  in-place vs Python); `const` target → ZAP3017; undeclared target → error.
  `**=`/`<<=`/`>>=`/`/=` are `[B-dep]` on B2/B4/B1.
* **Bitwise & shift** (`&|^~`, `<<`,`>>`): negatives (`-5 & 3`, `~x == -x-1`),
  `bool & bool`, precedence vs comparison, bigint. Shift-count edges `[B-dep B4]`.
* **Unary** `+ - ~ not`: `-INT64_MIN`, `- -x`, `+True`, `~` on bool/float → error.
* **`**`** matrix `[B-dep B2]`: `0**0`, `(-2)**3`, `2**63` wrap-vs-bigint, right-assoc.

### B-2. Numerics breadth
* **Integer literals** runtime value: `1_000`, `0o17`, `0b101`, `0xFF`, int64
  boundary (`…807`, `…808`→bigint, `-…808`). (syntax unit + golden)
* **`//`/`%` on floats/mixed**: `-7.5 // 2`, `5.5 % -2`, `inf // 1`, `0.0 // 0.0`
  → `ZeroDivisionError` probe.
* **`INT64_MIN` edges**: `-INT64_MIN`, `INT64_MIN // -1`, `% -1` `[B-dep, probe]`.
* **bigint**: `str`/parse of >int64 text, `//`/`%` sign matrix, `**` growth,
  bitwise/shift, exactness vs float (`2**53+1 == 9007...993.0` is `False`), mixed
  `int64+bigint` (promotion or compile error — pin), `int64(big)` overflow type.
* **float inf/nan/-0.0**: `nan != nan`, `nan == nan`, `inf - inf`,
  `-0.0 == 0.0`, `float("inf")`, `float("nan")`, nan in `<`.
* **bool-as-distinct-type** (D-0022): `True + 1`, `True == 1`, `sum([True,True])`,
  `~True`, `bool & int` — pin Zap's chosen result (value vs compile error).
* **Mixed-width/widening ladder** (NUM-7): `int32 + int64`, `int32` overflow
  wrap, `int + float`, narrowing without cast → compile error.
* **float↔int conversion**: `int(3.9)`, `int(-3.9)`, `int(1e30)`, `int(nan)/int(inf)`
  → error, `int("  12  ")`, `int("+5")`, `int("1_000")`, `int("1e3")`→error,
  `int("")`. `str(float)` forms `[B-dep B1]`.
* **complex**: `repr` forms (`1j`, `(1-0j)`, nan/inf parts), `/`-by-zero, `**`,
  mixed, `==` vs real — pin the `%g` divergent rendering.

### B-3. Strings / bytes / format
* **Non-ASCII** index/slice/iter (ASCII portion green now): `"日本語"[1:]`, emoji,
  step≠±1 on multibyte, step 0 → `ValueError`, out-of-range clamping. `len`
  non-ASCII `[B-dep B6]`.
* **`+`/`*`/`in`/compare**: `"ab"*3`, `*0`, `*-1`, `"" in s`, `"a" < "B"` —
  `str * int` is `FEATURE-GAP` (not in SPEC — needs §8 ruling).
* **present methods** full matrix: `strip(chars)`, `split()` no-arg (whitespace),
  `split(sep, maxsplit)`, `replace(old,new,count)`, empty `sep` → `ValueError`,
  `"é".upper()` (pin ASCII-only divergence). (golden + diff for parity ones)
* **absent methods** (`count rfind rsplit splitlines partition title center
  ljust rjust zfill isdigit…`) → `FEATURE-GAP` (§8); until decided, no test.
* **f-strings**: `{{`/`}}`, multi-line/triple-quoted, lambda/dict/ternary/walrus
  in field, other-quote inside field, `{x!s}`; negative tests for each documented
  limit (`!r`, nested spec, non-ASCII fill → ZAP2017/ZAP4026); probe `{x=}`.
* **format-spec**: sign (`+`/` `), `#`, `0`-pad with sign, `=` align, `.precision`
  truncate on str, types `b o x X d e E f F g G s`, rounding (`format(2.675,'.2f')`),
  `inf`/`nan`. Huge-magnitude specs `[B-dep B5]`.
* **bytes**: index→int, slice→bytes, concat/repeat, `in`, `decode` errors,
  `bytes([256])`→`ValueError`, `bytes(-1)`. `bytearray`/`bytes %` → FEATURE-GAP.
* **print kwargs**: `sep=`, `end=`, empty `print()`, nested container/None/Optional.
* **repr/str consistency** (D-0124): `print("a")` vs `print(["a"])` vs `f"{['a']}"`.

### B-4. Containers
* **list methods** matrix on empty/one/many; `pop()` empty→`IndexError`,
  `remove`/`index` absent→`ValueError`, `sort` stability (D-0083). `insert extend
  count reverse` → FEATURE-GAP (§8).
* **list operators**: `+`, `*`, `[0]*3`, `[[0]*2]*2` aliasing trap, slice
  assign/delete (likely unsupported — pin rejection), lexicographic `<`, `+=`
  aliasing (pin D-0047).
* **structural equality** `[1,2]==[1,2]`, nested, differing length, order
  (set/dict), `list==tuple` → pin (CMP-4) — **probe first** whether it works.
* **dict**: `.items()` (used in goldens, not indexed), `pop`/`setdefault`/
  `popitem`/`update`/`clear`/`copy`, `d[missing]` catchable fault (DICT-4),
  equality order-independent, delete-during-iteration (Python `RuntimeError` —
  pin Zap's). Order stays `# diff: diverge` (D-0056).
* **set**: `| & - ^ <= <`, `union/intersection/difference/issubset`, `discard`,
  `remove` absent→`KeyError`, `pop`, equality, iteration-while-mutating.
  `frozenset`/operators possibly FEATURE-GAP.
* **tuple**: `(1,2)<(1,3)`, `==` nested, concat/repeat, `len`, `in`, `t[-1]`
  const, `t[i]` non-const → compile error (pin TUP-4), `tuple(list)`, `(1,)`, `()`.
* **comprehensions**: dict-comp duplicate key (last wins), `for`/`if` eval order,
  comp over dict/str/set/range-step, nested, comp referencing `self`, empty,
  element expr evaluated once; explicit "loop var absent after comp" negative.
* **aliasing/value-copy** (D-0023/0051): `# diff: diverge` where Python aliases
  and Zap copies (tuple-of-ints, value struct); list-of-list aliasing; pass list
  to fn and mutate; struct assign shares.

### B-5. Control flow, iteration, truthiness
* **`for` over** list/str/dict/set/tuple/range/generator/channel; target unpack
  (`for k,v in d.items()`, nested); mutate-list-during-iteration (pin Zap's);
  loop var after loop; empty loop. (`enumerate`/`zip` absent → FEATURE-GAP.)
* **`range`**: 1/2/3-arg, negative step, empty, step 0 → `ValueError`,
  `in`/`len`/index/slice on range (probe).
* **break/continue**: out of nested loops, `continue` in `finally`.
* **`do…while`** (Zap-only, golden): body-once, `continue`→test, `break`.
* **truthiness breadth** (TRUTH-4 `None`): `Optional` Some/None, `()` vs `(0,)`,
  empty set, `b""`, `'\0'` char, `nan` truthy, `-0.0`, bigint `0`, user struct in
  `if` (compile error?), truthiness in comp-`if`/`while`/ternary/`not`.
* **`not`** on each type → strict bool (BOOL-3); side-effect counters on operands.
* **ternary**: right-assoc nesting, lambda, evaluated-once branches, precedence
  vs `or`, branch type unification (`1 if c else 2.0`).
* **chained comparison**: 4+ links, `a < b == c`, mix with `in`/`is`/`not in`
  (latter two chain / `not in` does not), short-circuit stops later operands.

### B-6. Functions & arguments
* **argument error matrix** (compile-time): too many/few, unexpected/duplicate
  keyword, positional-after-keyword, pos-only by name, kw-only positionally,
  missing required kw-only — one `error_codes.rs` case each, assert ZAP code.
* **argument evaluation order**: `f(b=g(), a=h())` left-to-right; args before an
  expression-callee.
* **defaults**: side-effecting default runs once *per call* (counter, pins
  D-0038); default referencing a module const / earlier param.
* **`*args`/`**kwargs`**: spread generator/dict/empty, duplicate key via `**`+kw
  → error, `**` merge order, `*args` after keyword.
* **lambda**: in default arg, returning lambda, IIFE, closing over loop var,
  as `sorted` key (`sorted(key=)` — probe present/absent).

### B-7. Exceptions
* **`try/finally` interactions** (where compilers diverge most): `finally` on
  return/break/continue in `try`/`except`; **return/break/continue inside
  `finally`** (overrides); exception in `finally` replaces original; exception in
  `except`; nested try in `finally`; `finally` with generator.
* **multiple `except`** ordering, catch-all not last → error, duplicate clause.
* **`raise` forms**: `raise E("m")`, `raise "str"` (Zap shorthand, golden),
  `raise ValueError` no-parens → ?, raise non-exception → error, re-raise identity.
* **chaining**: uncaught chained traceback stderr golden; `from None`; context vs
  cause; `e.cause`.
* **hierarchy**: pin existing `issubclass` relations (`KeyError⊂LookupError⊂
  Exception`), `Cancelled⊂BaseException` golden. `AssertionError` `[B-dep B7]`.
* **uncaught → exit 1 + traceback**: pin exit code for uncaught user exc, in
  spawned coroutine, in `finally`, `sys.exit(n)`, in generator (EXC-6).
* **`assert`**: `assert cond, msg` (msg evaluated lazily only on failure), assert
  of tuple, side-effect-once in condition. `except AssertionError` `[B-dep B7]`.

### B-8. `with`, `match`, decorators, scope
* **`with`** (index WITH-1): reverse exit order for multiple items; exception in
  `__enter__` (no `__exit__`); exception in `__exit__` replaces body's; `as`
  unpack; nested with + generator; with in loop with `continue`; pin "cannot
  suppress" (D-0069).
* **`match`**: subject evaluated once; guard side effects & fall-through; literal
  patterns for `None`/`True`/`False`/negatives/floats/strings/`char`; `case 1` vs
  `True` (strict-bool); or-pattern must bind same names → error; irrefutable not
  last → error; duplicate capture → error; `case [x, *_]`; string/bytes subjects;
  no-match non-enum. Pin struct positional-by-decl-order ≡ by-name.
* **decorators** (flip DEC-1/2 index): on method, bottom-up application order,
  returning non-callable (golden exists), `@undeclared` → compile error.
* **scope/closures** (unpinned divergences, D-0067/0112): mutate captured var
  after lambda (Zap snapshot — pin); `[lambda: i for i in range(3)]` late-binding
  classic as `# diff: diverge`; rebind captured local → error/local; captured
  list mutation visible; capture-of-capture; module-binding rebind vs Python
  local (`# diff: diverge` + ZAP5125 sharing-lint golden).
* **recursion**: mutual; deep (~100k) non-tail; infinite → pin *documented*
  outcome (clean diagnostic + non-zero exit, not a hang).

### B-9. Lexing / literals / line structure (syntax unit tests + a few goldens)
* `_` grouping, `0x/0o/0b`, leading-zero rule; float literal forms (`.5`, `1.`,
  `1e-3`, `1_0.0_1`); imaginary (`0j`, `1e3j`).
* string escapes (`\n\t\x\u\U\N{}`), raw, triple-quoted, **adjacent-literal
  concatenation** `"a" "b"` (probe: does it parse?), unknown escape `"\q"`,
  bytes escape ranges, surrogate `"\ud800"` → error.
* line structure: `;`-separated, `\` continuation (in expr and in string),
  implicit continuation.
* indentation: inconsistent-dedent error, first-line-indented, form-feed, 100-deep.
* **soft keywords as identifiers** (one golden each: `pass del from fn type m s`
  as a variable/function/field name) — a Zap-specific regression magnet.
* unicode identifiers accepted or cleanly rejected (probe).

---

## 6. Differential-generator widening (§1 item 2 of the doc)

Extend `diff_cpython.rs::gen` — each generator re-runs thousands of
CPython-checked programs with zero new oracle cost. Add, roughly in value order:

1. **Precedence/augmented-assignment statements** — emit the §B-1 operator
   grammar (mixed `+ - * // %` with `**` right-assoc, comparison vs bitwise, the
   ternary ladder). Pure-int, no `[B-dep]`.
2. **List/dict order-independent observations** — build a list/dict, observe
   `len`, `sum`, membership, `sorted(...)` (never raw iteration order).
3. **String generator** widening — methods that match CPython (`strip`, `split`,
   `replace`, `find`, `startswith`), `+`/`in`/compare.
4. **Float generator** — **gated on B1.** Until B1 is fixed, observe floats only
   through B1-independent channels (comparisons, `int(x)`, booleans), **not**
   `str(float)` / bare `print(float)`. After B1 lands, add the shortest-round-trip
   `str(x)` equality property over random doubles.

The generator's unit tests (`generator_is_deterministic_and_bounded`) and the
`zap_to_python` transform tests stay the guardrail; new grammar must keep every
emitted program valid in **both** languages.

---

## 7. CLAIMS.md hygiene (cheap, high-signal — in the green PR)

Make the "61 covered / 43 TODO" number honest:

* **Add rows** for the ~12 shipped-without-a-row features (§3.2 of the doc):
  `pass`, `not in`, chained assignment, `del`, positional-only `/`, `as`
  patterns, `raise … from`, bare `raise`, `type` alias, `is`/`is not`, `int`
  synonym, `@cache`/behavioral decorators — each already has a golden; point a
  (new or existing) conformance case at it.
* **Flip** TODO rows already exercised by goldens (OP-IN-1, DICT-3, COMP-2,
  TRUTH-4 once a case lands, …).
* **Add `(impl gap #NNN)` rows** for every KNOWN-FAIL from §4 (B1, B3, B6, and
  B4/B5 after confirm) — this is how a core defect is recorded without a passing
  fixture.

---

## 8. Decisions the maintainer must make (block some tests; the fix/feature is yours to assign)

These are **not** test questions — they gate whole clusters. Recommend a single
`PYTHON_DIFFERENCES.md` / `DECISIONS.md` batch (record the answer even when it is
"no"):

* **B2** `int ** negative` — divergence or bug? (NUM-5)
* **B7** `AssertionError` — add the type, or document its absence?
* **B8-feature** `try/for/while … else` — add, or stay absent? (the SPEC §6
  *text correction* is green regardless and rides the PR)
* **`except (A, B)`** tuple — add or document.
* **Missing builtins** `abs min max any all enumerate zip map filter reversed
  isinstance type iter next id hash repr divmod pow round` — minimalism or
  oversight? (§9 of the doc). Each decided-present builtin then gets a diff case.
* **Container/str methods** absent (list `insert/extend/count/reverse`, set
  algebra, most `str` methods), **`str * int`**, **`%`/`str.format`**,
  **`frozenset`**, **`bytearray`** — keep/defer each.

Per `AGENTS.md`, these go in the repo's own `DECISIONS.md` once ruled; any
resulting *fix* is a separate, separately-assigned issue.

---

## 9. Proposed PR structure

The user wants "mostly a single PR." Structure:

### PR 0 (prerequisite, tiny): confirmation probes
~12 `tests/diff/` cases marked `# diff: diverge BUG <unfiled>` exercising B1, B4,
B5, B7, B9 and the §4 "other" probes, run in CI to convert source-read suspicions
into observed results. Output triages every CONFIRM in §4. ~1 small PR, docs-light.
*(Alternative: fold the probes into the main PR's CI and read failures — but a
separate probe PR keeps the main PR unambiguously green.)*

### PR 1 (the bulk — ONE green PR): walled-garden widening
Everything in Dimension B that is green now, plus the green slices of Dimension A.
All fixtures pass on `main`. Internal clusters (each a tidy commit):

| Cluster | Surfaces | Rough size |
|---|---|---|
| Operators & precedence (§B-1, minus `[B-dep]`) | diff + conformance + golden | ~15–20 fixtures + 1 generator extension |
| Numerics breadth (§B-2, minus `[B-dep]`) | conformance + golden + syntax unit | ~25–30 fixtures |
| Strings/bytes/format (§B-3, present methods, f-string edges, negatives) | golden + diff + error_codes | ~25–30 fixtures |
| Containers (§B-4, methods/equality/aliasing/comprehensions) | golden + conformance + diff | ~30–35 fixtures |
| Control flow / iteration / truthiness (§B-5) | conformance + golden + diff | ~20–25 fixtures |
| Functions & arguments (§B-6, incl. compile-fail matrix) | conformance + error_codes | ~15–20 fixtures |
| Exceptions (§B-7, finally/raise/hierarchy/uncaught) | golden + conformance | ~20–25 fixtures |
| with / match / decorators / scope (§B-8) | golden + conformance + diff | ~25–30 fixtures |
| Lexing / literals / line structure (§B-9) | syntax unit + golden | ~15–20 cases |
| Negative corpus (§3.3 of the doc + B8 `else` rejection) | error_codes | ~25 `assert_code` cases |
| Generator widening (§6, precedence + list/dict + string) | diff_cpython.rs | code, not fixtures |
| CLAIMS hygiene (§7: add rows, flip, impl-gap rows) | CLAIMS.md + a few conformance cases | ~12 rows + ~6 cases |
| Docs | SPEC §6 `else` correction; `PYTHON_DIFFERENCES.md` gap notes; CHANGELOG | per AGENTS.md |

**Total ≈ 240–290 fixtures + generator code + doc/CLAIMS updates** — large but
cohesive; it is all "pin behavior that already works + reject what should be
rejected." Review-friendly because every file is self-describing and the suite is
green. If the maintainer prefers, split along the cluster lines into 2–3 PRs by
area; the clusters are independent.

### Must split OUT of PR 1 (not green — never in the single PR)
* **Correct-behavior fixtures for confirmed core defects** — B1 (float
  rendering + JSON), B3 (`== None`), B6 (non-ASCII `len`), and B4/B5 after
  confirm. Each lands **with its separately-assigned core-fix PR** (bug-first
  rule), not here. In PR 1 they appear only as `CLAIMS (impl gap #NNN)` rows +
  filed issues.
* **Decision-gated tests** — B2, B7, and every §8 FEATURE-GAP cluster wait for
  the ruling; then they land as conformance/divergence pins (if decided
  divergent/absent) or with the feature PR (if decided to implement).
* **Float generator `str(float)` observations** — gated on B1.

### Reporting to issue #374
If findings are posted to #374, attribution is **required** on the comment
("Claude input on …"). Any commit or the eventual PR description carries **only
the maintainer's name** — no model mention (memory `no-claude-attribution-zaplang`).

---

## 10. Immediate next actions (after sign-off)

1. Land **PR 0** (probes); triage B1/B4/B5/B7/B9 and the §4 "other" list.
2. File core issues for each confirmed defect (B1/B3/B6 already strong; add
   B4/B5 per probe) — **report only; do not fix** (golden rule).
3. Maintainer rules the §8 decision batch.
4. Build **PR 1** cluster by cluster, green throughout; add CLAIMS impl-gap rows
   + the SPEC §6 doc fix.
5. Correct-behavior defect fixtures ride their assigned fix PRs later.
