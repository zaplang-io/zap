# CPython vs. Zap: language-level test-coverage comparison

Epic #374 (language correctness hardening). **Analysis only** — no tests were
added, no issues filed, no production code changed. Snapshot: `origin/main` @
`1ac5ad6` (2026-11-06).

The goal is *not* to adopt CPython's suite. CPython's `Lib/test` is used as a
**checklist of language behaviors** to confirm Zap's own tests are exhaustive and
to surface gaps. Deliberate divergences (D-00xx) are called out as divergent, not
as gaps.

## How to read this, and its limits

* **Method.** Everything below was derived by *reading* — fixtures, harnesses,
  `SPEC.md`, `DECISIONS.md`, `docs/GRAMMAR.ebnf`, the parser/sema/lowering/runtime
  sources, and the grammar-parity ledger (#614). No Zap program was *executed* for
  this document (no host LLVM, Docker is down). Where a claim rests on source
  reading rather than a run, it is labelled **(source-read)**; anything that needs a
  real run is labelled **probe** and belongs in CI.
* **CPython side.** The categories follow CPython 3.13's pure-language `Lib/test`
  modules (`test_grammar`, `test_syntax`, `test_scope`, `test_int`/`test_long`,
  `test_float`, `test_complex`, `test_bool`, `test_str`/`test_unicode`,
  `test_bytes`, `test_fstring`, `test_format`, `test_list`/`test_dict`/`test_set`/
  `test_tuple`, `test_listcomps`/`test_genexps`, `test_generators`, `test_iter`,
  `test_richcmp`, `test_augassign`, `test_binop`, `test_unary`, `test_slice`,
  `test_unpack`/`test_unpack_ex`, `test_extcall`/`test_keywordonlyarg`/
  `test_positional_only_arg`, `test_exceptions`/`test_raise`/`test_try`/
  `test_exception_group`, `test_with`, `test_patma`, `test_decorators`,
  `test_named_expressions`, `test_builtin`, …). Module names are from knowledge of
  the CPython tree; the `Lib/test` listing was not re-fetched, and individual
  CPython tests were not read.
* **Zap surfaces** are tagged: **D** = CPython differential (`tests/diff/`,
  `crates/driver/tests/diff_cpython.rs`, #375), **C** = spec-conformance
  (`tests/conformance/`, #376), **G** = golden fixtures (`tests/golden/`,
  361 programs), **U** = Rust unit tests in the crates (~1000 `#[test]`: syntax
  169, sema 253, ir 173, driver 282, …), **Doc** = doctests. grep-based "usage"
  counts are approximate (comments and unrelated tokens inflate them); a "no hit"
  is more trustworthy than a "hit".
* **Verdicts.** `COVERED` · `THIN` (exists, but named sub-cases missing) ·
  `MISSING` (no test) · `N/A-divergent` (deliberate decision; the question is only
  whether a test *pins* it) · `FEATURE-GAP` (the language does not do this at all —
  a missing-feature question, not a missing-test question).

---

## 1. Executive summary — highest-value gaps, ranked

Ranking is by *(chance it hides a real bug) × (how many Python programs it
touches) ÷ (cost to add)*. Items 1–3 are cheap because the CPython oracle is free.

| # | Gap | Why it ranks here |
|---|---|---|
| 1 | **Float `str`/`print`/f-string/JSON rendering is not tested beyond trivial values — and appears wrong** (see Bug **B1**: `%g` ⇒ 6 significant digits, so `str(1/3)` is `0.333333`). | Touches every program that prints a float. The differential generator is int/bool-only and `float_str_integral.zap` only exercises integral floats, so the oracle never saw it. |
| 2 | **Widen the differential generator.** `diff_cpython.rs`'s seeded generator produces only bounded int/bool programs. Add float, str, list, dict (order-independent observations), comparison/precedence, and augmented-assignment generators. | One generator widening re-runs thousands of CPython-checked cases for free; this is where CPython's oracle is strongest and Zap's coverage thinnest. |
| 3 | **Operator semantics matrix is mostly TODO**: `**` (NUM-5), bitwise/shift (NUM-6), unary, **precedence & associativity** (`-2**2`, `2**3**2`, `not a == b`, `a if b else c if d else e`), **all 13 augmented-assignment ops** with *evaluate-once* subscripts/attributes. | Pure-expression ⇒ trivially diffable against CPython. Compound-assignment evaluate-once is called out in the divergence checklist (#3) but has no test. |
| 4 | **Language features absent *and untracked*** (not in the #614 ledger, which was a grammar pass): `try … else`, `for … else`, `while … else`; `except (A, B)`; the builtins `abs min max any all enumerate zip map filter reversed isinstance type` etc.; list `insert/extend/count/reverse`; set algebra (`union/intersection/…`); most `str` methods (`count rfind rsplit splitlines partition title center ljust rjust zfill isdigit isalpha …`); `str * int`; `frozenset`/`bytearray`. | These are the first things a Python reader reaches for. Needs a keep/defer decision recorded in `PYTHON_DIFFERENCES.md` — silently absent is the worst state. (Details §4–§6, §11.) |
| 5 | **Closure/scope semantics are documented-divergent but unpinned**: Zap captures *by value* (D-0067; "closures capture by value" in `PYTHON_DIFFERENCES.md`), Python late-binds cells. No test shows snapshot-vs-cell, the `[lambda: i for i in …]` classic, or module-binding rebind vs. Python's local-shadow. | A divergence with no pinning test is a divergence that will drift. |
| 6 | **Exceptions beyond the basics**: `finally` interactions (return/break/continue in `finally`, exception in `finally`/`except`), nested handlers, implicit chaining (`__context__`), `raise` forms, `AssertionError`, uncaught exit code + stderr shape, hierarchy. EXC-6 is TODO. | `try/finally` is where compilers most often diverge; only the happy paths are covered. |
| 7 | **Argument-passing error matrix**: duplicate keyword, missing/extra/unexpected arg, positional-only passed by name, keyword-only passed positionally, left-to-right argument evaluation when keywords are reordered, default-evaluated-per-call *count*. | SPEC §7 describes the errors in comments; none is asserted. |
| 8 | **Unpacking & sequence assignment**: subscript/attribute targets (`xs[i], xs[j] = xs[j], xs[i]`), nested, star in every position, length mismatch (runtime for lists), unpack from str/dict/range/generator, evaluate-once of RHS with subscript targets. | `swap` of *names* is covered (TUP-2); the idiomatic subscript swap is not. |
| 9 | **CLAIMS.md index drift** (cheap, high signal): 12 features shipped under #593–#606/#611/#657 have goldens but **no CLAIMS rows**; several TODO rows (OP-IN-1, DICT-3, COMP-2, TRUTH-4, …) are effectively exercised by goldens already. The conformance harness enforces index↔fixture round-trip, so un-indexed goldens are invisible to the "measurable coverage" story. | Makes the 61-covered / 43-TODO number honest. |
| 10 | **Negative ("must-reject") corpus** for the WON'T-DO and DEFERRED syntax (`global`, `x: int = 0`, `a := 10`, `del name`, `from m import x`, `async`, `yield from`, `except*`, `case {…}:`, `...`) and for the compile-time-vs-runtime divergences (ZAP6002, ZAP4073/4074). `error_codes.rs` is the only place; it has 23 code mentions. | The "walled garden" intent in AGENTS.md: if a deferred feature half-lands by accident, a test should notice. |
| 11 | **Numeric breadth**: bigint (str/parse/bitwise/shift/pow/mixed with int64), int⇄float conversion edges (`int(-3.9)`, `int(" 12 ")`), the implicit-widening ladder (NUM-7 TODO), bool-vs-int arithmetic, complex `repr` forms, float inf/nan/-0.0 comparisons. | `test_long`/`test_float`/`test_complex`/`test_bool` are big; Zap has one case each for bigint/complex. |
| 12 | **Strings/format**: non-ASCII `len`/`upper`/slice (Bug **B6**), format-spec edges (large magnitudes, Bug **B5**), pinned rejection of the unsupported f-string forms (`!r`, nested spec, `=`). | STR-3/4/5/6 and BYTES-1/2 are all TODO. |
| 13 | `match` edge semantics (subject evaluated once, guard order, or-pattern binding consistency, bool-vs-int literal, irrefutable-pattern-not-last, class/struct patterns). | `test_patma` is ~3k lines; Zap has `match_structural` + `match_as_pattern` + enum goldens. |
| 14 | Container method matrices & aliasing/mutation-during-iteration/equality (list/dict/set/tuple). | REF-1, SCAL-1, VSTRUCT-1, CMP-4, DICT-3/4 all TODO. |
| 15 | Generators: laziness/evaluation order, exhaustion, `finally` on abandoned generator, args evaluated eagerly vs lazily. | GENR-1 TODO. |

### Suspected actual Zap bugs found by comparing (reported, **not** fixed)

Detailed in §10. All are **(source-read)** unless stated; each needs a CI/`zapc run`
repro before filing.

| ID | Suspected bug | Confidence |
|---|---|---|
| **B1** | `str(float)`, `print(float)`, default f-string float, and JSON number output use C `%g` (6 significant digits) ⇒ `str(1/3)`→`0.333333`, `str(0.1+0.2)`→`0.3`, `str(1234567.0)`→`1.23457e+06`, `str(1e6)`→`1e+06`. CPython: shortest round-trip repr. **Silent precision loss in JSON output.** | High (code + modelled in CPython) |
| **B2** | `int ** negative-int` yields the integer floor (`2 ** -1` ⇒ `0`) instead of Python's float `0.5`. Possibly a deliberate static-typing choice, but it is undocumented (NUM-5 is TODO). | Medium — decision needed |
| **B3** | `x == None` / `x != None` passes sema but dies in lowering with ZAP6001; SPEC §5 says it is "consistent with `is None`". (Already noted in the #611 findings; not separately filed.) | High (prior finding) |
| **B4** | `<<` / `>>` lower to raw LLVM `shl`/shift with the count unmasked ⇒ count ≥ 64 or negative is LLVM poison. CPython: `1 << 64` is exact, negative count raises `ValueError`. | Medium (needs probe) |
| **B5** | `zap_fmt_flt` renders into a 96-byte buffer; `f"{1e100:f}"` / `{x:.60f}` are silently truncated (`snprintf` bounds the write, so not a memory bug, but the text is wrong). | Medium (needs probe) |
| **B6** | `len(str)` returns the **byte** length (`zap_str_len`), while indexing/slicing/iteration are codepoint-based — `len("héllo")==6`, so `s[len(s)-1]` is wrong for non-ASCII. SPEC §9 admits this as pending (#284), but STR-2 is ASCII-only so nothing flags it. | High (documented, untested) |
| **B7** | There is no `AssertionError` anywhere in `crates/`, `tests/`, or `SPEC.md`; the built-in exception list (SPEC §10) omits it. What `assert` raises, and whether `except AssertionError` resolves, is unspecified. | Medium (needs probe) |
| **B8** | `SPEC.md` §6 (`pass` paragraph) lists "`for`/`while` (and their `else`)" and `try/except/finally`, but the parser has no `else` on `for`/`while`/`try` (`parse_while`, `parse_for`, `parse_try`). Spec text and implementation disagree. | High (source-read) |
| **B9** | `float(s)` on a malformed string "faults" (SPEC §9) whereas `int(s)` raises a catchable `ValueError` (#456); CPython raises `ValueError` for both. | Low/Medium — confirm it is catchable |

---

## 2. Inventory of Zap's language-test surfaces

| Surface | Where | Size | What it asserts | Gate |
|---|---|---|---|---|
| **CPython differential (D)** | `tests/diff/*.zap`, `crates/driver/tests/diff_cpython.rs` | 34 curated cases (30 `# diff: match`, 4 `# diff: diverge`) + a **seeded generator of bounded int/bool programs** | stdout + exit status identical to `python3` for the shared subset; `diverge` cases assert they *differ* (and cite the decision) | host unit tests of the Zap→Python transform + generator (no LLVM); e2e rides `cargo test --features zapc/gc-*` in full/arm64 CI |
| **Spec conformance (C)** | `tests/conformance/` (+ `CLAIMS.md`, `README.md`), `crates/driver/tests/conformance.rs` | 54 cases; **61 claims covered / 43 TODO** | Zap's *own* specified output for a SPEC.md claim; index↔fixture round-trip enforced both ways | same wiring as golden |
| **Golden programs (G)** | `tests/golden/` (+ `golden_projects/` 11 multi-module cases), `crates/driver/tests/golden.rs` | 361 `.zap` + `.expected` (31 `.stderr`, 27 `.exit`, 8 `.env`) | exact stdout/stderr/exit; mix of language behavior and stdlib/runtime (GC stress, net, tls, json, …). ~half are language-behavior | full CI, both collectors; one quarantined on aarch64 |
| **Unit tests (U)** | `crates/{syntax,sema,ir,mono,codegen,diag,fmt,analysis,driver}` | ~1000 `#[test]` | lexer (28 incl. fstring nesting, continuation, tabs), parser, sema rules, lowering shapes, formatter, error codes (`error_codes.rs`, `crates/syntax/tests/grammar.rs` lints GRAMMAR.ebnf structure) | host `cargo test` |
| **Doctests (Doc)** | `zap test --doc` (#234), `crates/driver/tests/doctest.rs` + `doctest_fixtures/{pass,fail}.zap` | harness + 2 fixtures | the *harness* (`# =>`, `# raises:`, `# error:`); **no** language-conformance role found | host |
| **Divergence checklist** | `docs/DIVERGENCE_CHECKLIST.md` | 11 questions | PR-time process, not a test | PR template |
| **Grammar-parity ledger** | #614 epic; sibling-session notes (`grammar-gaps-cpython-vs-zap-REPORT.md`, `grammar-parity-DECISIONS.md`); `docs/PYTHON_DIFFERENCES.md` (PRs #615, #623) | D1–D35 | dispositions only; *folded in* below (§3) | — |

Two structural observations:

1. **D and C overlap but don't compose.** The differential layer (the only thing
   that checks Zap against an independent oracle) covers 34 hand-picked cases + an
   int/bool generator; the conformance layer checks Zap against *itself* (its own
   `.expected`). A conformance case can therefore pin a wrong-but-consistent
   output. B1 is exactly that shape.
2. **Goldens carry most real language coverage but aren't indexed.** 361 goldens
   exercise much of the language, but `CLAIMS.md` only sees C-suite cases. The
   12 features listed in §3.2 have goldens and no claim row.

---

## 3. Grammar / syntax axis (folded in from #614 / PRs #615, #623)

Not re-derived. Source: the ledger D1–D35, `docs/PYTHON_DIFFERENCES.md`, and the
GRAMMAR.ebnf/parser. "Test" is what exists **today**.

### 3.1 Dispositions and their tests

| Feature (ledger #) | Status | Zap test today | Verdict |
|---|---|---|---|
| `pass` (#591) | shipped | G `pass_noop` | COVERED (no CLAIMS row; contextual-keyword-as-identifier case not tested) |
| `not in` (#593) | shipped | G (7 files use it) | THIN — no CLAIMS row; "`not in` does not chain" and `not in` on dict/str/set untested |
| chained assign (#594) | shipped | G `chained_assignment` | THIN — evaluate-once RHS and mixed targets (`xs[0] = y = f()`) |
| `del d[k]`/`del xs[i]` (#595) | shipped | G `del_element` | THIN — negative index, missing key no-op (**divergent**, pending #22), `del xs[a:b]`, `del a, b` |
| generator exprs (#596) | shipped | C `genexpr`, G `genexpr_lazy` | COVERED (nested genexpr documented-unsupported → needs negative test) |
| starred call args (#597) | shipped | C `function_starred_args`, G `starred_call_args` | COVERED |
| display `*`/`**` (#598) | shipped | C `display_unpacking`, G `display_unpacking` | COVERED |
| multi `for`/`if` comps (#599) | shipped | C `comprehension_multi_clause`, G `comp_multi_clause` | COVERED |
| walrus (#600) | shipped | C `walrus`, G `walrus`, D `walrus` | COVERED |
| `**kwargs` param (#601) | shipped | C `function_kwargs`, G `kwargs_param` | COVERED |
| positional-only `/` (#602) | shipped | G `posonly_params` | THIN — no CLAIMS row; error cases (passed by name) unasserted |
| `as` sub-pattern (#603) | shipped | G `match_as_pattern` | THIN — no CLAIMS row |
| `raise … from` (#604) | shipped | G `raise_from_cause` | THIN — no CLAIMS row; chained uncaught traceback shape |
| `except*` (#605) | **not shipped** | — | N/A — add a negative test (see §3.3) |
| `type X = …` alias (#606) | shipped | G `type_alias` | THIN — no CLAIMS row; generic alias |
| anonymous unions (#607) | design-decision | — | N/A |
| set graduation / ctors (#608/#609) | shipped | C `set_constructors`, G `set_*` | COVERED |
| `is`/`is not` (#611) | shipped (D-0110, **divergent**: value types are a compile error) | G `is_identity`; D (`is not` ×2) | THIN — no CLAIMS row; ZAP4073/4074 negative cases |
| bare `raise` (#657) | shipped (D-0116, **divergent**: ZAP6002 compile-time) | G `reraise_*` (4) | THIN — no CLAIMS row; each ZAP6002 position (try body, `else`, `finally`, top-level, called-from-handler) |
| `int`/`float` synonyms (#582/#569) | shipped | G `scalar_int`, `scalar_float` | COVERED |
| Ellipsis, mapping patterns, rich lambda params, `yield from`, dotted-const patterns (#616–#620) | deferred | — | N/A — negative tests |
| t-strings (#612), `@` (#613), `await` synonym (#610) | deferred | — | N/A — negative tests |
| `global`/`nonlocal`, `x: int = 0`, `from M import`, `async`/`await`, metaclasses (D30–D34) | WON'T-DO | — | N/A — negative tests |
| full OO (#66/#30) | deferred | struct/interface goldens | N/A |

### 3.2 Features shipped without CLAIMS rows (index drift)

`pass`, `not in`, chained assignment, `del`, positional-only `/`, `as` patterns,
`raise … from`, bare `raise`, `type` alias, `is`/`is not`, `int` synonym,
`@cache`/behavioral decorators. Each has a golden; none is in `CLAIMS.md`, so the
conformance README's rule ("every language PR adds a conformance case + claim
row") was applied unevenly after #395.

### 3.3 Proposed negative corpus (must-reject)

Each should assert the *diagnostic code* (not the message), so it survives
rewording: `global x` / `nonlocal x`; `int64 x: int = 0`; `a := 10` as a statement
(SPEC §5); `del name` (SPEC §6); `from m import x`; `import *`; `async def`;
`yield from g`; `try: … except* E:`; `case {"k": v}:`; `...`; `lambda x=1: x`;
`t"…"`; `a @ b`; nested genexpr (SPEC §8.1); `f"{x!r}"`, nested format spec,
non-ASCII fill (SPEC §9.3, ZAP2017/ZAP4026); mixing positional after keyword;
`break`/`continue` outside a loop; `return`/`yield` outside a function; bare
`raise` out of context (ZAP6002, all five positions); `is` on a value type
(ZAP4073); `is None` on a never-None value (ZAP4074); reassigning a `const`
(ZAP3017); executable module-scope statement (D-0090).

### 3.4 Lexing / literals / line structure

| Behavior | CPython | Zap coverage | Verdict | Cases to add |
|---|---|---|---|---|
| Integer literals (`_` grouping, `0x/0o/0b`, leading-zero rule) | `test_grammar`, `test_int_literal` | U `numbers()`; G `scalars` | THIN | run-time value of `1_000`, `0o17`, `0b101`, `0XFF`; literal at the int64 boundary (`9223372036854775807`, `…808` ⇒ `bigint`, `-9223372036854775808`) |
| Float literals (`.5`, `1.`, `1e10`, `1_0.0_1`, `1E-3`) | `test_grammar`, `test_float` | U `numbers()` | THIN | evaluated value + `str()` of each form (couples to B1) |
| Imaginary literals | `test_complex` | C `complex_arithmetic` | COVERED | `0j`, `1e3j`, `1_0j` |
| String literals: escapes (`\n \t \x \u \U \N{}`), raw, triple-quoted, bytes, adjacent-literal concatenation | `test_string_literals`, `test_grammar` | U `strings_and_prefixes`, `triple_quoted_string_spans_lines`; G `string_utf8` | THIN | `"a" "b"` implicit concatenation (not mentioned in SPEC/GRAMMAR — probe whether it parses); unknown escape (`"\q"`); `\N{…}`; raw string ending in backslash; bytes escape ranges (`b"\xff"`, non-ASCII in `b""` ⇒ error) |
| Line structure: `\` continuation, implicit continuation, `;`, comments, blank lines | `test_grammar`, `test_tokenize` | U (4 tests) | THIN | run-time (golden) cases for `;`-separated statements and `\` continuation inside an expression and inside a string |
| Indentation: consistent spaces, tab error, dedent mismatch, mixed | `test_grammar`, `test_tabnanny` | U `tab_indentation_is_an_error`, `indentation_blocks` | THIN | inconsistent-dedent error; first-line-indented error; form-feed; deeply nested (100 levels) |
| Soft/contextual keywords usable as identifiers (`pass`, `del`, `from`, `fn`, `type`, `m`/`s` regex prefixes) | n/a (CPython has `match`/`case`/`type` soft keywords — `test_patma`, `test_type_aliases`) | U `m_is_identifier_without_delimiter`; SPEC App. A | THIN | one golden per soft keyword used as a variable/function/field name — a Zap-specific regression magnet (this is the "walled garden" case) |
| Unicode identifiers | `test_unicode_identifiers` | none found | MISSING (probe) | non-ASCII identifier accepted or cleanly rejected |
| Syntax-error diagnostics (location, message, recovery) | `test_syntax` (~1.5k assertions), `test_exceptions` (SyntaxError attrs) | `error_codes.rs`; U | THIN | see §3.3; plus error *line/column* for the common mistakes (#571 touched this), multi-error recovery, unclosed bracket/string at EOF |

---

## 4. Matrix: names, scope, closures, numerics

### 4.1 Names / scope / closures

| Behavior | CPython | Zap coverage | Divergent? | Verdict | Cases to add |
|---|---|---|---|---|---|
| LEGB resolution, shadowing, use-before-define | `test_scope` | LEX-1 TODO (forward refs); G `fib` | Zap: *static* lexical scope; definitions may follow uses (D-0021) | THIN | function defined after use; local shadows module binding; shadow with `auto x`; redeclare in same scope ⇒ error; use before declaration ⇒ compile error (Python: `UnboundLocalError`) |
| Block scoping (names declared in `if`/`for`/`while` bodies) | Python: function-scoped | none specific found | **Probably divergent** (type-first declarations are block-scoped?) — verify | MISSING (probe) | declare in `if`, read after; declare in loop body; same name in two sibling blocks with different types |
| Comprehension variable does not leak | `test_listcomps` | checklist #11 asserts it; G `comp_*`, genexpr doc says so | same | THIN | explicit "loop var absent after comp" negative test; walrus-in-comp leaks to enclosing (covered by C `walrus`) |
| Closures: capture, counter factories, nested | `test_scope` | G `fnval_closure_capture`, `fnval_closure_*`, `fnval_gc_stress` | **Divergent**: capture *by value* into the closure record (D-0067); no `nonlocal` | COVERED (basic) / **N/A-divergent but unpinned** | (a) mutate captured var after lambda creation: Python sees new value, Zap snapshot — pin Zap's; (b) `[lambda: i for i in range(3)]` late-binding classic; (c) closure that tries to rebind a captured local ⇒ error or local; (d) captured *reference* (list) mutation visible both ways; (e) capture-of-capture in nested lambdas; (f) value-struct captured by value (D-0067 lists as unbuilt) |
| `global` / `nonlocal` | `test_global`, `test_scope` | — | **WON'T-DO** (D30); Zap's bare assignment to a module-level name *rebinds the module binding* (opposite of Python's implicit local) | N/A-divergent | G `module_mutable_*` cover module state; add a **D `diverge`** case: same source, Python creates a local, Zap rebinds; plus a ZAP5125 sharing-lint golden |
| `del name` / unbound names | `test_scope`, `test_grammar` | — | N/A (static) | N/A | negative test (§3.3) |
| Recursion (mutual, deep) | `test_scope`, `test_sys` (`RecursionError`) | G `fib` | **Divergent**: no `RecursionError`; stack overflow ⇒ fault handler (design/0005 §fault signals, `sigaltstack`) | MISSING | mutual recursion; deep (≈100k) non-tail recursion; infinite recursion ⇒ assert the *documented* outcome (clean diagnostic + non-zero exit, not a hang) |
| Module-level state | n/a | G `module_*` (≈25), C `ENTRY-1` | Zap-only (D-0112) | COVERED | — |

### 4.2 Numerics

| Behavior | CPython | Zap coverage | Divergent? | Verdict | Cases to add |
|---|---|---|---|---|---|
| int `+ - *` (small) | `test_int`, `test_long`, `test_binop` | D `integer_ops`; D generator (int/bool) | — | COVERED | — |
| int overflow | `test_long` (bigint, no overflow) | C `numeric_int_wraps`, D `int_overflow` (diverge) | **Divergent**: default `int` is int64 and wraps (D-0005/D-0019) | N/A-divergent, **pinned (C+D)** | `INT64_MIN` edges: `-INT64_MIN`, `INT64_MIN // -1`, `INT64_MIN % -1`, `abs`-equivalent (probe: x86 `idiv` traps on `MIN/-1`; the runtime helper `zap_ifloordiv` should be checked) |
| `//` and `%` floor semantics, ints | `test_long`, `test_int`, `test_binop` | C `numeric_floor_division`, `numeric_floored_modulo`; D `floor_div_negatives`, `mod_negatives`; G `floored_modulo`, `divide_by_zero_*` | — | COVERED (ints) | identity `a == (a//b)*b + a%b` as a *generated* property over negatives |
| `//` and `%` on floats / mixed | `test_float`, `test_long` | none found | — | MISSING | `-7.5 // 2`, `7.5 // -2`, `5.5 % -2`, `inf // 1`, `x % inf`, int//float, `0.0 // 0.0` ⇒ `ZeroDivisionError` |
| `divmod`, `pow(a,b)`, `round`, `abs` builtins | `test_builtin` | not in `BUILTINS` | **FEATURE-GAP** | FEATURE-GAP | decide: provide, or document absence; note `math.round` is *half-away* (G `math_module`), while Python's `round()` is banker's |
| True division `/` | `test_float`, `test_long` | C `numeric_true_division` | — | COVERED | `int/int` exactness for operands > 2^53; `1/0`, `1.0/0.0`, `0/0.0` ⇒ Python raises `ZeroDivisionError` — **probe what Zap does** (IEEE inf/nan would be an undocumented divergence) |
| `**` | `test_pow`, `test_long`, `test_float` | NUM-5 **TODO**; incidental in 7 G / 5 C / 4 D files | **Possibly divergent**: int `**` negative ⇒ integer floor (lowering comment, `zap_ipow`); Python ⇒ float | MISSING | `2**10`, `0**0`, `(-2)**3`, `2**-1` (**B2**), `2**63` wrap vs bigint, float `**`, `(-8)**(1/3)` (Python ⇒ complex), right-assoc `2**3**2`, `-2**2`, `** ` with bigint |
| Bitwise `&` `\|` `^` `~` and shifts | `test_long`, `test_binop`, `test_unary` | NUM-6 **TODO**; D has one case | **Possibly divergent** (B4) | THIN | negatives (`-5 & 3`, `-1 >> 1`, `~x == -x-1`), shift ≥ 64, negative shift count ⇒ `ValueError`, `bool & bool`, precedence vs comparison (`a & b == c`), bigint shifts |
| bigint | `test_long` (vast) | C `bigint_arithmetic` (1 case), BIG-1 | opt-in type (Python's int is always arbitrary) | THIN | str()/format of huge values; `bigint("…")`/`int("…")` of > int64 text; `//`/`%` sign matrix; `**` growth; bitwise/shift; compare vs float exactness (`2**53+1 == 9007199254740993.0` is `False` in Python); mixed `int64 + bigint` (promotion or compile error?); hash/dict-key; `int64(big)` overflow error *type* |
| float arithmetic, inf/nan, `-0.0` | `test_float`, `test_math` | G `scalars`, `scalar_float`, `float_str`, `math_module` | — | THIN | `nan != nan`, `nan == nan`, `inf - inf`, `-0.0 == 0.0`, `str(-0.0)`, comparisons with nan in `<`, `max`-like behavior, `float("inf")`, `float("nan")` |
| **float `str`/`repr`** | `test_float` (`repr` round-trip), `test_format` | C `str_scalar_rendering`, G `float_str`, `print_scalar_values`, D `float_str_integral` — all trivial values | — | **THIN + suspected BUG B1** | shortest-round-trip corpus: `1/3`, `0.1+0.2`, `1e15`, `1e16`, `1e-4`, `1e-5`, `123456789.123`, `2**-1074`, `1.7976931348623157e308`, `5e-324`; as a **generated** D property (random doubles, `str(x)` equality) |
| float ⇄ int conversion | `test_float`, `test_int` | CONV-1/2 (malformed `int(s)`) | — | THIN | `int(3.9)`, `int(-3.9)`, `int(1e30)` (overflow ⇒ ?), `int(nan)`/`int(inf)` ⇒ errors, `float(10**30)`, `int("  12  ")`, `int("+5")`, `int("1_000")`, `int("1e3")` (error), `int("")`, `float(s)` malformed (**B9**), `float("1_0.5")` |
| complex | `test_complex` | C `complex_arithmetic` (CPLX-1) | **Divergent rendering**: always `(re±imj)` with `%g` components, Python prints `1j` for pure-imaginary and shortest-repr parts | THIN | `repr` forms (`1j`, `(1-0j)`, `-0j`, nan/inf parts), division by zero, `**`, mixed int/float/complex, `==` vs real, `abs()`/`conjugate()` equivalents, pin the divergent rendering |
| bool | `test_bool` | C `bool_strict_value`; G `scalars`; D `boolean_flow` | **Divergent**: `bool` is a distinct type, not an `int` (D-0022 strict bool) | THIN / N/A-divergent | `True + 1`, `True == 1`, `sum([True, True])`, `~True`, `True & False`, `str(True)`, `bool & int` — assert Zap's *chosen* behavior (compile error vs value); `int(True)`, `bool("")`, `bool(0.0)`, `bool(nan)` ⇒ `True` |
| Mixed-type coercion / widening ladder | `test_binop`, `test_numeric_tower` | NUM-7 **TODO**; G `int32_decimal`, `scalar_int` | Zap: implicit widening only, narrowing explicit (D-0020) | MISSING | `int32 + int64`, `int32` overflow wrap, `int + float`, `float32` vs `float64` rounding, `char` arithmetic, comparison across widths, narrowing without cast ⇒ compile error |
| Numeric conversions to text | `test_format` | STR-1 covered, STR-6 TODO | — | THIN | see §5.2 |
| Hash/equality across numeric types (`hash(1)==hash(1.0)`) | `test_hash`, `test_dict` | G `dict_float_key`, `set_float_member` (#803) | N/A (static key types; a `float64` key/member is its own type — `-0.0`/`+0.0` collide, `NaN` raises a catchable `ValueError`, D-0138) | N/A-divergent | an `int64` key and a `float64` key are distinct key *types* (no cross-type `1 == 1.0` lookup); `dict[int64,…]` indexed by a float still mismatches the key type |

---

## 5. Matrix: strings, bytes, formatting, containers

### 5.1 str / bytes / unicode

| Behavior | CPython | Zap coverage | Divergent? | Verdict | Cases to add |
|---|---|---|---|---|---|
| `len`, index, slice, iteration (codepoint) | `test_str`, `test_unicode`, `test_slice` | C `str_codepoint_ops` (STR-2, ASCII); G `string_index_slice`, `string_utf8`; D `string_slice` | SPEC says `len(str)` is **bytes** today (pending #284) while the rest is codepoint-based | THIN + **B6** | non-ASCII: `len("héllo")`, `"héllo"[-1]`, `"日本語"[1:]`, emoji (non-BMP), combining marks; slice step ≠ ±1 on multibyte; step 0 ⇒ `ValueError`; clamping of out-of-range slices; `s[len(s)-1]` |
| `+`, `*` repeat, `in`, compare | `test_str` | STR-3 TODO; CMP-3 covered; G `string_compare` | `str * int` unmentioned in SPEC (**FEATURE-GAP?**) | THIN | `"ab" * 3`, `"ab" * 0`, `* -1`, `"" in s`, `"a" < "B"`, `"ß" < "z"` |
| Methods | `test_str` (~50 methods) | STR-4 **TODO**; G `string_methods`; D `string_methods` | Case/space ops are **ASCII-only** in v1 (documented) | THIN + **FEATURE-GAP** | present: `upper lower strip lstrip rstrip split join find index replace startswith endswith contains encode`. Absent from `sema`/`ir` (string search): `count rfind rindex rsplit splitlines partition rpartition title capitalize swapcase center ljust rjust zfill expandtabs isdigit isalpha isalnum isspace isupper islower format`. For present ones: `strip(chars)`, `split()` no-arg (whitespace), `split(sep, maxsplit)`, `replace(old,new,count)`, empty `sep` ⇒ `ValueError`, `"é".upper()` (pin the ASCII-only divergence) |
| `%`-formatting and `str.format` | `test_str`, `test_format` | none | **Not in SPEC** — f-strings only | FEATURE-GAP (undocumented) | record in `PYTHON_DIFFERENCES.md` Part A/B or implement |
| f-strings | `test_fstring` (~2k lines) | STR-5 **TODO**; G `fstrings`, `fstring_nested_quote`, `fstring_percent_encode`; U 5 lexer tests | Documented limits: `!r`/`!a`, nested `{…}` in spec, non-ASCII fill, non-literal spec ⇒ unsupported (SPEC §9.3) | THIN | `{x=}` self-documenting (not mentioned — probe); `{{`/`}}`; multi-line / triple-quoted f-string; lambda/dict/ternary/walrus in field; string with the other quote inside field; `{x!s}`; `{x:>10}`/`^`/`<` with fill; `{n:,}` `{n:_}` (not in the Zap subset ⇒ pin rejection); `{x:%}`; negative tests for each documented limit |
| `format()` / format-spec | `test_format`, `test_types` | STR-6 **TODO**; G `fstrings` (partial) | subset of the mini-language | THIN + **B5** | sign (`+`, ` `), `#`, `0`-pad with sign, `=` align, `.precision` on str (truncate), types `b o x X d e E f F g G s`; rounding (`format(2.675,'.2f')`==`2.67`, `.0f` half-even on exact ties); `inf`/`nan`; huge magnitudes (`1e100:f`, `.60f`) and big widths; bool with `d`; char/str with `s` |
| `print()` | `test_print`, `test_builtin` | G 128 files | `print(sep=, end=, file=, flush=)` — 4 golden hits only | THIN | `sep`, `end`, empty `print()`, print of nested containers/None/Optional |
| `repr` vs `str` | `test_str`, `test_repr` | G `repr_*` (8), C Display protocol (§9.4) | **Divergent**: Zap draws no repr/str quoting distinction (D-0124) except inside containers (G `repr_string_quoting`) | N/A-divergent, pinned (G) | pin `print("a")` vs `print(["a"])` vs `f"{['a']}"` consistency (SPEC says they agree) |
| bytes | `test_bytes` | BYTES-1/2 **TODO**; G `bytes_basics`, `bytes_invalid_utf8`, `bytes_sha256`, `hex_codec` | `bytearray`, `bytes % …`, `.hex()`/`fromhex` partially | THIN | index ⇒ int, slice ⇒ bytes, concat/repeat, comparison, `in`, `decode` errors, `bytes(list)` range check (`256` ⇒ `ValueError`), `bytes(-1)` |
| Unicode semantics (normalization, case folding, `isidentifier`, surrogates, encode error handlers) | `test_unicode`, `test_codecs` | none | N/A (UTF-8 `str`; lone surrogates can't exist) | N/A-divergent | pin: no surrogate escapes (`"\ud800"` ⇒ error) |

### 5.2 Containers

| Behavior | CPython | Zap coverage | Divergent? | Verdict | Cases to add |
|---|---|---|---|---|---|
| list construction/indexing/append/pop/remove | `test_list`, `list_tests`, `seq_tests` | G `list_int/str/float`, `list_negative_index`, `list_membership`, `list_sort`; C `list_*`; D `list_index` | — | COVERED (core) | — |
| list methods | `test_list` | partial | `insert extend count reverse index(start,end)` — **no hit in sema/ir** ⇒ FEATURE-GAP; `sort(key=, reverse=)` | THIN + FEATURE-GAP | each method on empty/one/many; `pop()` on empty ⇒ `IndexError`; `remove` absent ⇒ `ValueError`; `index` absent ⇒ `ValueError`; stability of `sort` with equal keys (D-0083 claims stable) |
| list operators: `+`, `*`, `+=` aliasing, slice assign/delete, comparison | `test_list`, `test_augassign` | SLICE-1/IDX-1 covered (read side) | slice assignment/deletion likely unsupported | MISSING | `a=b; a+=[1]` (mutates shared list in Python; Zap `+=` is **derived**/in-place per D-0047 — pin it), `xs[1:3]=[…]`, `del xs[1:3]`, `[0]*3`, `[[0]*2]*2` aliasing trap, lexicographic `<` on lists |
| list/dict/set/tuple **equality** | `test_list`, `test_dict`, `test_set`, `test_tuple` | CMP-4 **TODO**; SPEC §5: "collections and user types define their own equality" | needs clarification — does `[1,2]==[1,2]` work? | MISSING (probe) | structural `==`/`!=` incl. nested, differing length, different element order (set/dict), `list == tuple` (⇒ `False`/compile error) |
| dict basics: get/set/missing/`in`/len/iterate | `test_dict` | C `dict_*`; G `dict`, `dict_get`, `dict_get_default`, `dict_iter`, `dict_str_iter`, `dict_word_count`; D `dict_lookup` | `d[k]` missing: DICT-4 **TODO**; iteration order **hash order**, not insertion (D-0056) | COVERED / THIN | `d[missing]` ⇒ catchable fault (DICT-4); `.items()` (DICT-3 TODO but used in 4 goldens); `pop`, `setdefault` (**absent**), `popitem` (**absent**), `update` (3 G), `clear`, `copy`; dict equality independent of insertion order; delete/insert **during iteration** (Python ⇒ `RuntimeError`; Zap ⇒ ?) |
| dict order | `test_dict`, `test_ordered_dict` | D `dict_order` (diverge, D-0056) | **Divergent** | N/A-divergent, **pinned (D)** | keep; add: `kwargs` dict order (also unspecified), `**` merge order, dict comprehension order, `list(d)` order stable across two runs of the *same* binary |
| dict views live-ness (`keys()` reflects later mutation) | `test_dictviews` | none | Zap `keys()`/`values()` semantics unspecified | MISSING | snapshot vs view; `k in d.keys()`; `d.keys() & other` (set-like) — likely unsupported |
| set | `test_set` (~2k lines) | C `set_constructors`; G `set_int/str/tuple/construct`, `oset`; D none | `frozenset` absent; set operators/methods absent | THIN + FEATURE-GAP | `\|`, `&`, `-`, `^`, `<=`, `<`, `union/intersection/difference/issubset/issuperset`, `discard`, `remove` absent ⇒ `KeyError`, `pop`, `clear`, set equality, set of tuples (G), iteration-while-mutating |
| tuple | `test_tuple` | C `tuple_unpack`; G `tuple_basic/multiret/rest/swap/dict_key/set_elem`, `tuple_key_unhashable_reject` | **Divergent**: value aggregate with compile-time-constant index (TUP-4 TODO; D-0049) | COVERED (core) / N/A-divergent | ordering `(1,2) < (1,3)`, `==` on nested, concat/repeat, `len`, `in`, `t[-1]` (constant negative), `t[i]` non-constant ⇒ compile error (pin TUP-4), `tuple(list)`, 1-tuples `(1,)`, empty tuple |
| comprehensions | `test_listcomps`, `test_setcomps`, `test_dictcomps` | COMP-1/3/4, DSP-1 covered; COMP-2 TODO; G `comp_*` | — | COVERED / THIN | dict-comp duplicate keys (last wins), evaluation order of `for`/`if`, comp over dict/str/set/range-with-step, nested comprehension, comprehension in a method referencing `self`, empty iterable, side effects in element expr evaluated exactly once |
| aliasing / reference vs value | `test_copy`, `test_list` | SCAL-1, REF-1, VSTRUCT-1 **TODO**; G `value_struct*` | **Divergent by design** (D-0023/D-0051): scalars/tuples/value structs copy; list/dict/struct share | THIN / N/A-divergent | same-source D `diverge` where Python aliases and Zap copies (tuple of ints, value struct); list-of-list aliasing; pass list to function and mutate; struct assign shares |
| `in`, `len`, `min`/`max`/`sum`/`sorted` on containers | `test_builtin`, `test_contains` | OP-IN-1 TODO (G covers dict/str); G `sum` in 12, `sorted` in 1 | `min`/`max`/`any`/`all` not builtins | THIN + FEATURE-GAP | `sum([])`, `sum(floats)`, `sum(start)`, `sorted(reverse=)`, `sorted` of strs/tuples |

---

## 6. Matrix: iteration, truthiness, operators, subscripting, unpacking

| Behavior | CPython | Zap coverage | Divergent? | Verdict | Cases to add |
|---|---|---|---|---|---|
| `for` over list/str/dict/set/tuple/range/generator/channel | `test_iter`, `test_range`, `test_grammar` | CTRL-4 **TODO**; G `range_iter`, `dict_iter`, `generator_for` | iteration protocol is `__iter__` + comma-ok `__next__` (D-0048) | THIN | iterate each builtin; mutate the list being iterated (append/remove) — Python semantics defined, Zap's? ; `for` target unpack (`for k, v in d.items()`, `for i, (a, b) in …` — `enumerate` absent); loop variable after loop ends / empty loop |
| `range` | `test_range` | CTRL-3 (2-arg only) | — | THIN | `range(n)`, 3-arg with negative step, empty ranges, `range` as `in`/`len`/index/slice (probe), step 0 ⇒ `ValueError` |
| `break`/`continue`, nested loops, `for/while … else` | `test_grammar` | G 7 `break`, 6 `continue` files; C `control_flow` | **`for`/`while … else` absent** (B8) | THIN + FEATURE-GAP | `break` out of nested loops, `continue` in `finally`; **decide** `for/else`, `while/else` |
| `do … while` | n/a (Zap-only) | DOWHILE-1 TODO | Zap-only (D-0039) | THIN | body-once, `continue` inside do-while jumps to the test, `break` |
| generators: `yield`, exhaustion, laziness | `test_generators`, `test_genexps` | GENR-1 **TODO**; G `generator_*` (10), `genexpr_lazy`; C `genexpr` | `yield` only in statement position, operand required; no `send`/`throw`/`close`/`return value`; no `yield from` (D25 deferred) | THIN | evaluation interleaving print-order, argument evaluated at call vs first `next`, re-iterating an exhausted generator, generator in `sum()`/`list()`/`in`, `finally` in a generator when the consumer `break`s (Python runs it on close; probe Zap), generator as a method (G covers), recursive generator |
| `yield from` | `test_yield_from` | — | deferred (#619) | N/A | negative test |
| truthiness | `test_bool`, `test_grammar` | TRUTH-1/2/3 covered; TRUTH-4 (`None`) TODO; G `container_truthiness`, `string_truthiness` | — | COVERED / THIN | `None`, `Optional` empty/Some, `()` vs `(0,)`, empty set, `b""`, `'\0'` char, `nan` (truthy), `-0.0`, `0j`, bigint `0`, user struct in `if` (compile error?), truthiness inside comprehension `if`, `while`, ternary, `not` |
| `and`/`or`/`not` | `test_grammar`, `test_bool` | BOOL-1/2 covered, BOOL-3 TODO; G `short_circuit`; D `short_circuit`, `and_or_value` (diverge) | **Divergent**: yields strict `bool`, not operand (D-0022) | N/A-divergent, **pinned (C+D)**; short-circuit COVERED | `not` on each type ⇒ strict bool (BOOL-3); side-effect counters on both operands, nested `a and (b or c)`, `x or default` idiom ⇒ the divergent result (a Python-reader trap; keep D `diverge`) |
| Ternary | `test_grammar` | G (4 files); D `boolean_flow` | — | THIN | nesting right-assoc, with lambda, evaluated-once branches, precedence vs `or`, type unification of branches (`1 if c else 2.0`) |
| Chained comparison | `test_grammar`, `test_richcmp` | CMP-1/2 covered (C, G, D) | — | COVERED | 4+-link chains, `a < b == c`, mixed with `in`/`is`/`not in` (the latter does **not** chain), short-circuit stops evaluating later operands, user `__cmp__` operands |
| Rich comparison on user types | `test_richcmp`, `test_class` | OP-3 **TODO**; G `operators`, `operator_derive`, `operator_index_call` | **Divergent**: single `__cmp__` + derived relations; no reflected ops/`NotImplemented` (D-0047) | THIN / N/A-divergent | `==` vs `__eq__` w/o `__hash__`; sort of user type via `__cmp__`; `!=` derived; comparing different user types ⇒ compile error |
| Arithmetic operator overloading | `test_binop`, `test_class` | G `operators` | dunder set per D-0047 | THIN | `__radd__`-style reflected (absent — pin), `__neg__`, `__bool__`/`__len__` truthiness hook (probe) |
| Augmented assignment | `test_augassign` | G `iadd` (user dunders only); D uses `+=` incidentally | in-place for types defining `__iop__`, else derived (D-0047) | **MISSING** | all of `+= -= *= /= //= %= **= <<= >>= &= \|= ^=` on int/float/str; `xs[f()] += 1` — `f()` once; `obj.a += 1` — `obj` once; `list += list` aliasing; `str +=`; `x += 1` where `x` undeclared ⇒ error; `const` target ⇒ ZAP3017; tuple element target ⇒ error |
| Unary `+ - ~ not` | `test_unary` | U; incidental | — | THIN | `-INT64_MIN`, `- -x`, `+True`, `-0.0`, `~` on bool/float ⇒ error |
| Precedence & associativity | `test_grammar`, `test_binop` | U (parser) | — | **MISSING** as a run-time/diff test | table of ≥ 40 expressions diffed against CPython: `2**3**2`, `-2**2`, `not a == b`, `a or b and c`, `a < b \| c`, `1 << 2 + 3`, `a if b else c if d else e`, `lambda: x if y else z`, `-x ** -y`, `a - b - c`, `a / b / c`, `a // b * c`, `x < y < z` vs `x < (y < z)` |
| Identity `is` | `test_grammar` | G `is_identity` | **Divergent**: value types are a compile error (D-0110) | N/A-divergent, partly pinned | ZAP4073/4074 negatives; `is` on two distinct equal structs; `Optional` `is None`/`is not None`; chains |
| `x == None` | n/a | none | SPEC §5 claims works | **B3** | probe: `Optional[T] x; x == None`, `x != None` |
| Subscription / slicing | `test_slice`, `test_index`, `test_list`, `test_str` | IDX-1, SLICE-1 covered; G `list_slice`, `string_index_slice`, `array_slice` | list slice = copy, array slice = view (D-0030) | THIN | out-of-range index on both sides (`xs[len]`, `xs[-len-1]` ⇒ `IndexError`), slice clamping (`xs[-100:100]`), `xs[::-1]`, `xs[::2]`, `xs[5:2]` empty, step 0 ⇒ `ValueError`, slice assignment/deletion (probably unsupported — pin), `d[k]` on missing, tuple const index, nested `xs[i][j]`, `s[i]` char type |
| Unpacking / sequence assignment | `test_unpack`, `test_unpack_ex` | TUP-1/2/3 covered; G `tuple_rest`, `tuple_swap`, `tuple_multiret`, `chained_assignment`; D `tuple_unpack` | tuple arity is compile-time (D-0049) | THIN | `xs[i], xs[j] = xs[j], xs[i]`; `a, (b, c) = …`; `a, *b = …`, `*a, b = …`, `a, *b, c = …`, empty star; from list (length mismatch ⇒ runtime `ValueError`), from str, from `d.items()` item, from `range`, from generator; target attributes/subscripts; RHS evaluated fully before any store (TUP-2 for non-name targets); `a, b = b, a+b` loop (fib) in D |

---

## 7. Matrix: functions, exceptions, with, match, decorators

### 7.1 Argument passing

| Behavior | CPython | Zap coverage | Divergent? | Verdict | Cases to add |
|---|---|---|---|---|---|
| positional / keyword / default | `test_extcall`, `test_call`, `test_defaults` | FN-2/3 covered; G `default_keyword_args`, `variadic_args`; D `variadic_args` | — | COVERED | — |
| default evaluated per call | `test_defaults` (once-at-def) | C `function_default_per_call` (FN-1); D `default_arg_once` (diverge) | **Divergent** (D-0038) | N/A-divergent, **pinned (C+D)** | side-effecting default expression: assert it runs once **per call** (counter); default referencing a module const / earlier param (illegal in Python; legal in Zap?) |
| `*args`, `**kwargs`, call-site spread | `test_extcall` | FN-4, FN-8, FN-9 covered; G | **Divergent**: homogeneous (`T... xs`, `V **kw`) ⇒ `dict[str,V]`; type-first | COVERED | spread a generator / dict / empty; duplicate key via `**` and explicit kw ⇒ error; `**` merge order; `*args` after keyword |
| keyword-only (`*`) | `test_keywordonlyarg` | FN-7 covered; C `function_keyword_only` | same | COVERED | error cases: missing required kw-only; kw-only passed positionally (SPEC §7 comments only) |
| positional-only (`/`) | `test_positional_only_arg` | G `posonly_params` | same | THIN | pos-only passed by name ⇒ error; pos-only + `**kwargs` same name (legal in Python); default after `/` |
| **error matrix**: too many/few args, unexpected/duplicate keyword, positional after keyword | `test_extcall`, `test_call` | none found | compile-time in Zap | MISSING | one compile-fail case each (assert the ZAP code) |
| argument evaluation order (left-to-right, keywords reordered) | `test_call`, `test_grammar` | none | — | MISSING | `f(b=g(), a=h())` — `g`,`h` order; nested calls; args evaluated before callee when callee is an expression |
| `lambda` | `test_lambda`? (`test_grammar`) | FN-5 **TODO**; G 13 files | rich params deferred (D24) | THIN | lambda in default arg, lambda returning lambda, immediately-invoked, lambda closing over loop var (B2 interplay), lambda as `sorted` key (`sorted(key=)` — absent?) |
| first-class functions / closures | `test_funcattrs`, `test_scope` | FN-6 **TODO**; G `fnval_*` (7) | static `fn(...)` types (D-0067) | COVERED (G) / unindexed | add CLAIMS row; function stored in list/dict/struct (G covers); recursive lambda |
| recursion | see §4.1 | — | — | MISSING | — |

### 7.2 Exceptions

| Behavior | CPython | Zap coverage | Divergent? | Verdict | Cases to add |
|---|---|---|---|---|---|
| raise/catch built-in faults | `test_exceptions` | EXC-1..5,7 covered; G `builtin_fault_*`, `divide_by_zero_*`, `none_access_*`, `exceptions_*` | `e` is the **message `str`**, not an exception object (SPEC §10) | COVERED | — |
| `try/except/else/finally` | `test_try`, `test_exceptions` | G `try_finally_early_exit`, D `try_finally_control` | **`else` clause absent** (B8) | THIN + **FEATURE-GAP** | decide on `try … else`; `finally` runs on return/break/continue in `try`/`except`; **return/break/continue *inside* `finally`** (overrides in Python); exception raised in `finally` replaces the original; exception inside `except`; nested try in `finally`; `finally` w/ generator |
| multiple `except`, ordering, subclass matching | `test_exceptions` | EXC-7 covered | — | COVERED | catch-all `except:` not last ⇒ error (SPEC says must be last); duplicate unreachable clause |
| `except (A, B):` tuple | `test_exceptions` | — | grammar allows a single `type` only | **FEATURE-GAP** | decide / document |
| `raise` forms (`raise E`, `raise E("m")`, `raise "str"`, class vs instance, bare) | `test_raise` | EXC-5; G `reraise_*` (4), `exceptions_uncaught` | `raise "str"` ⇒ `Exception(msg)` shorthand (divergence); `raise E` without call? | THIN | `raise ValueError` (no parens) ⇒ ?; raise non-exception ⇒ error; re-raise preserves type & identity (G); `raise` in generator |
| `raise … from` / explicit cause / implicit context | `test_raise`, `test_exceptions` | G `raise_from_cause` | explicit cause only; **implicit `__context__` chaining** ("During handling…") not specified | THIN | uncaught traceback shape for chained exceptions (stderr golden); `from None` (suppress) — probe; context vs cause; `e.cause` accessor |
| built-in exception hierarchy | `test_exception_hierarchy` | SPEC §10 list (15 types) | Python has ~60 (`AttributeError`, `NameError`, `OSError`, `OverflowError`, `AssertionError`, `NotImplementedError`, `UnicodeError`, `KeyboardInterrupt`, `SystemExit`, …) | MISSING + **B7** | pin `issubclass` relations that exist (`KeyError`⊂`LookupError`⊂`Exception`); `Cancelled` derives `BaseException` (SPEC) — a golden; what `int64(big)` overflow, `assert`, `sys.exit` raise |
| user-defined exceptions | `test_exceptions` | G `exceptions_*`, C `exception_typed_dispatch` | one string message only; no fields/`args` | THIN | exception hierarchy of user types, catching base, retaining identity across frames (SPEC says yes), user exception with extra fields (unsupported — pin) |
| uncaught exception ⇒ exit 1 + traceback | `test_exceptions`, `test_cmd_line` | EXC-6 **TODO**; G `exceptions_uncaught`, `*_uncaught` (stderr/exit sidecars) | stderr shape differs from CPython | THIN | pin exit code for: uncaught user exc, uncaught in spawned coroutine, in `finally`, `sys.exit(n)` (G `sys_exit`), exception in generator |
| `assert` | `test_grammar`, `test_exceptions` | G `assert_pass`, `assert_fail` | **power-assert** diagnostic (D-0054), type unspecified (**B7**) | THIN | `assert cond, msg` — `msg` evaluated lazily (only on failure); `assert` of tuple; `except AssertionError`; side effects in condition evaluated once |
| `except*` / `ExceptionGroup` | `test_exception_group` | `ExceptionGroup` raised by `scope` (G `scope_*`, C SCOPE-*) | `except*` deferred (#605) | N/A | negative test; group `.exceptions` accessors if any |
| faults catchable (D-0007) | n/a (Python faults are exceptions) | EXC-1/3/4 covered | — | COVERED | `INT64_MIN // -1` (hardware trap ⇒ must be a catchable fault, or defined), stack overflow, OOM (D-0119 aborts — pin), `int(s)`/`float(s)` (B9), `chr(0x110000)` ⇒ `ValueError`, `bytes([256])` |

### 7.3 `with` / context managers

| Behavior | CPython | Zap coverage | Divergent? | Verdict | Cases to add |
|---|---|---|---|---|---|
| enter/exit on normal, return, break/continue, exception | `test_with` | WITH-1 **TODO**; G `with_early_return`, `with_exit_on_unwind`, `with_open_as`, `with_lock`, `with_multi`, `with_expect_raises` | `__exit__(self)` takes **no exception triple and cannot suppress** (v1, D-0069) | COVERED (G) / N/A-divergent (suppress) | index WITH-1; **exit order for multiple items is reverse**; exception in `__enter__` (no `__exit__` call); exception in `__exit__` replaces body's; `as` target unpack (`with a as (x, y)`); nested with + generator; with inside loop with `continue`; pin "cannot suppress" (a `with` body raising still propagates even if `__exit__` returns truthy-looking value) |
| contextlib-style generator CMs | `test_contextlib` | — | not provided | FEATURE-GAP | document |

### 7.4 `match`

| Behavior | CPython | Zap coverage | Divergent? | Verdict | Cases to add |
|---|---|---|---|---|---|
| literal / capture / wildcard / or / guard / sequence / struct / star | `test_patma` | MATCH-1 covered (C `match_structural`); G `enum_*`, `match_as_pattern`, `generics_*` | exhaustiveness checked at compile time for enums (Zap-only); struct (not class) patterns | THIN | subject evaluated exactly once; guard side effects & fall-through to next arm; literal patterns for `None`/`True`/`False`/negative numbers/floats/strings/`char`; **`case 1` vs `True`** (Python: `True == 1` matches; Zap strict bool?); or-pattern alternatives must bind the same names ⇒ error; irrefutable pattern not last ⇒ error; duplicate capture name in one pattern ⇒ error; nested star positions; `case [x, *_]`; string/bytes subjects; match with no matching arm and no `_` (non-enum) ⇒ ? |
| mapping patterns | `test_patma` | — | deferred (#617) | N/A | negative test |
| value patterns (`case Color.RED:`) | `test_patma` | G `enum_tier1`, `enum_shape` | shipped (D29) | COVERED | dotted *non-enum* constant (`case mod.CONST`) is deferred (#620) ⇒ negative test |
| class patterns with `__match_args__` | `test_patma` | struct patterns (positional by decl order or by name, D-0046) | **Divergent**: no `__match_args__` | N/A-divergent | pin positional-by-declaration-order and by-name equivalence |

### 7.5 Decorators, `del`, `assert`, `pass`, walrus, identity

| Behavior | CPython | Zap coverage | Divergent? | Verdict | Cases to add |
|---|---|---|---|---|---|
| decorator application, stacking order, args | `test_decorators` | DEC-1/DEC-2 **TODO** (index only); G `decorators`, `behavioral_decorator_*` (9), `fn_decorator_*`, `field_decorators`, `cache_*` (4), `builtin_deprecated` | **Divergent**: compile-time code generation (D-0036/D-0108), not a runtime rebind; arbitrary-expression decorators (PEP 614) not allowed; class decorators deferred with OO | COVERED (G) / N/A-divergent | flip DEC-1/2; decorator on method; decorator evaluation order (expression evaluated before def, applied bottom-up); decorator returning non-callable (G `wrap_noncallable`); negative: `@some_runtime_callable` not declared as `decorator` ⇒ compile error |
| `del` | `test_grammar` | G `del_element` | element-only (D3) | THIN | §3.1 |
| `pass` | `test_grammar` | G `pass_noop` | — | COVERED | `pass` as identifier |
| walrus | `test_named_expressions` | C `walrus` (WAL-1), G, D | **Divergent**: parentheses always required; bare `a := 10` rejected | COVERED | walrus in comprehension iterable (illegal in Python) ⇒ error; in `lambda`; target shadowing a comprehension var; nested walrus; walrus at module level ⇒ error |

---

## 8. Deliberate divergences — and whether a test pins each

| Divergence | Decision | Pinned? |
|---|---|---|
| dict/set iteration is hash order, not insertion order | D-0056 | **Yes** — D `dict_order` (diverge), C `dict_iteration` |
| `and`/`or` return a strict `bool`, not an operand | D-0022 | **Yes** — C `bool_strict_value`, D `and_or_value` |
| default arguments evaluated per call | D-0038 | **Yes** — C `function_default_per_call`, D `default_arg_once` |
| default `int` is int64 and wraps | D-0005, D-0019 | **Yes** — C `numeric_int_wraps`, D `int_overflow` |
| bigint is opt-in (`bigint`), literals beyond int64 become `bigint` | D-0019 | Partly — C `bigint_arithmetic` |
| complex renders `(re±imj)` with `%g` parts | D-0019 | Partly — C `complex_arithmetic`; pure-imaginary form unpinned |
| `is`/`is not` on value types is a compile error | D-0110 | Partly — G `is_identity`; ZAP4073/4074 negatives unpinned |
| bare `raise` outside a handler is a compile error (ZAP6002) | D-0116 | Partly — G `reraise_*`; each position unpinned |
| closures capture by value; no `nonlocal`/`global`; bare assignment rebinds module binding | D-0067, D-0112 | **No** |
| `e` in `except E as e` is the message `str` | D-0007/D-0029 | Yes (C `exception_typed_dispatch`) |
| `with` `__exit__` cannot suppress | D-0069 | **No** |
| decorators are compile-time, not runtime rebinding | D-0036, D-0108 | Yes (many G) |
| tuples are value aggregates; compile-time-constant index | D-0049 | **No** (TUP-4 TODO) |
| `bool` is not an `int` | D-0022 | **No** |
| `del d[missing]` is a no-op (pending #22) | SPEC §6 | Unclear — confirm `del_element` covers it |
| comprehension/genexpr variables don't leak (same as Python) | D-0026 | Not pinned (negative leak test missing) |
| list slice copies, array slice views | D-0030 | Yes (C `list_slice`, `array_contract`) |
| string case/space ops ASCII-only | D-0025 | **No** |
| `len(str)` = bytes (pending) | SPEC §9 | **No** — and it contradicts codepoint indexing (B6) |
| `%`/`str.format`, `repr()` quoting distinction, `type()`, `isinstance`, `hash()`, `id()` absent | not recorded | **Not a recorded decision** — add to `PYTHON_DIFFERENCES.md` |

---

## 9. Builtins parity (`test_builtin`)

`BUILTINS` in `crates/sema/src/lib.rs` lists `print range len sum sorted ord chr
char str int float bigint complex bool format` plus exception names and reflection
helpers. A string search of `crates/sema` and `crates/ir` for the following finds
**no** registration, and none appears in SPEC.md, `PYTHON_DIFFERENCES.md`, or any
fixture: `abs min max any all enumerate zip map filter reversed isinstance type
iter next id hash repr divmod pow round hex bin oct callable getattr hasattr input
open(as builtin) slice vars dir frozenset bytearray`. (`min`/`max` appear only as
the heap `cmp` keyword; `hex` and `round`/`pow` exist only under stdlib modules.)

Two readings: (a) intentional minimalism — then `PYTHON_DIFFERENCES.md` Part A/B
should say so, because it currently opens with "most Python code reads the same in
Zap"; (b) oversight — then these are the highest-traffic missing features.
Either way the conformance suite has no row (the #614 ledger is a *grammar* pass).
**Needs a decision, not a test.** Once decided, each present builtin needs a
CPython-diffable case (the differential harness fits `abs/min/max/any/all/sum/
enumerate/zip/sorted/reversed` perfectly).

---

## 10. Suspected bugs — evidence and repro sketches

Per the task, none is fixed here. Each needs a `zapc run` repro in CI before an
issue is filed; the repro should be written **first** as a failing test (AGENTS.md
test rule).

**B1 — float rendering uses `%g`.** `zap_f64_fmt_g`
(`crates/codegen/src/zaprt.c:2562`) calls `snprintf("%g")`, then appends `.0` only
if the text is all digits. `zap_f64_to_str` (`:2590`) is the single path for
`str(float)`, bare `print`, default f-string interpolation, the `:g` default in
`zap_fmt_flt`, and JSON number serialization (`crates/ir/src/lower/modules/json.rs`).
`%g` keeps 6 significant digits. Modelled in CPython (same formatter), against
`str()`:

| value | Zap (modelled) | CPython |
|---|---|---|
| `1/3` | `0.333333` | `0.3333333333333333` |
| `0.1 + 0.2` | `0.3` | `0.30000000000000004` |
| `1234567.0` | `1.23457e+06` | `1234567.0` |
| `1e6` | `1e+06` | `1000000.0` |
| `123456789.5` | `1.23457e+08` | `123456789.5` |
| `3.141592653589793` | `3.14159` | `3.141592653589793` |

`tests/golden/float_str.zap` and `scalars` only use values that survive 6 digits.
Repro: `print(1/3)` and compare to `python3`. The consequence for `json.dumps` is
silent precision loss in an API/AWS-SDK context. Fix direction (not done): shortest
round-trip (`%.17g` + trim, or Ryu/`dtoa` mode 0) with CPython's exponent
thresholds (`repr` switches to exponent at `1e16` and below `1e-4`).

**B2 — `int ** negative` is an integer.** `lower.rs` (`Pow`, all-integer branch)
documents "a negative exponent yields the floor of the true power — 0 for |base| > 1"
via `zap_ipow`. Python: `2 ** -1 == 0.5`. May be a deliberate static-typing
consequence; if so it needs a decision in `DECISIONS.md` and a pinned conformance
case. NUM-5 is TODO so it is currently undocumented.

**B3 — `== None` fails in lowering.** Carried over from the #611 findings:
`Expr::None` has no standalone arm in `ir/lower.rs::expr()`, so
`Optional[T] x; x == None` type-checks and then ZAP6001. SPEC §5 and GRAMMAR say it
is "consistent with `is None`".

**B4 — shifts are unmasked.** `Shl`/`Shr` lower to `Inst::Bin` with the raw
right operand (`lower.rs` ~17058). LLVM `shl`/`ashr` with count ≥ bit-width or
negative is poison. CPython: exact big result / `ValueError("negative shift count")`.
Probe: `print(1 << 64)`, `print(1 << -1)`, `print(-1 >> 100)`.

**B5 — `zap_fmt_flt` buffer.** `char body[96]` with `snprintf("%.{p}{t}")`:
`f"{1e100:f}"` needs ≥ 107 characters and `{x:.60f}` of a large value overflows
the buffer's capacity; `snprintf` truncates, so the output is silently wrong, not
unsafe. Likewise `zap_fmt_int` uses `char body[72]` (check width/`b` format of
`int64` min with `#`).

**B6 — `len(str)` is bytes.** `lower.rs:10901` selects `zap_str_len` (byte length
field) for `Ty::Ptr`; `zap_str_len_cp` exists. SPEC §9 flags this as pending. The
inconsistency with codepoint indexing is user-visible (`s[len(s)-1]`). STR-2 is
ASCII-only so no test notices.

**B7 — no `AssertionError`.** No occurrence in `crates/`, `tests/`, `SPEC.md`. The
exception list in `sema/lib.rs` omits it. `assert_fail` golden exercises the
failure path but the *type* is unspecified.

**B8 — spec/implementation mismatch on `else`.** SPEC §6 (`pass` paragraph) says
the no-op is accepted in "`for`/`while` (and their `else`)" and
"`try`/`except`/`finally`"; GRAMMAR.ebnf `for_stmt`, `while_stmt`, `try_stmt` have
no `else`, and `parse_for`/`parse_while`/`parse_try` don't parse one. Python
programs using `for/else` or `try/else` fail to parse; the spec sentence implies
they exist.

**B9 — `float(s)` failure mode.** SPEC §9: "`float(s)` … faults on a malformed
string" vs `int(s)` ⇒ catchable `ValueError` (#456). Confirm it is a catchable
`ValueError` and add the C case (CONV-3).

Also worth a probe, not yet a bug: `INT64_MIN // -1` and `% -1` (x86 `idiv`
traps); deep recursion outcome; `d[k]` mutation during iteration; `1.0/0.0`.

---

## 11. Recommended next steps (for the maintainer to prioritize)

Nothing below is started. Ordered by value/cost:

1. **Repro B1, B3, B4, B5, B7** as failing tests (one PR each or one "float
   rendering" PR for B1/B5), per the bug-first rule.
2. **Extend the differential generator** (floats, strings, lists, order-independent
   dict/set observations, precedence tables, augmented assignment) — no new oracle
   work required.
3. **CLAIMS.md hygiene PR**: add the ~12 missing rows (§3.2), flip rows already
   exercised by goldens, so the 61/43 number is honest.
4. **Decision batch** for the FEATURE-GAPs in §1 item 4 / §9 (record in
   `PYTHON_DIFFERENCES.md` even if the answer is "no"): `try/for/while … else`,
   `except (A, B)`, the missing builtins, container/str methods, `str * int`,
   `%`/`.format`, `frozenset`, `bytearray`, `AssertionError`.
5. **Pin the unpinned divergences** (§8, "No" rows).
6. **Negative corpus** (§3.3).
7. Then breadth: numeric (§4.2), exceptions (§7.2), unpacking/args (§6, §7.1),
   containers (§5.2), `match` (§7.4).
