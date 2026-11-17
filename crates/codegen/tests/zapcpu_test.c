/* zapcpu_test.c — standalone unit test for the `cpu_features` detector (#943,
 * epic #139, D-0192). Compiled with -DZAP_CPU_TEST so it pulls in the pure
 * helpers of zapcpu.inc.c WITHOUT the zaprt.c string dependencies, and drives
 * them with SYNTHETIC inputs — so x86 XCR0 gating and aarch64 HWCAP mapping are
 * both exercised regardless of the host arch, which no single CI box could do
 * from live silicon alone. Driven by crates/codegen/tests/cpu_features_c.rs; also
 * runnable by hand:  cc -DZAP_CPU_TEST crates/codegen/tests/zapcpu_test.c -o t && ./t
 *
 * Exits 0 on success, 1 on the first-or-any failed check (all checks run; count
 * is printed). */

#define ZAP_CPU_TEST 1
#include "../src/runtime/accel/cpu.inc.c"

#include <stdio.h>

static int zt_fail = 0;
#define CHECK(cond, msg)                                                                           \
    do {                                                                                           \
        if (!(cond)) {                                                                             \
            printf("FAIL: %s  (%s:%d)\n", (msg), __FILE__, __LINE__);                              \
            zt_fail++;                                                                             \
        }                                                                                          \
    } while (0)

#define BIT(i) ((uint64_t)1 << (i))

static void test_x86_bits_xcr0_gating(void) {
    /* CPUID advertises SSE4.2 + AVX + FMA + AVX2 + AVX512F + OSXSAVE. The XCR0
     * value decides which wide tiers are actually usable. */
    uint32_t ecx1 = 0;
    ecx1 |= 1u << 20; /* SSE4.2  */
    ecx1 |= 1u << 12; /* FMA     */
    ecx1 |= 1u << 28; /* AVX     */
    ecx1 |= 1u << 27; /* OSXSAVE */
    uint32_t ebx7 = 0;
    ebx7 |= 1u << 5;  /* AVX2    */
    ebx7 |= 1u << 16; /* AVX512F */

    /* OS has NOT enabled wide state: AVX2_FMA and AVX512F must be reported ABSENT
     * even though every CPUID bit is set — the core XCR0 safety rule. */
    uint64_t f = zap_cpu__x86_bits(ecx1, ebx7, 0);
    CHECK(f & BIT(ZAP_CPU_X86_SSE2), "x86 SSE2 baseline always present");
    CHECK(f & BIT(ZAP_CPU_X86_SSE42), "x86 SSE4.2 present (no XCR0 gate)");
    CHECK(!(f & BIT(ZAP_CPU_X86_AVX2_FMA)), "x86 AVX2_FMA gated OFF when XCR0 lacks YMM");
    CHECK(!(f & BIT(ZAP_CPU_X86_AVX512F)), "x86 AVX512F gated OFF when XCR0 lacks ZMM");

    /* OS enables YMM (SSE+AVX) only: AVX2_FMA now on, AVX512F still off. */
    f = zap_cpu__x86_bits(ecx1, ebx7, ZAP_XCR0_YMM_MASK);
    CHECK(f & BIT(ZAP_CPU_X86_AVX2_FMA), "x86 AVX2_FMA on once XCR0 saves YMM");
    CHECK(!(f & BIT(ZAP_CPU_X86_AVX512F)), "x86 AVX512F still off without ZMM state");

    /* OS enables the full ZMM state: all four tiers on. */
    f = zap_cpu__x86_bits(ecx1, ebx7, ZAP_XCR0_ZMM_MASK);
    CHECK(f & BIT(ZAP_CPU_X86_AVX512F), "x86 AVX512F on once XCR0 saves ZMM");

    /* AVX2 requires BOTH avx2 and fma CPUID bits (bundled tier): drop FMA. */
    f = zap_cpu__x86_bits(ecx1 & ~(1u << 12), ebx7, ZAP_XCR0_ZMM_MASK);
    CHECK(!(f & BIT(ZAP_CPU_X86_AVX2_FMA)), "x86 AVX2_FMA needs FMA too (bundled)");

    /* Unknown/baseline hardware: nothing but the mandatory floor. */
    f = zap_cpu__x86_bits(0, 0, 0);
    CHECK(f == BIT(ZAP_CPU_X86_SSE2), "x86 baseline-only hardware => SSE2 only");
}

static void test_aarch64_bits(void) {
    uint64_t f = zap_cpu__aarch64_bits(ZAP_HWCAP_AES | ZAP_HWCAP_CRC32, ZAP_HWCAP2_SVE2);
    CHECK(f & BIT(ZAP_CPU_AARCH64_NEON), "aarch64 NEON baseline always present");
    CHECK(f & BIT(ZAP_CPU_AARCH64_AES), "aarch64 AES from HWCAP");
    CHECK(f & BIT(ZAP_CPU_AARCH64_CRC32), "aarch64 CRC32 from HWCAP");
    CHECK(f & BIT(ZAP_CPU_AARCH64_SVE2), "aarch64 SVE2 from HWCAP2");
    CHECK(!(f & BIT(ZAP_CPU_AARCH64_SHA2)), "aarch64 SHA2 absent when HWCAP bit clear");
    CHECK(!(f & BIT(ZAP_CPU_AARCH64_DOTPROD)), "aarch64 DOTPROD absent when HWCAP bit clear");

    f = zap_cpu__aarch64_bits(0, 0);
    CHECK(f == BIT(ZAP_CPU_AARCH64_NEON), "aarch64 baseline-only hardware => NEON only");

    /* Full HWCAP: every taxonomy bit set. */
    f = zap_cpu__aarch64_bits(ZAP_HWCAP_AES | ZAP_HWCAP_SHA2 | ZAP_HWCAP_CRC32 |
                                  ZAP_HWCAP_ASIMDDP | ZAP_HWCAP_SVE,
                              ZAP_HWCAP2_SVE2 | ZAP_HWCAP2_I8MM | ZAP_HWCAP2_SME);
    CHECK(f & BIT(ZAP_CPU_AARCH64_SME), "aarch64 SME from HWCAP2");
    CHECK(f & BIT(ZAP_CPU_AARCH64_I8MM), "aarch64 I8MM from HWCAP2");
    CHECK(f & BIT(ZAP_CPU_AARCH64_DOTPROD), "aarch64 DOTPROD from HWCAP");
}

static void test_name_tables(void) {
    CHECK(zap_cpu__x86_name_to_idx("sse2") == ZAP_CPU_X86_SSE2, "name sse2");
    CHECK(zap_cpu__x86_name_to_idx("AVX2") == ZAP_CPU_X86_AVX2_FMA, "name AVX2 case-insensitive");
    CHECK(zap_cpu__x86_name_to_idx("fma") == ZAP_CPU_X86_AVX2_FMA, "name fma aliases AVX2_FMA");
    CHECK(zap_cpu__x86_name_to_idx("sse4.2") == ZAP_CPU_X86_SSE42, "name sse4.2");
    CHECK(zap_cpu__x86_name_to_idx("sse42") == ZAP_CPU_X86_SSE42, "name sse42 alias");
    CHECK(zap_cpu__x86_name_to_idx("avx512") == ZAP_CPU_X86_AVX512F, "name avx512 alias");
    CHECK(zap_cpu__x86_name_to_idx("nope") == -1, "x86 unknown name => -1");
    CHECK(zap_cpu__x86_name_to_idx("sve") == -1, "sve is not an x86 name => -1");

    CHECK(zap_cpu__aarch64_name_to_idx("neon") == ZAP_CPU_AARCH64_NEON, "name neon");
    CHECK(zap_cpu__aarch64_name_to_idx("asimd") == ZAP_CPU_AARCH64_NEON, "name asimd alias");
    CHECK(zap_cpu__aarch64_name_to_idx("sve2") == ZAP_CPU_AARCH64_SVE2, "name sve2");
    CHECK(zap_cpu__aarch64_name_to_idx("sme") == ZAP_CPU_AARCH64_SME, "name sme");
    CHECK(zap_cpu__aarch64_name_to_idx("avx2") == -1, "avx2 is not an aarch64 name => -1");
}

static void test_env_overrides(void) {
    uint64_t x86_base = BIT(ZAP_CPU_X86_SSE2);
    uint64_t full = BIT(ZAP_CPU_X86_SSE2) | BIT(ZAP_CPU_X86_SSE42) | BIT(ZAP_CPU_X86_AVX2_FMA) |
                    BIT(ZAP_CPU_X86_AVX512F);

    /* ZAP_CPU=generic forces baseline-only; reports changed. */
    uint64_t feat[2] = {full, 0};
    int changed = zap_cpu__apply_overrides(feat, x86_base, zap_cpu__x86_name_to_idx, "generic", NULL,
                                           NULL);
    CHECK(changed, "generic override reports a change");
    CHECK(feat[0] == x86_base && feat[1] == 0, "generic => baseline only");

    /* ZAP_NO_AVX512 drops AVX512F only. */
    feat[0] = full;
    feat[1] = 0;
    changed = zap_cpu__apply_overrides(feat, x86_base, zap_cpu__x86_name_to_idx, NULL, "1", NULL);
    CHECK(changed, "ZAP_NO_AVX512 reports a change");
    CHECK(!(feat[0] & BIT(ZAP_CPU_X86_AVX512F)), "ZAP_NO_AVX512 clears AVX512F");
    CHECK(feat[0] & BIT(ZAP_CPU_X86_AVX2_FMA), "ZAP_NO_AVX512 keeps AVX2_FMA");

    /* ZAP_NO_AVX512=0 is falsey: no change. */
    feat[0] = full;
    feat[1] = 0;
    changed = zap_cpu__apply_overrides(feat, x86_base, zap_cpu__x86_name_to_idx, NULL, "0", NULL);
    CHECK(!changed && feat[0] == full, "ZAP_NO_AVX512=0 is a no-op");

    /* ZAP_DISABLE_CPU_FEATURES: per-name list, mixed separators. */
    feat[0] = full;
    feat[1] = 0;
    changed = zap_cpu__apply_overrides(feat, x86_base, zap_cpu__x86_name_to_idx, NULL, NULL,
                                       "avx2, sse42");
    CHECK(changed, "disable list reports a change");
    CHECK(!(feat[0] & BIT(ZAP_CPU_X86_AVX2_FMA)), "disable clears avx2");
    CHECK(!(feat[0] & BIT(ZAP_CPU_X86_SSE42)), "disable clears sse42");
    CHECK(feat[0] & BIT(ZAP_CPU_X86_AVX512F), "disable leaves unlisted avx512f");

    /* DOWNGRADE-ONLY: a disable list on baseline-only hardware can never ADD a
     * feature, and an unknown name is ignored. */
    feat[0] = x86_base;
    feat[1] = 0;
    changed = zap_cpu__apply_overrides(feat, x86_base, zap_cpu__x86_name_to_idx, NULL, NULL,
                                       "avx2 avx512f bogusname");
    CHECK(!changed, "disabling absent features adds nothing (no change)");
    CHECK(feat[0] == x86_base, "overrides never introduce a feature the silicon lacks");

    /* Baseline is never stranded, even if the user disables it. */
    feat[0] = full;
    feat[1] = 0;
    (void)zap_cpu__apply_overrides(feat, x86_base, zap_cpu__x86_name_to_idx, NULL, NULL, "sse2");
    CHECK(feat[0] & BIT(ZAP_CPU_X86_SSE2), "baseline re-asserted after an attempt to disable it");

    /* Same machinery on the aarch64 table + baseline. */
    uint64_t arm_base = BIT(ZAP_CPU_AARCH64_NEON);
    uint64_t arm_full = arm_base | BIT(ZAP_CPU_AARCH64_SVE) | BIT(ZAP_CPU_AARCH64_SVE2);
    feat[0] = arm_full;
    feat[1] = 0;
    changed = zap_cpu__apply_overrides(feat, arm_base, zap_cpu__aarch64_name_to_idx, "generic", NULL,
                                       NULL);
    CHECK(changed && feat[0] == arm_base, "aarch64 generic => NEON only");
}

static void test_live_detection(void) {
    /* On a real CPU the detector must report the mandatory floor and a concrete
     * source (never GENERIC on a supported arch), and must be stable call-to-call. */
    const zap_cpu_t *c = zap_cpu();
    CHECK(c == zap_cpu(), "zap_cpu() returns the same cached pointer");
    CHECK(zap_cpu_has(200) == 0, "out-of-range index => 0");
    CHECK(zap_cpu_has_name("totally-made-up") == -1, "unknown public name => -1");
#if defined(ZAP_CPU_ARCH_X86)
    CHECK(c->source == ZAP_CPU_SRC_CPUID || c->source == ZAP_CPU_SRC_ENV_OVERRIDE,
          "x86 source is CPUID (or ENV_OVERRIDE under a downgrade)");
    CHECK(zap_cpu_has_name("sse2") == 1, "x86 host advertises its SSE2 baseline");
    CHECK(zap_cpu_has_name("sve") == -1, "sve is not queryable on an x86 build");
#elif defined(ZAP_CPU_ARCH_AARCH64)
    CHECK(c->source != ZAP_CPU_SRC_GENERIC, "aarch64 source is a concrete detection path");
    CHECK(zap_cpu_has_name("neon") == 1, "aarch64 host advertises its NEON baseline");
    CHECK(zap_cpu_has_name("avx2") == -1, "avx2 is not queryable on an aarch64 build");
#endif
}

int main(void) {
    test_x86_bits_xcr0_gating();
    test_aarch64_bits();
    test_name_tables();
    test_env_overrides();
    test_live_detection();
    if (zt_fail == 0) {
        printf("zapcpu_test: all checks passed\n");
        return 0;
    }
    printf("zapcpu_test: %d check(s) FAILED\n", zt_fail);
    return 1;
}
