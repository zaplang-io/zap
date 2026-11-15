/* zaphwysort.cpp — aarch64 SIMD sort accelerator (#933, D-0195, epic #139).
 *
 * The aarch64 kernel of the numeric-array sort accelerator: thin extern-C
 * wrappers over Google Highway's static, header-only vqsort
 * (`hwy::HWY_NAMESPACE::VQSortStatic` / `VQArgSortStatic`, under
 * `VQSORT_ONLY_STATIC` — no Highway dynamic-dispatch library). Compiled for the
 * aarch64 NEON baseline (always present on ARMv8-A); an SVE tier is a follow-up.
 * Zap's `cpu_features` detector (#943) gates the accelerator on at the arch level
 * and Highway self-selects the NEON path.
 *
 * Only the UNSTABLE value sort / argsort are accelerated, over the same 6 dtypes
 * as the x86 kernel (f64/f32/i64/i32/u64/u32); the stable kind and the other
 * dtypes stay on the own floor (#932). Highway sorts NaNs to the end for
 * ascending order (CountAndReplaceNaN + a trailing Fill of canonical NaN), so the
 * sorted VALUES match the own floor / x86 kernel / NumPy — no fp-determinism
 * concern (a sort is a permutation, #946).
 *
 * Built as a standalone native object on aarch64 only (like zapfft.cpp), linked
 * after the runtime. */

#define VQSORT_ONLY_STATIC 1
#include "hwy/contrib/sort/vqsort-inl.h"
#include "hwy/contrib/sort/vqargsort-inl.h"

#include <cstddef>
#include <cstdint>
#include <cstdlib>

/* dtype codes mirror zaprt.c's DTYPE_* enum. */
enum {
    ZDT_F64 = 0,
    ZDT_I64 = 1,
    ZDT_F32 = 4,
    ZDT_I32 = 5,
    ZDT_U32 = 9,
    ZDT_U64 = 10
};

/* In-place unstable value sort of a contiguous buffer of `n` elements. */
extern "C" void zap_hwysort_neon_sort(void *p, int64_t n, int64_t dt) {
    size_t sz = (size_t)n;
    namespace hn = hwy::HWY_NAMESPACE;
    const hwy::SortAscending asc;
    switch (dt) {
    case ZDT_F64: hn::VQSortStatic((double *)p, sz, asc); break;
    case ZDT_F32: hn::VQSortStatic((float *)p, sz, asc); break;
    case ZDT_I64: hn::VQSortStatic((int64_t *)p, sz, asc); break;
    case ZDT_I32: hn::VQSortStatic((int32_t *)p, sz, asc); break;
    case ZDT_U64: hn::VQSortStatic((uint64_t *)p, sz, asc); break;
    case ZDT_U32: hn::VQSortStatic((uint32_t *)p, sz, asc); break;
    default: break;
    }
}

/* Unstable argsort of a contiguous buffer; writes `n` i64 indices into `out`.
 * Highway packs a 32-bit key with its index into one 64-bit lane (no scratch),
 * but a 64-bit key needs a `uint128_t` scratch buffer (key+index per element). */
extern "C" void zap_hwysort_neon_argsort(void *p, int64_t n, int64_t dt, int64_t *out) {
    size_t sz = (size_t)n;
    namespace hn = hwy::HWY_NAMESPACE;
    const hwy::SortAscending asc;
    uint64_t *idx = (uint64_t *)out; /* same width; indices are non-negative */
    switch (dt) {
    case ZDT_F32: hn::VQArgSortStatic((const float *)p, sz, idx, asc); break;
    case ZDT_I32: hn::VQArgSortStatic((const int32_t *)p, sz, idx, asc); break;
    case ZDT_U32: hn::VQArgSortStatic((const uint32_t *)p, sz, idx, asc); break;
    case ZDT_F64:
    case ZDT_I64:
    case ZDT_U64: {
        hwy::uint128_t *scratch =
            (hwy::uint128_t *)aligned_alloc(64, (sz ? sz : 1) * sizeof(hwy::uint128_t));
        if (dt == ZDT_F64) hn::VQArgSortStatic((const double *)p, sz, idx, scratch, asc);
        else if (dt == ZDT_I64) hn::VQArgSortStatic((const int64_t *)p, sz, idx, scratch, asc);
        else hn::VQArgSortStatic((const uint64_t *)p, sz, idx, scratch, asc);
        free(scratch);
        break;
    }
    default: break;
    }
}
