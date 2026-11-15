/* zapxsort.cpp — x86-64 SIMD sort accelerator (#933, D-0195, epic #139).
 *
 * The x86 kernel of the numeric-array sort accelerator: thin extern-C wrappers
 * over the header-only Intel x86-simd-sort (`x86simdsortStatic::qsort`/`argsort`,
 * the library NumPy adopted). Compiled ONCE PER ISA TIER by Zap's build — the
 * same TU with `-DZAP_XSORT_TIER=avx2 -mavx2` and `-DZAP_XSORT_TIER=avx512 -mavx512f
 * -mavx512dq -mavx512vl -mavx512bw` — so each object exports its own
 * `zap_xsort_<tier>_{sort,argsort}`. The runtime (`zapsortaccel.inc.c`) resolves
 * the widest tier the host supports via `cpu_features` (#943) and calls through a
 * function pointer, so a tier is never executed on a CPU that lacks it.
 *
 * Only the UNSTABLE value sort / argsort are accelerated (x86-simd-sort is not a
 * stable sort), over the 6 common dtypes f64/f32/i64/i32/u64/u32; the stable kind
 * and the other dtypes stay on the own floor (#932). `nans_last=true` sorts NaNs
 * to the end, matching the own floor and NumPy. A sort is a permutation, so the
 * sorted VALUES are identical to the own floor — no fp-determinism concern (#946).
 *
 * Built as a standalone native object (like zapfft.cpp), NOT merged into the LTO
 * bitcode (C++↔C stays extern-C), linked after the runtime. */

#include "x86simdsort-static-incl.h"

#include <cstddef>
#include <cstdint>
#include <vector>

#define ZAP_XS_CAT2(a, b) a##b
#define ZAP_XS_CAT(a, b) ZAP_XS_CAT2(a, b)
#define ZAP_XS_SORT ZAP_XS_CAT(ZAP_XS_CAT(zap_xsort_, ZAP_XSORT_TIER), _sort)
#define ZAP_XS_ARG ZAP_XS_CAT(ZAP_XS_CAT(zap_xsort_, ZAP_XSORT_TIER), _argsort)

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
extern "C" void ZAP_XS_SORT(void *p, int64_t n, int64_t dt) {
    size_t sz = (size_t)n;
    switch (dt) {
    case ZDT_F64: x86simdsortStatic::qsort((double *)p, sz, true); break;
    case ZDT_F32: x86simdsortStatic::qsort((float *)p, sz, true); break;
    case ZDT_I64: x86simdsortStatic::qsort((int64_t *)p, sz); break;
    case ZDT_I32: x86simdsortStatic::qsort((int32_t *)p, sz); break;
    case ZDT_U64: x86simdsortStatic::qsort((uint64_t *)p, sz); break;
    case ZDT_U32: x86simdsortStatic::qsort((uint32_t *)p, sz); break;
    default: break;
    }
}

/* Unstable argsort of a contiguous buffer; writes `n` i64 indices into `out`. */
extern "C" void ZAP_XS_ARG(void *p, int64_t n, int64_t dt, int64_t *out) {
    size_t sz = (size_t)n;
    std::vector<size_t> a;
    switch (dt) {
    case ZDT_F64: a = x86simdsortStatic::argsort((const double *)p, sz, true); break;
    case ZDT_F32: a = x86simdsortStatic::argsort((const float *)p, sz, true); break;
    case ZDT_I64: a = x86simdsortStatic::argsort((const int64_t *)p, sz); break;
    case ZDT_I32: a = x86simdsortStatic::argsort((const int32_t *)p, sz); break;
    case ZDT_U64: a = x86simdsortStatic::argsort((const uint64_t *)p, sz); break;
    case ZDT_U32: a = x86simdsortStatic::argsort((const uint32_t *)p, sz); break;
    default: return;
    }
    for (size_t i = 0; i < sz; i++) out[i] = (int64_t)a[i];
}
