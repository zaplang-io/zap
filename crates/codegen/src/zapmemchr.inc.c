/* zapmemchr.inc.c — SIMD byte/substring search (ISA-acceleration epic #139,
 * sub-issue #945, D-0194; design snapshot Discussion #920 §4). The FIRST
 * multiversioned kernel: it proves the clone + resolver + `cpu_features` dispatch
 * pattern end to end on a real, high-traffic operation (`str.find` / `in` /
 * `index`, line splitting, tokenizers) before the facility grows.
 *
 * memchr finds a byte; memmem finds a substring (scan for the needle's first
 * byte with the SIMD memchr, verify the rest with memcmp). Only memchr is
 * multiversioned — it is where the lane-parallelism lives — and memmem rides it.
 *
 * DISPATCH (#920 §6a): hand-written pointer-with-once-init in C (NOT the #944
 * codegen cloner, which multiversions *Zap* functions — this is a C-runtime hot
 * path). The resolver reads the shared `cpu_features` detector (#943) and picks
 * the widest clone the host supports; a benign first-call race recomputes the
 * identical pointer. x86-64's floor is SSE2 (AMD64-guaranteed) so the fallback
 * is never scalar there; aarch64's floor is NEON. AVX2 is the runtime-detected
 * upgrade (the one real dispatch decision).
 *
 * LANE REDUCE (#920 §4): x86 exports a comparison bitmask and finds the first
 * match with a count-trailing-zeros; NEON has no movemask, so it detects "any
 * match in this block" with a horizontal max and then locates the lane. Same
 * algorithm, different horizontal primitive — never designed to the x86 spelling.
 *
 * TARGET FEATURES / LTO: the AVX2 clone carries `__attribute__((target("avx2")))`
 * so this baseline-compiled TU may still contain AVX2 codegen. Its (and the other
 * SIMD clones') `_mvsimd_` name marks it for the I#841 `strip_target_attrs_bitcode`
 * EXEMPTION, exactly like a #944 `$mv$` clone — otherwise LTO would strip the
 * `+avx2` attribute and the backend could not legalize the AVX2 instructions the
 * bitcode already carries (the green-debug / broken-LTO failure mode, #845). The
 * clones are reached only through the function pointer, so they are never inlined
 * into a baseline caller regardless.
 *
 * Compiled as part of zaprt.c (after zapcpu.inc.c, so `zap_cpu_has` is in scope),
 * and standalone by the C unit test (crates/codegen/tests/zapmemchr_test.c). */

#include "zap_cpu_features.h"

#include <stddef.h>
#include <stdint.h>
#include <string.h>

/* --- scalar baseline (every arch; the GENERIC fallback) ------------------- */
static ptrdiff_t zap_memchr_mvsimd_scalar(const uint8_t *d, size_t n, uint8_t t) {
    for (size_t i = 0; i < n; i++) {
        if (d[i] == t) return (ptrdiff_t)i;
    }
    return -1;
}

#if defined(__x86_64__) || defined(_M_X64)
#include <emmintrin.h> /* SSE2 */
/* SSE2 is the x86-64 baseline (no detection, no target attribute needed). 16-byte
 * lanes: compare-equal, export a 16-bit mask, ctz to the first match. */
static ptrdiff_t zap_memchr_mvsimd_sse2(const uint8_t *d, size_t n, uint8_t t) {
    size_t i = 0;
    __m128i vt = _mm_set1_epi8((char)t);
    for (; i + 16 <= n; i += 16) {
        __m128i chunk = _mm_loadu_si128((const __m128i *)(d + i));
        unsigned mask = (unsigned)_mm_movemask_epi8(_mm_cmpeq_epi8(chunk, vt));
        if (mask) return (ptrdiff_t)(i + (size_t)__builtin_ctz(mask));
    }
    for (; i < n; i++) {
        if (d[i] == t) return (ptrdiff_t)i;
    }
    return -1;
}

#include <immintrin.h> /* AVX2 */
/* AVX2 upgrade (runtime-detected). 32-byte lanes. The target attribute lets this
 * one function use AVX2 in an otherwise-baseline TU; its `_mvsimd_` name keeps
 * the attribute through the LTO strip (see the file header). */
__attribute__((target("avx2"))) static ptrdiff_t zap_memchr_mvsimd_avx2(const uint8_t *d, size_t n,
                                                                        uint8_t t) {
    size_t i = 0;
    __m256i vt = _mm256_set1_epi8((char)t);
    for (; i + 32 <= n; i += 32) {
        __m256i chunk = _mm256_loadu_si256((const __m256i *)(d + i));
        unsigned mask = (unsigned)_mm256_movemask_epi8(_mm256_cmpeq_epi8(chunk, vt));
        if (mask) return (ptrdiff_t)(i + (size_t)__builtin_ctz(mask));
    }
    for (; i < n; i++) {
        if (d[i] == t) return (ptrdiff_t)i;
    }
    return -1;
}
#endif /* x86-64 */

#if defined(__aarch64__) || defined(_M_ARM64)
#include <arm_neon.h>
/* NEON is the aarch64 baseline. No movemask: detect "any match in this 16-byte
 * block" with a horizontal max of the compare mask, then locate the first lane
 * scalarly (a block with a hit is rare, so the per-lane rescan is cheap). */
static ptrdiff_t zap_memchr_mvsimd_neon(const uint8_t *d, size_t n, uint8_t t) {
    size_t i = 0;
    uint8x16_t vt = vdupq_n_u8(t);
    for (; i + 16 <= n; i += 16) {
        uint8x16_t chunk = vld1q_u8(d + i);
        uint8x16_t eq = vceqq_u8(chunk, vt); /* 0xFF where equal, else 0x00 */
        if (vmaxvq_u8(eq)) {
            for (size_t j = 0; j < 16; j++) {
                if (d[i + j] == t) return (ptrdiff_t)(i + j);
            }
        }
    }
    for (; i < n; i++) {
        if (d[i] == t) return (ptrdiff_t)i;
    }
    return -1;
}
#endif /* aarch64 */

/* --- pointer-with-once-init dispatch -------------------------------------- */
typedef ptrdiff_t (*zap_memchr_fn)(const uint8_t *, size_t, uint8_t);

static zap_memchr_fn zap_memchr_mvsimd_resolve(void) {
#if defined(__x86_64__) || defined(_M_X64)
    if (zap_cpu_has(ZAP_CPU_X86_AVX2_FMA)) return zap_memchr_mvsimd_avx2;
    return zap_memchr_mvsimd_sse2; /* SSE2 is the guaranteed x86-64 floor */
#elif defined(__aarch64__) || defined(_M_ARM64)
    return zap_memchr_mvsimd_neon; /* NEON is the guaranteed aarch64 floor */
#else
    return zap_memchr_mvsimd_scalar;
#endif
}

/* Benign first-call race: concurrent callers resolve the IDENTICAL pointer, so an
 * overlapping store is harmless and no lock is needed. */
static zap_memchr_fn zap_memchr_mvsimd_selected;

/* First offset of byte `needle` in `hay[0..n)`, or -1. The public byte-search
 * entry every string/bytes path routes through. */
ptrdiff_t zap_memchr_off(const void *hay, size_t n, uint8_t needle) {
    zap_memchr_fn f = zap_memchr_mvsimd_selected;
    if (!f) {
        f = zap_memchr_mvsimd_resolve();
        zap_memchr_mvsimd_selected = f;
    }
    return f((const uint8_t *)hay, n, needle);
}

/* First offset of `needle[0..nn)` in `hay[0..hn)`, or -1. Empty needle -> 0.
 * Uses the multiversioned memchr to jump between candidate first-byte positions,
 * then memcmp to confirm the full needle — the memchr-crate / glibc strategy. The
 * memchr range is clamped to `[pos, hn-nn]` so a first-byte too near the end
 * (where a full needle could not fit) is never chased. */
ptrdiff_t zap_memmem_off(const void *hayv, size_t hn, const void *ndlv, size_t nn) {
    const uint8_t *hay = (const uint8_t *)hayv;
    const uint8_t *ndl = (const uint8_t *)ndlv;
    if (nn == 0) return 0;
    if (nn > hn) return -1;
    uint8_t first = ndl[0];
    size_t last_start = hn - nn; /* last index at which a full needle fits */
    size_t pos = 0;
    for (;;) {
        /* Only [pos, last_start] can start a match; search that many bytes. */
        size_t span = (last_start - pos) + 1;
        ptrdiff_t c = zap_memchr_off(hay + pos, span, first);
        if (c < 0) return -1;
        size_t cand = pos + (size_t)c;
        if (nn == 1 || memcmp(hay + cand, ndl, nn) == 0) return (ptrdiff_t)cand;
        pos = cand + 1;
        if (pos > last_start) return -1;
    }
}
