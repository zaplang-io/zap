/* zapmemchr_test.c — unit test for the SIMD memchr/memmem kernel (#945, epic
 * #139, D-0194). Compiles the detector (zapcpu.inc.c, in ZAP_CPU_TEST mode so its
 * Zap string wrappers drop out) + the kernel (zapmemchr.inc.c) and exercises:
 *   - each available clone against a scalar reference across EVERY length that
 *     crosses a vector-width boundary (0, 1, w-1, w, w+1, multi-block + tail),
 *     with the needle at the start / middle / end / absent, and byte value 0;
 *   - the dispatched zap_memchr_off end to end;
 *   - zap_memmem_off against a brute-force substring search: needle lengths
 *     crossing the width, at start/middle/end/absent, overlapping matches, needle
 *     longer than the haystack, and the empty needle.
 * A single CI host only has one arch's SIMD, so the clones are arch-gated; the
 * dispatch + memmem paths run everywhere. Exits 0 on success, 1 on any failure.
 *
 *   cc -DZAP_CPU_TEST crates/codegen/tests/zapmemchr_test.c -o t && ./t */

#define ZAP_CPU_TEST 1
#include "../src/zapcpu.inc.c"
#include "../src/zapmemchr.inc.c"

#include <stdio.h>
#include <stdlib.h>

static int zt_fail = 0;
#define CHECK(cond, msg)                                                                           \
    do {                                                                                           \
        if (!(cond)) {                                                                             \
            printf("FAIL: %s  (%s:%d)\n", (msg), __FILE__, __LINE__);                              \
            zt_fail++;                                                                             \
        }                                                                                          \
    } while (0)

/* Ground truth: linear byte scan. */
static ptrdiff_t ref_memchr(const uint8_t *d, size_t n, uint8_t t) {
    for (size_t i = 0; i < n; i++) {
        if (d[i] == t) return (ptrdiff_t)i;
    }
    return -1;
}
/* Ground truth: naive substring scan. */
static ptrdiff_t ref_memmem(const uint8_t *h, size_t hn, const uint8_t *nd, size_t nn) {
    if (nn == 0) return 0;
    if (nn > hn) return -1;
    for (size_t i = 0; i + nn <= hn; i++) {
        if (memcmp(h + i, nd, nn) == 0) return (ptrdiff_t)i;
    }
    return -1;
}

/* Compare one memchr clone to the reference over a buffer, for a byte present at
 * `plant` (SIZE_MAX = absent) and the absent case. */
typedef ptrdiff_t (*mc_fn)(const uint8_t *, size_t, uint8_t);
static void exercise_clone(const char *name, mc_fn f) {
    /* Lengths that straddle 16- and 32-byte lanes, plus tails. */
    static const size_t lens[] = {0,  1,  2,  15, 16, 17, 31,  32,
                                  33, 47, 48, 49, 63, 64, 65,  200};
    uint8_t buf[256];
    for (size_t li = 0; li < sizeof lens / sizeof lens[0]; li++) {
        size_t n = lens[li];
        /* Fill with a filler that is NOT the needle. */
        for (size_t i = 0; i < n; i++) buf[i] = 0xAA;
        const uint8_t needle = 0x5B;
        /* Absent. */
        CHECK(f(buf, n, needle) == ref_memchr(buf, n, needle), name);
        CHECK(f(buf, n, needle) == -1, name);
        /* Needle at every position (start, middle, end, each lane boundary). */
        for (size_t p = 0; p < n; p++) {
            buf[p] = needle;
            ptrdiff_t got = f(buf, n, needle);
            CHECK(got == (ptrdiff_t)p, name); /* first match is the earliest plant */
            CHECK(got == ref_memchr(buf, n, needle), name);
            buf[p] = 0xAA;
        }
        /* Two matches: the FIRST must win. */
        if (n >= 2) {
            buf[n - 1] = needle;
            buf[n / 2] = needle;
            CHECK(f(buf, n, needle) == (ptrdiff_t)(n / 2), name);
            buf[n - 1] = 0xAA;
            buf[n / 2] = 0xAA;
        }
        /* Searching for 0x00 must work (no NUL-termination assumptions). */
        if (n >= 1) {
            buf[n - 1] = 0x00;
            CHECK(f(buf, n, 0x00) == (ptrdiff_t)(n - 1), name);
            buf[n - 1] = 0xAA;
        }
    }
}

static void test_clones(void) {
    exercise_clone("scalar", zap_memchr_mvsimd_scalar);
#if defined(__x86_64__) || defined(_M_X64)
    exercise_clone("sse2", zap_memchr_mvsimd_sse2);
    exercise_clone("avx2", zap_memchr_mvsimd_avx2);
#endif
#if defined(__aarch64__) || defined(_M_ARM64)
    exercise_clone("neon", zap_memchr_mvsimd_neon);
#endif
}

static void test_dispatch(void) {
    /* The resolved pointer must behave exactly like the scalar reference, and be
     * stable call-to-call. */
    uint8_t buf[128];
    for (size_t i = 0; i < sizeof buf; i++) buf[i] = (uint8_t)(i * 7 + 1);
    for (int t = 0; t < 256; t++) {
        ptrdiff_t got = zap_memchr_off(buf, sizeof buf, (uint8_t)t);
        CHECK(got == ref_memchr(buf, sizeof buf, (uint8_t)t), "dispatch vs ref");
    }
    CHECK(zap_memchr_off(buf, 0, 0x01) == -1, "empty haystack => -1");
}

static void test_memmem(void) {
    /* Haystack with repeats so overlapping and multi-candidate cases arise. */
    uint8_t h[128];
    for (size_t i = 0; i < sizeof h; i++) h[i] = (uint8_t)(i % 5); /* 0,1,2,3,4,0,1,… */

    /* Empty needle -> 0; needle longer than haystack -> -1. */
    CHECK(zap_memmem_off(h, sizeof h, h, 0) == 0, "empty needle => 0");
    uint8_t big[200];
    memset(big, 7, sizeof big);
    CHECK(zap_memmem_off(h, sizeof h, big, sizeof big) == -1, "needle > haystack => -1");

    /* Known substrings at the start, interior, and end. */
    uint8_t start[] = {0, 1, 2};
    CHECK(zap_memmem_off(h, sizeof h, start, 3) == 0, "match at start");
    uint8_t end3[] = {2, 3, 4};
    /* last full {2,3,4} starts where (i%5)==2, the greatest such i+2 < len. */
    CHECK(zap_memmem_off(h, sizeof h, end3, 3) == ref_memmem(h, sizeof h, end3, 3), "match interior");
    uint8_t absent[] = {0, 2, 4};
    CHECK(zap_memmem_off(h, sizeof h, absent, 3) == -1, "absent substring => -1");

    /* Overlapping matches: the FIRST occurrence wins. */
    uint8_t ov[] = "aabaabaa";
    uint8_t pat[] = "aab";
    CHECK(zap_memmem_off(ov, 8, pat, 3) == 0, "overlapping: first match");

    /* Brute-force cross-check over many needle lengths crossing the vector width,
     * at every start position (present) and a guaranteed-absent needle. */
    for (size_t nn = 1; nn <= 40; nn++) {
        for (size_t start_at = 0; start_at + nn <= sizeof h; start_at++) {
            CHECK(zap_memmem_off(h, sizeof h, h + start_at, nn) ==
                      ref_memmem(h, sizeof h, h + start_at, nn),
                  "memmem vs brute force (present)");
        }
        /* An absent needle of this length: all 0xFF never occurs in h. */
        uint8_t miss[40];
        memset(miss, 0xFF, nn);
        CHECK(zap_memmem_off(h, sizeof h, miss, nn) == -1, "memmem absent => -1");
    }
}

int main(void) {
    test_clones();
    test_dispatch();
    test_memmem();
    if (zt_fail == 0) {
        printf("zapmemchr_test: all checks passed\n");
        return 0;
    }
    printf("zapmemchr_test: %d check(s) FAILED\n", zt_fail);
    return 1;
}
