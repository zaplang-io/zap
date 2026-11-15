# Vendored: Google Highway (headers only, for vqsort)

Used by the numeric-array **SIMD sort accelerator** (#933, D-0195, epic #139) as
the aarch64 kernel: a NEON (and, as a follow-up, SVE) `_mvsimd_` sort/argsort
variant calls Highway's **static, header-only** `hwy::HWY_NAMESPACE::VQSortStatic`
/ `VQArgSortStatic` (`VQSORT_ONLY_STATIC`), compiled for the target the TU is
built with. Highway self-selects within the arch; Zap's `cpu_features` detector
(#943) gates it at the arch/feature level.

- **Upstream**: https://github.com/google/highway
- **Commit**: `0e0a01d` (default branch), retrieved 2026-11-15
- **License**: dual Apache-2.0 / BSD-3-Clause (`LICENSE`); used under BSD-3-Clause.

## What was kept

- `hwy/*.h` (core), `hwy/ops/*.h` (per-ISA op headers), and the sort headers
  `hwy/contrib/sort/{vqsort,vqargsort}-inl.h` + their `*-inl.h` / `order.h`
  dependencies, plus the one `hwy/contrib/algo/*-inl.h` they include.
- `LICENSE`, `README.md`.

## What was pruned

All `.cc` (113 files — the dynamic-dispatch `foreach_target` library; NOT needed
for the static header-only `VQSortStatic` path), all `*_test.{cc,h}`, the other
`hwy/contrib/*` modules, `hwy/io`, `hwy/tests`, `hwy/contrib/testdata/` (~22 MB of
corpora), and the Bazel/CMake/g3doc/CI files. The sort is compiled per-target by
Zap's own build (`crates/codegen/src/zaphwysort_*.cpp`), not Highway's.
