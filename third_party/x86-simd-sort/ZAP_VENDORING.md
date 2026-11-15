# Vendored: Intel x86-simd-sort

Used by the numeric-array **SIMD sort accelerator** (#933, D-0195, epic #139) as
the x86-64 kernel: the AVX2 / AVX-512 `_mvsimd_` sort/argsort variants call
`x86simdsortStatic::qsort` / `argsort` (header-only), dispatched by Zap's
`cpu_features` detector (#943). This is the library NumPy adopted for `np.sort`.

- **Upstream**: https://github.com/intel/x86-simd-sort
- **Commit**: `13d89f8` (default branch), retrieved 2026-11-15
- **License**: BSD 3-Clause (`LICENSE.md`)

## What was kept

- `src/` — the header-only templated sort (the `x86simdsortStatic::*` static
  API via `x86simdsort-static-incl.h`; per-ISA networks in `avx2-*` / `avx512-*`).
- `lib/`, `LICENSE.md`, `README.md`.

## What was pruned

`tests/`, `benchmarks/`, `examples/`, `scripts/`, `utils/`, `misc/`, the meson /
Make build files, and the CI/meta markdown — none are needed for the header-only
static use. The sort is compiled per-ISA by Zap's own build (see
`crates/codegen/src/zapxsort_*.cpp` + `blas`-style gating in `lib.rs`), not by the
upstream build system.
