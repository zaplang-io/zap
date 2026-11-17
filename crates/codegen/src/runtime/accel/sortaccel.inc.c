/* zapsortaccel.inc.c — SIMD sort accelerator dispatch (#933, D-0195, epic #139).
 *
 * The runtime seam between the numeric-array sort path and the vendored SIMD
 * kernels (x86: x86-simd-sort `zapxsort.cpp`; aarch64: Highway vqsort
 * `zaphwysort.cpp`). Resolves the widest tier the host supports via the shared
 * `cpu_features` detector (#943) — exactly like the memchr pilot (#945) — with
 * the `ZAP_ACCEL` knob (`native` forces the own floor) reusing `zap_accel_tier`.
 * The per-ISA objects are linked (and `ZAP_HAVE_SIMDSORT` defined) only for a
 * program that reaches the sort path on a supported arch (BackendUse::simdsort,
 * like the BLAS/FFT seams); otherwise every entry here returns 0 and the caller
 * uses the own floor (#932).
 *
 * Only the UNSTABLE kind, contiguous arrays (no view strides), the last axis (or
 * a flattened sort), the 6 common dtypes (f64/f32/i64/i32/u64/u32), and lengths
 * at/above a threshold are accelerated; everything else returns 0. A sort is a
 * permutation, so accelerated VALUES equal the own floor (NaNs to the end on both
 * paths) — the sort goldens validate whichever kernel the CI host runs.
 *
 * Included by zaprt.c AFTER zapcpu.inc.c (so `zap_cpu_has` is in scope). The sort
 * entry points (defined far earlier) call the two array-level functions below via
 * forward declarations. */

#include "zap_cpu_features.h"

/* ZAP_ACCEL tier (0 auto / 1 native / 2 fast), from the BLAS seam. */
int64_t zap_accel_tier(void);

/* below this many elements, dispatch + the kernel's own scalar floor aren't worth
 * it over the own introsort (tunable; perf follow-up). */
#define ZAP_SORTACCEL_MIN 256

enum {
    ZAP_SA_F64 = 0,
    ZAP_SA_I64 = 1,
    ZAP_SA_F32 = 4,
    ZAP_SA_I32 = 5,
    ZAP_SA_U32 = 9,
    ZAP_SA_U64 = 10
};
static int zap_sortaccel_dtype_ok(int64_t dt) {
    switch (dt) {
    case ZAP_SA_F64:
    case ZAP_SA_F32:
    case ZAP_SA_I64:
    case ZAP_SA_I32:
    case ZAP_SA_U64:
    case ZAP_SA_U32:
        return 1;
    default:
        return 0; /* 8/16-bit, bool, complex -> own floor */
    }
}

#if defined(ZAP_HAVE_SIMDSORT)
#if defined(__x86_64__) || defined(_M_X64)
extern void zap_xsort_avx2_sort(void *, int64_t, int64_t);
extern void zap_xsort_avx2_argsort(void *, int64_t, int64_t, int64_t *);
extern void zap_xsort_avx512_sort(void *, int64_t, int64_t);
extern void zap_xsort_avx512_argsort(void *, int64_t, int64_t, int64_t *);
#elif defined(__aarch64__) || defined(_M_ARM64)
extern void zap_hwysort_neon_sort(void *, int64_t, int64_t);
extern void zap_hwysort_neon_argsort(void *, int64_t, int64_t, int64_t *);
#endif

/* Resolve the tier once: 0 = none (use own floor), else an arch-specific id. */
enum { ZAP_SA_TIER_NONE = 0, ZAP_SA_TIER_A = 1, ZAP_SA_TIER_B = 2 };
static int zap_sortaccel_resolve(void) {
    if (zap_accel_tier() == 1) return ZAP_SA_TIER_NONE; /* ZAP_ACCEL=native */
#if defined(__x86_64__) || defined(_M_X64)
    if (zap_cpu_has(ZAP_CPU_X86_AVX512F)) return ZAP_SA_TIER_B; /* AVX-512 */
    if (zap_cpu_has(ZAP_CPU_X86_AVX2_FMA)) return ZAP_SA_TIER_A; /* AVX2 */
    return ZAP_SA_TIER_NONE;
#elif defined(__aarch64__) || defined(_M_ARM64)
    return ZAP_SA_TIER_A; /* NEON is the aarch64 baseline */
#else
    return ZAP_SA_TIER_NONE;
#endif
}
static int zap_sortaccel_tier_cache = -1;
static int zap_sortaccel_tier(void) {
    int t = zap_sortaccel_tier_cache;
    if (t < 0) {
        t = zap_sortaccel_resolve();
        zap_sortaccel_tier_cache = t;
    }
    return t;
}
/* Sort one contiguous run of `n` elements in place via the resolved kernel. */
static void zap_sortaccel_run(void *base, int64_t n, int64_t dt, int tier) {
#if defined(__x86_64__) || defined(_M_X64)
    if (tier == ZAP_SA_TIER_B) zap_xsort_avx512_sort(base, n, dt);
    else zap_xsort_avx2_sort(base, n, dt);
#elif defined(__aarch64__) || defined(_M_ARM64)
    (void)tier;
    zap_hwysort_neon_sort(base, n, dt);
#else
    (void)base; (void)n; (void)dt; (void)tier;
#endif
}
/* argsort one contiguous run; writes `n` i64 indices into `out`. */
static void zap_sortaccel_argrun(void *base, int64_t n, int64_t dt, int64_t *out, int tier) {
#if defined(__x86_64__) || defined(_M_X64)
    if (tier == ZAP_SA_TIER_B) zap_xsort_avx512_argsort(base, n, dt, out);
    else zap_xsort_avx2_argsort(base, n, dt, out);
#elif defined(__aarch64__) || defined(_M_ARM64)
    (void)tier;
    zap_hwysort_neon_argsort(base, n, dt, out);
#else
    (void)base; (void)n; (void)dt; (void)out; (void)tier;
#endif
}
#endif /* ZAP_HAVE_SIMDSORT */

/* Whether `a` (unstable kind) is accelerable: a supported dtype, contiguous
 * storage, and a last-axis / flattened sort (so lanes are contiguous runs). */
static int zap_sortaccel_eligible(ZapArray *a, int64_t axis, int flatten, int64_t stable) {
#if defined(ZAP_HAVE_SIMDSORT)
    if (stable) return 0;
    if (!zap_sortaccel_dtype_ok(a->dtype)) return 0;
    if (a->strides || a->off != 0) return 0; /* a view: own floor */
    if (zap_sortaccel_tier() == ZAP_SA_TIER_NONE) return 0;
    int64_t L = flatten ? zap_array_count_of(a) : a->dims[axis];
    if (L < ZAP_SORTACCEL_MIN) return 0;
    if (!flatten && axis != a->ndim - 1) return 0; /* non-last axis: strided lanes */
    return 1;
#else
    (void)a; (void)axis; (void)flatten; (void)stable;
    return 0;
#endif
}

/* Value-sort `a` in place with the accelerator if eligible; returns 1 if it did
 * (caller is done), 0 to fall back to the own floor. `flatten` sorts the whole
 * contiguous buffer; otherwise each last-axis run of length dims[last]. */
int zap_sortaccel_sort_array(void *ap, int64_t axis, int64_t flatten, int64_t stable) {
#if defined(ZAP_HAVE_SIMDSORT)
    ZapArray *a = (ZapArray *)ap;
    if (!zap_sortaccel_eligible(a, axis, (int)flatten, stable)) return 0;
    int tier = zap_sortaccel_tier();
    int64_t count = zap_array_count_of(a);
    size_t esz = zap_array_elem_size(a->dtype);
    char *data = (char *)a->data;
    if (flatten) {
        zap_sortaccel_run(data, count, a->dtype, tier);
        return 1;
    }
    int64_t L = a->dims[axis];
    for (int64_t off = 0; off < count; off += L)
        zap_sortaccel_run(data + (size_t)off * esz, L, a->dtype, tier);
    return 1;
#else
    (void)ap; (void)axis; (void)flatten; (void)stable;
    return 0;
#endif
}

/* argsort `a` into the i64 array `out` with the accelerator if eligible; returns
 * 1 if it did. Indices are per-run (0..L-1), matching NumPy's along-axis argsort. */
int zap_sortaccel_argsort_array(void *ap, int64_t axis, int64_t flatten, int64_t stable, void *outp) {
#if defined(ZAP_HAVE_SIMDSORT)
    ZapArray *a = (ZapArray *)ap;
    if (!zap_sortaccel_eligible(a, axis, (int)flatten, stable)) return 0;
    int tier = zap_sortaccel_tier();
    ZapArray *out = (ZapArray *)outp;
    int64_t *od = (int64_t *)out->data;
    int64_t count = zap_array_count_of(a);
    size_t esz = zap_array_elem_size(a->dtype);
    char *data = (char *)a->data;
    if (flatten) {
        zap_sortaccel_argrun(data, count, a->dtype, od, tier);
        return 1;
    }
    int64_t L = a->dims[axis];
    for (int64_t off = 0; off < count; off += L)
        zap_sortaccel_argrun(data + (size_t)off * esz, L, a->dtype, od + off, tier);
    return 1;
#else
    (void)ap; (void)axis; (void)flatten; (void)stable; (void)outp;
    return 0;
#endif
}
