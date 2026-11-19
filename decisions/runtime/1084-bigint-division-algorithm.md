---
title: Native bigint division uses word-wise long division (Knuth Algorithm D)
status: Locked
issue: 1084
pr:
date: 2026-11-19
domain: runtime
supersedes:
superseded-by:
---

# Bigint division is a word-wise long division, not a bit-by-bit scan

**Status:** Locked

## Decision

The raw unsigned bigint division kernel `zap_big_raw_divmod` (base-2^32
little-endian limbs, in the runtime C) computes `q = a / b`, `r = a % b` by
**word-wise long division**:

- a **single-limb fast path** — schoolbook division of the dividend by a scalar
  when the divisor is one limb; and
- **Knuth Algorithm D** (TAOCP vol. 2, §4.3.1) for multi-limb divisors —
  normalize so the divisor's top limb has its high bit set, estimate each
  quotient limb from the top two numerator limbs over the top divisor limb (with
  the two-limb correction), multiply-and-subtract, add the divisor back on the
  rare over-estimate, then de-normalize the remainder.

Trivial cases (`a < b` → quotient 0, remainder `a`; `a == 0`) are handled up
front. The signed layer `zap_big_divmod_signed` is **unchanged**: Python floor
semantics (quotient rounds toward −∞), remainder takes the sign of the divisor,
and divide-by-zero still raises a catchable `ZeroDivisionError` via the existing
`zap_panic_typed(ZAP_EXC_ZERO_DIVISION_ERROR, …)` throwing path. The immutable
base-2^32 limb representation and all callers (`//`, `%`) are unchanged.

## Rationale

The previous kernel performed **bit-by-bit** long division: it walked all
`numerator_bits` bits of the dividend and shifted every active remainder limb on
each step, so its cost scaled with `numerator_bits × remainder_limbs` — even
when the quotient fits in a single limb. A near-equal-length division with a
small quotient (exactly the CLBG pidigits hot path) therefore paid a full
bit-length scan: 100 isolated 16,000-bit divisions cost ~0.447 s, growing
super-linearly, and the canonical 10,000-digit pidigits exceeded 30 minutes
(1,786 s user CPU) when every non-Zap configuration passed (#1084,
zap-benchmarks#12).

Algorithm D makes a small quotient cost ~O(n) limb operations instead of
O(bits): each quotient limb is produced in one multiply-subtract pass over the
divisor limbs. It is the standard, well-analyzed choice (the same algorithm GMP,
Go `math/big`, and libgmp-style libraries use for the schoolbook range), its
quotient-digit estimate is provably off by at most one after the two-limb
correction, and the add-back step makes it exact.

Options considered:

- **Keep bit-by-bit, special-case one-limb quotients.** Rejected: the
  super-linear scan returns for any quotient wider than a limb, and the hot path
  is precisely near-equal-length operands where the quotient is tiny but the
  operands are huge — a scan over all numerator bits is unavoidable in that
  shape.
- **Burnikel–Ziegler / divide-and-conquer division.** Rejected for now: it only
  wins far above the pidigits operand sizes, needs a fast multiplication base to
  pay off, and is materially more code to get correct. Algorithm D removes the
  pathology and is the right first step; a sub-quadratic divider can layer on
  later behind the same `zap_big_raw_divmod` contract if a future workload needs
  it.
- **Change the limb representation (e.g. 64-bit limbs).** Rejected: out of scope
  and ABI-visible. This is a hot-path rewrite of the raw unsigned kernel only.

## Consequences

- `//` and `%` on bigints are exact and fast for all operand sizes; the
  near-equal-length / small-quotient hot path is no longer super-linear
  (a C-level microbench of the kernel shows 100 divisions scaling roughly
  linearly in operand size, vs. the old super-linear curve). The end-to-end
  10,000-digit pidigits number must be confirmed on the perf box / CI (it cannot
  be built on a host without the LLVM-18 toolchain).
- No change to signed floor/modulo semantics, divisor-signed remainders, the
  catchable divide-by-zero behavior, the limb representation, or any caller.
- No new runtime fragment, no new `#include`, and no Rust-side ir/codegen change:
  the rewrite is confined to the body of `zap_big_raw_divmod` in the runtime C.
- Correctness is pinned by `tests/golden/bigint_division.zap` (exactness across
  sizes, the Algorithm-D add-back branch, all four sign combinations with the
  floor/modulo identity, limb-boundary and mid-limb powers of two, and catchable
  divide-by-zero) against Python-computed goldens, and by a C-level differential
  fuzz of the kernel against the old bit-by-bit reference and the `q·b + r == a`
  identity (millions of cases, ASan/UBSan clean).
