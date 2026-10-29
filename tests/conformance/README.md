# Spec-conformance suite (I#376, epic #374)

This suite turns the **normative claims of [`SPEC.md`](../../SPEC.md)** into
executable goldens. Where `SPEC.md` and the implementation disagree, `SPEC.md`
wins (its own preamble says so); a conformance case pins what `SPEC.md` says, so
a case that fails is either a real regression or a found divergence to file —
never a reason to quietly edit the case to match the compiler.

## How it relates to the other two suites

| Suite | Oracle | Pins |
|---|---|---|
| `tests/golden/` | author-written `.expected` | the feature's happy path |
| `tests/diff/` (#375) | **CPython** (run both, diff stdout/exit) | Zap↔Python *parity* over the shared subset |
| `tests/conformance/` (this) | **SPEC.md** + author-written `.expected` | Zap's OWN normative behavior, **including deliberate divergences from Python** |

The differential harness can only assert parity, so it cannot pin the places
Zap is *meant* to differ from Python (strict-`bool` `and`/`or`, per-call default
args, wrapping int64): those show up there as `# diff: diverge`. Conformance
pins them as the specified, intended behavior — the thing most at risk of a
silent regression.

## A case

Each case is a `name.zap` program plus sidecars, exactly like a golden:

* `name.zap` — the program. Its **leading comment** must carry a
  `# claim: ID[, ID...]` directive citing the `SPEC.md` claim id(s) it pins
  (catalogued in [`CLAIMS.md`](CLAIMS.md)), followed by prose naming the
  `SPEC.md §section` and the decision/ticket, so a reader sees *why* the output
  is what it is.
* `name.expected` — exact expected stdout (required).
* `name.exit` — expected process exit code (optional, default `0`).
* `name.stderr` — a substring that must appear in stderr (optional).

```zap
# claim: NUM-3
# SPEC §5 (#381, D-0024): `%` is floored modulo — its result takes the sign of
# the DIVISOR (Python), consistent with floored `//`.
def main():
    print(-7 % 2)
```
```
# name.expected
1
```

### Rules for a good case

* **Cite the claim.** Every fixture needs a `# claim:` id that exists in
  `CLAIMS.md` as `covered` and points back at this case — the harness enforces
  the round trip in both directions.
* **Render bool/float/char through `str()`.** Zap's bare `print` float/bool
  format is a separate concern from `str()` (see `tests/golden/conversions.zap`);
  `str(...)` is the deterministic, specified rendering. Bare `print` of an
  int/string is fine.
* **Stay order-independent for unordered types.** Built-in `dict`/`set`
  iteration order is unspecified (D-0056), so observe counts, sums or
  membership, never a sequence.
* **Reduce to small observables.** Prefer scalars/short strings over container
  repr formatting.
* **`zap fmt --check` clean.** The `host` CI job formats this directory; keep the
  88-column `MAX_WIDTH` rule (split long call lines into intermediate vars). Fix
  locally with `cargo run --bin zap -- fmt tests/conformance`.

## How it runs

[`crates/driver/tests/conformance.rs`](../../crates/driver/tests/conformance.rs):

* **Host (no LLVM), always:** the index/backbone checks — fixtures are paired,
  `CLAIMS.md` parses with no duplicate ids, every `covered` claim names a real
  case, every case's `# claim:` ids are `covered` and point back at it. A broken
  claim index or an uncited case fails `cargo test` on any host.
* **Codegen (LLVM), in CI's `full` + `arm64` jobs:** `conformance_programs_match`
  compiles + runs each program and checks stdout/exit/stderr — riding the
  existing `cargo test --workspace --features zapc/gc-*` steps (no new CI job),
  the same way goldens and the diff harness do.

## Scope of the first cut

The initial cases cover the semantic **danger zones** the #375 differential
tester flagged, and their neighbors: truthiness, boolean short-circuit + value,
numeric `/`/`//`/`%`/overflow, `str()` rendering, chained comparison, dict
`get`→`Optional` and iteration, built-in-fault catchability, tuples, function
arguments (defaults/keyword/variadic), comprehensions, control flow.

`CLAIMS.md` additionally lists dozens of `TODO` claims across the rest of
`SPEC.md`, so coverage is measurable and grows by adding a case and flipping a
row. Broadening coverage toward the whole spec is incremental follow-up (see the
epic #374 note). A few `TODO`s are flagged **(impl gap …)** — `SPEC.md`
specifies the behavior but the compiler does not match it yet (list `in`, list
slicing, list negative indexing, divide-by-zero catchability); they stay `TODO`
until the gap closes rather than being pinned as passing.
