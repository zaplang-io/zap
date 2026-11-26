---
title: str.center biases odd padding to the left, matching CPython
status: Accepted
issue: 1656
pr:
date: 2026-11-26
domain: stdlib
supersedes: DECISIONS.md#D-0210 (center odd-padding split only)
superseded-by:
---

# str.center biases odd padding to the left, matching CPython

**Status:** Accepted

## Decision

`str.center(width[, fill])` splits odd padding the CPython way: the extra fill
goes on the **left** when both the total `width` and the margin
(`width - len(s)`) are odd, and on the right otherwise. Concretely the left count
is `pad / 2 + (pad & width & 1)`. `"hi".center(5)` is now `"  hi "` (was
`" hi  "`). The characters and total length were already correct; only the
left/right balance changes. `ljust`/`rjust` are unaffected.

This supersedes the clause of **D-0210** that split centered padding with the
smaller half on the left (`pad / 2`).

## Rationale

- Zap is "Python-readable"; `str.center` is a Python method and must place the
  centered text where CPython does over the shared subset. The old split put the
  extra fill on the opposite side for every odd-padding case, so centered output
  was off by one position — visible and surprising.
- The one-line `(pad & width & 1)` bias is exactly CPython's `unicode_center`
  rule; adopting it verbatim avoids any new, third behavior to reason about.
- #1648 fixed the multibyte-fill correctness bug and deliberately left this split
  alone (it is not a UTF-8 problem); this decision closes the remaining CPython
  divergence it noted.

## Consequences

- `zap_str_pad`'s center branch adds the bias term; the multibyte-fill repetition
  and the overflow-checked allocation from #1648 are unchanged.
- The odd-width centered cases previously excluded from the CPython differential
  suite are promoted into `tests/diff/str_pad_multibyte.zap`; the golden pins the
  new split on ASCII and multibyte input.
- A changelog `fixed` fragment records the change. Part of the epic #1638 Unicode
  audit follow-up; completes the `center` alignment begun in #1648.

<!-- Related: decisions/stdlib/1648-pad-unicode-fill.md; SPEC §9.2. -->
