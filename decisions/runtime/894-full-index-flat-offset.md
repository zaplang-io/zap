---
title: Full-index flat-offset fast path — single scalar get/set for m[i, j] and m[i][j], no row-view allocation
status: Locked
issue: 894
pr:
date: 2026-11-20
domain: runtime
supersedes:
superseded-by:
---

# Full-index flat-offset fast path — single scalar get/set for `m[i, j]` and `m[i][j]`

**Status:** Locked

## Decision

A FULL integer index into a dense array — one index per axis, `m[i0, …,
i_{n-1}]`, whether read, assigned (`= v`), or augmented (`+= v`) — lowers to a
single scalar `zap_array_get/set_<dtype>` over the whole-shape row-major flat
offset computed **inline** (Horner over the axis dims), with **no** intermediate
`zap_array_row` view allocated. This covers **both halves of #894**: the comma
form `m[i, j]` (part 1) and the chained form `m[i][j]` (part 2).

Concretely, for rank `n ≥ 2`:

- each axis index is validated against *that axis's* length by the new runtime
  `zap_array_ckidx(a, axis, i)` (returns the validated index; out-of-range
  aborts, exactly as the former per-axis `zap_array_row` / `zap_array_index_axis`
  fold did);
- the lowering folds the validated indices with Horner's rule
  `flat = (..(i0·d1 + i1)·d2 + i2)..` (the trailing-axis dims read via
  `zap_array_dim`);
- the typed scalar get/set now address storage through the general
  `zap_array_phys_nd(a, flat)` (previously rank-1-only `zap_array_phys`), so the
  flat offset composes with strided/transposed views (#502).

The **chained** form `m[i0][i1]…[i_{n-1}]` (part 2) fuses to the identical flat
access at lowering, but only when every link is a *pure scalar index* (a name or
integer literal) and the chain indexes all `n` axes of an array root. Each
`m[i0]…[i_k]` prefix would otherwise materialize a rank-reduced row view only to
index it again; under that guard those intermediates are dead, single-use
temporaries, so the whole chain becomes one flat `get`.

Rank 1 (`a[i]`, and the degenerate `a[i,]`) is unchanged — it was already a
direct get/set with no fold. Partial indexing (`m[i]`, fewer indices than axes),
a *bound* row view (`row = m[i]`), and a non-pure or fancy chain
(`m[i + 1][j]`, `m[idx][j]`) are unchanged — each still returns / materializes an
allocated rank-reduced view, because it is (or feeds) a first-class bindable
value whose allocation cannot be elided by the syntactic guard.

No surface-syntax or semantic change: `m[i, j]` and `m[i][j]` keep their current
results and their current out-of-range behavior.

## Rationale

- **Kill the per-access allocation, which is the whole point.** The #500 N-D
  lowering folded a full index through one `zap_array_row` per leading axis, each
  allocating a view header **and** a copied `dims` buffer. In a hot element-access
  loop that is one GC allocation per access — precisely the overhead a dense,
  NumPy-style array exists to avoid. The flat Horner offset needs no heap object.
- **Reuse the existing N-D address primitive, add no new per-dtype symbols.**
  `zap_array_phys_nd` already maps a whole-shape flat index to a physical offset
  honoring strides (it backs #510 in-place advanced assignment). Pointing the
  existing typed get/set at it — rather than adding a parallel `get_flat_<dtype>`
  for all ~11 dtypes + bool + complex — keeps the change small and inherits the
  proven stride handling. `phys_nd` gained fast paths for the contiguous
  (`off + flat`) and rank-1-strided (`off + flat·strides[0]`, a single multiply,
  no modulo) cases, so **no existing access regresses**: it is now a strict,
  equal-cost superset of the rank-1-only `zap_array_phys` for every shape a scalar
  get/set lands on. `zap_array_phys` is retained for the accel RNG shuffle
  (`runtime/accel/random.inc.c`), its one remaining caller.
- **Preserve per-axis bounds, do not loosen to a total-count check.** The issue
  offered "bounds-check against the total element count (or per-axis)". A
  total-count-only check would let `m[0, 5]` on a `(3, 3)` array pass
  (`flat = 5 < 9`) and silently read element `(1, 2)` — a correctness regression,
  not merely a different error message, since today that access aborts. We
  therefore keep the per-axis check (`zap_array_ckidx`) so the observable result
  of every program, valid or out-of-range, is identical to before. This needed no
  design decision from the maintainer because it is the behavior-preserving
  choice.
- **No new ZIR instruction / no codegen-gotcha surface.** The fast path emits only
  existing `Inst::Call` (to `zap_array_ckidx` / `zap_array_dim` / the existing
  typed get/set) and `Inst::Bin` (Mul/Add). No new `Inst`/`Elem`/`Expr` variant,
  no new `zir::` type, no new runtime `.inc.c` fragment, and the scalar results
  are integers/doubles (pointer-returning `get_complex` is rooted by its existing
  declared signature), so none of the `inst_dest` / exhaustive-match / re-export /
  two-writer hazards apply.
- **Part 2 as a syntactic lowering fusion, not an escape-analysis rewrite.** The
  issue framed `m[i][j]` fusion as "prove the intermediate row view dead via the
  #839/#850 escape pass". For the *chained* form that proof is already structural:
  an unbound `m[i]` sub-expression has exactly one consumer (the next index), so
  it cannot escape or alias — no dataflow pass needed. Fusing at lowering (a small
  peephole over the `Expr::Index` chain) reuses the part-1 flat-offset machinery
  verbatim, is fully host-testable via `zap ir` (no LLVM build), and carries none
  of the GC-rooting risk of extending the escape pass to recognize and rewrite
  array row views — risk that only the LLVM / GC-stress CI legs would surface. The
  guard is deliberately conservative (pure name / integer-literal links, bare-name
  array root, all axes): it fires for the hot `m[i][j]` loop/index form and bails
  safely to the unchanged per-step lowering for everything else. Because the
  probed links are side-effect-free, any IR emitted before a bail is dead and
  re-executable, so the fall-through path stays correct. Fusing a *bound* dead row
  view (`row = m[i]; … row[j]`) is the genuinely escape-pass-shaped case and is
  left as a follow-up (noted in design/0002 Deferred).

## Consequences

- A full-index element-access loop over an N-D array — written either `m[i, j]`
  or `m[i][j]` — allocates **zero** GC objects per access and emits no
  `zap_array_row`; it is a handful of non-allocating leaf calls plus integer
  arithmetic (inlinable under LTO). IR regressions
  (`full_index_takes_the_flat_fast_path`, `_rank3_`, `_store_`,
  `chained_full_index_fuses_like_comma_index`, `chained_full_index_rank3_fuses`,
  and the negative `chained_partial_index_still_views` / `bound_row_view_is_not_fused`
  / `impure_chain_index_is_not_fused` / `fancy_index_chain_is_not_fused`) assert the
  `zap_array_row` count is 0 for both full-index forms and that the per-step view is
  retained exactly where fusion must not fire, so the optimization cannot silently
  regress in either direction.
- `zap_array_phys_nd` is now on the scalar get/set hot path for every rank. Its
  contiguous and rank-1-strided fast paths keep that identical in cost to the old
  `zap_array_phys`; only a rank-≥2 *strided* view in a full index takes the
  general decode loop (still allocation-free, and strictly better than the old
  allocating fold).
- Still allocating (unchanged, by design): partial-index views (`m[i]`), slice
  views (#502), advanced indexing (#510), a *bound* row view (`row = m[i]`), and a
  non-pure or fancy chain (`m[i + 1][j]`, `m[idx][j]`) — each produces or feeds a
  first-class value the syntactic guard cannot prove dead.
- Fusing a *bound* dead row view remains a follow-up (the genuinely
  escape-pass-shaped case); both the comma and the pure chained full-index forms
  are handled here, completing #894's stated scope.

<!-- Follow-up to the #500 N-D indexing lowering (frozen DECISIONS.md D-0164) and
#502 strided views (D-0171). Sibling perf follow-up: #896 broadcast kernel
(D-0178). Escape pass: #839 / #850. -->
