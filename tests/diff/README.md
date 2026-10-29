# CPython differential-testing corpus (I#375, epic #374)

Zap aims to be "Python-readable": over the subset the two languages share, Zap
intends CPython's exact runtime semantics. CPython is therefore a free,
authoritative **oracle**. The harness in
[`crates/driver/tests/diff_cpython.rs`](../../crates/driver/tests/diff_cpython.rs)
runs the same program under `zapc run` and `python3` and diffs **stdout + exit
status**. Any difference is either a bug we want to catch the day it lands, or a
*deliberate* divergence we pin down and document here instead of hiding.

## How a case works

Each `*.zap` file in this directory is a real Zap program — the thing we ship
and test. The CPython side is **derived** from it by a small, auditable
source transform (`zap_to_python`) that bridges the only three places the shared
subset differs in surface syntax:

* **declarations** — `auto x = …`, `int64 x = …`, `list[int64] xs = …`,
  `const f64 PI = …` become plain Python assignments;
* **function signatures** — `def f(int64 a, str b = "x") -> int64:` becomes
  `def f(a, b="x"):`, a variadic `int64... xs` becomes `*xs`, the `-> ret` is
  dropped;
* **entry point** — Zap auto-runs `main`; CPython gets a trailing `main()` call.

Everything else (control flow, `print`, comprehensions, lambdas, operators,
f-strings, `try`/`except`/`finally`) is already valid Python and is emitted
verbatim. So **adding a case = dropping in one `.zap` file**; no second file to
keep in sync.

## The directive

The program's leading comment declares its expected cross-language outcome:

```
# diff: match
```
Zap and CPython must produce identical stdout and exit status. (This is the
default if no directive is present.)

```
# diff: diverge <reason>
```
Zap and CPython are **expected** to differ; `<reason>` documents why — either a
locked decision (`D-00xx`) or a filed bug (`BUG`, with the issue). The harness
asserts these cases *do* differ, and fails loudly if one ever stops differing,
so a fixed gap gets promoted to `# diff: match` instead of silently rotting.

## Corpus conventions

* Programs wrap their work in `def main():` (matching the golden suite). Do not
  add your own top-level `main()` call — Zap would run `main` twice.
* Reduce observations to scalars/strings/bools where possible; don't depend on
  container repr formatting.
* Render floats and bools through `str(...)` — Zap's bare `print` float/bool
  format differs from CPython's repr *by design* (see
  `tests/golden/conversions.zap`); `str(...)` is the Python-faithful rendering.
* Keep every case valid in **both** languages. Zap-only constructs
  (`match`/`case`, `.contains()`, `raise "string"`, `try … else`) are not part
  of the shared subset and belong in the golden suite, not here.

## Documented divergences (as of the PR that introduced this harness)

Intentional (locked decisions):

| case | reference |
|---|---|
| `default_arg_once` | D-0038 — defaults evaluated per call, not once at def (fixes Python's mutable-default footgun) |
| `and_or_value` | D-0022 — `and`/`or` yield a strict `bool`, not the surviving operand |
| `dict_order` | D-0056 — built-in `dict` is hash-ordered; insertion order is the opt-in `collections` ordered map |
| `int_overflow` | D-0005 / D-0019 — default `int` is a wrapping int64; `bigint` is the arbitrary-precision opt-in |

Bugs found on `main` (for triage; NOT fixed in this PR):

| case | finding |
|---|---|
| `mod_negatives` | `%` uses C-truncated sign on negatives while `//` floors — internally inconsistent (D-0024 says Python-style) |
| `collection_truthiness` | `if <list>:` / `if <dict>:` crashes codegen (panic, exit 101) instead of applying truthiness |
| `except_builtin_fault` | a built-in fault (index-out-of-range) is not caught by `except` (contradicts D-0007) |
| `float_str_integral` | `str()` of an integral float drops the `.0` (`str(6 / 2)` is `"3"`, not `"3.0"`) |
| `string_truthiness` | `""` is truthy (Zap treats every string as truthy); CPython makes empty strings falsy |
| `chained_comparison` | `a < b < c` parses left-associative `(a < b) < c`, not Python-chained |

Feature gaps vs CPython (for triage):

| case | finding |
|---|---|
| `list_membership` | `x in <list>` unsupported ("'in' is only supported with a set on the right") |
| `list_slice` | list/array slicing unsupported ("only ... str and bytes ... a follow-up") |
| `list_negative_index` | `xs[-1]` faults instead of indexing from the end |

### Not expressible as a clean runtime diff

* **`try … else`** — Zap's grammar has no `else` clause on `try`
  (`docs/GRAMMAR.ebnf`), so a program using it is a *parse error*, not a stdout
  difference. A structural, compile-time divergence.
* **Comprehension variable leakage** — in CPython the loop variable is scoped to
  the comprehension; observing a leak cross-language only yields a
  `NameError`/compile failure, not comparable stdout. Comprehension *value*
  semantics are covered by `comprehensions.zap`.

## The random generator

`random_programs_match_cpython` emits a seeded corpus of fully-parenthesized
`print(...)` programs over the subset where the two engines are *required* to
agree (bounded int arithmetic, boolean logic with boolean operands, single
comparisons, string concat, in-range list/dict indexing, ternaries, and
comprehension counts). It deliberately avoids every divergence above, so a
failure is a genuine, newly-found bug. Deterministic in CI (fixed seed, modest
count); fuzz harder locally:

```bash
ZAP_DIFF_SEED=42 ZAP_DIFF_COUNT=2000 cargo test -p zapc \
    --features zapc/gc-immix --test diff_cpython random_programs_match_cpython
```
