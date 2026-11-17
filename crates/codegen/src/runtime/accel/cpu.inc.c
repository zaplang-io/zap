/* zap runtime fragment: accel/cpu.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* zapcpu.inc.c — `cpu_features` runtime detection (ISA-acceleration epic #139,
 * sub-issue #943, D-0192; design snapshot Discussion #920 comment 4).
 *
 * One process-lifetime-constant capability struct (zap_cpu_t), computed once and
 * read-only thereafter — which is exactly what makes it coroutine-safe (no per-G
 * state, nothing to synchronize after init) and deterministic. Every future
 * multiversioning resolver (#944) and every arch intrinsic reads it to pick a
 * kernel variant; nobody else re-probes the CPU.
 *
 * CONSTRAINTS (from #920 c4 / #943):
 *  - Malloc-free & reentrant. An ifunc resolver (#944) may call zap_cpu() during
 *    relocation, BEFORE libc init: no malloc, no constructor-dependent locks, no
 *    I/O. Only getenv() (async-safe, allocation-free) and CPUID/HWCAP/sysctl.
 *  - Compute-once, publish with acquire/release; a benign recompute race is fine
 *    because the answer is identical. A constructor warms it where available.
 *  - Baseline is mandatory (SSE2 on x86, NEON on aarch64), so even total
 *    detection failure yields a correct CONSERVATIVE answer, never a crash.
 *
 * This file is #included by zaprt.c for the shipping build. It is ALSO compiled
 * standalone by the C unit test (crates/codegen/tests/cpu_features_c.rs ->
 * zapcpu_test.c) with -DZAP_CPU_TEST, which exercises the pure helpers below
 * (bit assembly, XCR0 gating, the env downgrade, the name tables) with synthetic
 * inputs on any host. The helpers for BOTH arches are compiled unconditionally
 * so the test covers x86 and aarch64 regardless of which it runs on; only the
 * live probing in zap_cpu__detect() and the Zap `sys.*` string wrappers are
 * arch/runtime gated. */

#include "zap_cpu_features.h"

#include <stdlib.h>
#include <string.h>

/* The per-arch pure helpers are compiled unconditionally (so the unit test can
 * check BOTH arches on one host), which leaves the opposite arch's helpers
 * unreferenced in a shipping single-arch build. Mark them maybe-unused so that
 * TU stays warning-clean without dropping the test coverage. */
#if defined(__GNUC__)
#define ZAP_CPU_MAYBE_UNUSED __attribute__((unused))
#else
#define ZAP_CPU_MAYBE_UNUSED
#endif

/* ---------------------------------------------------------------------------
 * Pure helpers — no I/O, no allocation, no globals. Host-independent and
 * unit-tested directly.
 * ------------------------------------------------------------------------- */

/* ASCII case-insensitive string equality (the taxonomy names are ASCII). */
static int zap_cpu__ieq(const char *a, const char *b) {
    for (;; a++, b++) {
        unsigned char ca = (unsigned char)*a, cb = (unsigned char)*b;
        if (ca >= 'A' && ca <= 'Z') ca = (unsigned char)(ca - 'A' + 'a');
        if (cb >= 'A' && cb <= 'Z') cb = (unsigned char)(cb - 'A' + 'a');
        if (ca != cb) return 0;
        if (ca == '\0') return 1;
    }
}

/* XCR0 bit masks for the OS-enabled-vector-state gate (Intel SDM Vol.1 §13). */
#define ZAP_XCR0_SSE 0x02u    /* bit 1: legacy SSE (XMM) state                */
#define ZAP_XCR0_AVX 0x04u    /* bit 2: AVX (YMM_Hi128) state                 */
#define ZAP_XCR0_OPMASK 0x20u /* bit 5: AVX-512 opmask (k0-k7)                */
#define ZAP_XCR0_ZMM_HI256 0x40u /* bit 6: ZMM_Hi256                         */
#define ZAP_XCR0_HI16_ZMM 0x80u  /* bit 7: Hi16_ZMM (zmm16-31)               */
#define ZAP_XCR0_YMM_MASK (ZAP_XCR0_SSE | ZAP_XCR0_AVX)
#define ZAP_XCR0_ZMM_MASK \
    (ZAP_XCR0_SSE | ZAP_XCR0_AVX | ZAP_XCR0_OPMASK | ZAP_XCR0_ZMM_HI256 | ZAP_XCR0_HI16_ZMM)

/* Assemble the x86-64 feature bitset from raw CPUID registers + XCR0.
 *   ecx1 = CPUID leaf 1, ECX   (OSXSAVE/AVX/FMA/SSE4.2 flags)
 *   ebx7 = CPUID leaf 7 sub 0, EBX (AVX2/AVX512F flags)
 *   xcr0 = XGETBV(0), valid only when OSXSAVE is set (ecx1 bit 27).
 *
 * THE XCR0 GATE IS MANDATORY (#920 c4): a CPUID flag saying "AVX" is NOT enough
 * — the OS must also have enabled saving the wide vector state, or the first
 * AVX instruction faults (#UD). So AVX2_FMA requires the YMM state to be enabled
 * in XCR0 and AVX512F the full ZMM state, on top of the CPUID bits. SSE2 (the
 * baseline) and SSE4.2 run on the always-saved XMM regs and need no XCR0 gate. */
ZAP_CPU_MAYBE_UNUSED static uint64_t zap_cpu__x86_bits(uint32_t ecx1, uint32_t ebx7, uint64_t xcr0) {
    uint64_t f = (uint64_t)1 << ZAP_CPU_X86_SSE2; /* AMD64-guaranteed baseline */

    if ((ecx1 >> 20) & 1u) f |= (uint64_t)1 << ZAP_CPU_X86_SSE42;

    int osxsave = (ecx1 >> 27) & 1u;
    int ymm_ok = osxsave && ((xcr0 & ZAP_XCR0_YMM_MASK) == ZAP_XCR0_YMM_MASK);
    int zmm_ok = osxsave && ((xcr0 & ZAP_XCR0_ZMM_MASK) == ZAP_XCR0_ZMM_MASK);

    int avx = (ecx1 >> 28) & 1u;
    int fma = (ecx1 >> 12) & 1u;
    int avx2 = (ebx7 >> 5) & 1u;
    /* AVX2 and FMA3 are bundled into one tier (they co-arrived on Haswell); emit
     * the bit only when both CPUID bits are set AND the OS saves YMM state. */
    if (ymm_ok && avx && avx2 && fma) f |= (uint64_t)1 << ZAP_CPU_X86_AVX2_FMA;

    int avx512f = (ebx7 >> 16) & 1u;
    if (zmm_ok && avx512f) f |= (uint64_t)1 << ZAP_CPU_X86_AVX512F;

    return f;
}

/* Linux aarch64 HWCAP / HWCAP2 bit positions (arch/arm64 uapi; stable ABI). We
 * spell them locally rather than relying on <asm/hwcap.h> being complete on an
 * old build host — the kernel never renumbers these. */
#define ZAP_HWCAP_ASIMD (1UL << 1)
#define ZAP_HWCAP_AES (1UL << 3)
#define ZAP_HWCAP_SHA2 (1UL << 6)
#define ZAP_HWCAP_CRC32 (1UL << 7)
#define ZAP_HWCAP_ASIMDDP (1UL << 20) /* dotprod */
#define ZAP_HWCAP_SVE (1UL << 22)
#define ZAP_HWCAP2_SVE2 (1UL << 1)
#define ZAP_HWCAP2_I8MM (1UL << 13)
#define ZAP_HWCAP2_SME (1UL << 23)

/* Assemble the aarch64 feature bitset from Linux getauxval(AT_HWCAP/AT_HWCAP2).
 * NEON/ASIMD is ARMv8-A-mandatory, so the baseline bit is always set regardless
 * of the HWCAP bit (a kernel that somehow omitted it must not strand us). */
ZAP_CPU_MAYBE_UNUSED static uint64_t zap_cpu__aarch64_bits(uint64_t hwcap, uint64_t hwcap2) {
    uint64_t f = (uint64_t)1 << ZAP_CPU_AARCH64_NEON; /* mandatory baseline */
    if (hwcap & ZAP_HWCAP_AES) f |= (uint64_t)1 << ZAP_CPU_AARCH64_AES;
    if (hwcap & ZAP_HWCAP_SHA2) f |= (uint64_t)1 << ZAP_CPU_AARCH64_SHA2;
    if (hwcap & ZAP_HWCAP_CRC32) f |= (uint64_t)1 << ZAP_CPU_AARCH64_CRC32;
    if (hwcap & ZAP_HWCAP_ASIMDDP) f |= (uint64_t)1 << ZAP_CPU_AARCH64_DOTPROD;
    if (hwcap & ZAP_HWCAP_SVE) f |= (uint64_t)1 << ZAP_CPU_AARCH64_SVE;
    if (hwcap2 & ZAP_HWCAP2_SVE2) f |= (uint64_t)1 << ZAP_CPU_AARCH64_SVE2;
    if (hwcap2 & ZAP_HWCAP2_I8MM) f |= (uint64_t)1 << ZAP_CPU_AARCH64_I8MM;
    if (hwcap2 & ZAP_HWCAP2_SME) f |= (uint64_t)1 << ZAP_CPU_AARCH64_SME;
    return f;
}

/* Stable-name -> bit-index tables, one per arch, both always compiled so the
 * unit test can check both on any host. A name not in the table returns -1
 * ("unknown name"), distinct from a known name whose bit is clear (feature
 * absent). "avx2" and "fma" are two spellings of the one bundled AVX2_FMA tier;
 * "sse42"/"sse4.2" and "avx512"/"avx512f"/"neon"/"asimd" are accepted aliases. */
ZAP_CPU_MAYBE_UNUSED static int zap_cpu__x86_name_to_idx(const char *name) {
    if (zap_cpu__ieq(name, "sse2")) return ZAP_CPU_X86_SSE2;
    if (zap_cpu__ieq(name, "sse4.2") || zap_cpu__ieq(name, "sse42")) return ZAP_CPU_X86_SSE42;
    if (zap_cpu__ieq(name, "avx2") || zap_cpu__ieq(name, "fma")) return ZAP_CPU_X86_AVX2_FMA;
    if (zap_cpu__ieq(name, "avx512f") || zap_cpu__ieq(name, "avx512")) return ZAP_CPU_X86_AVX512F;
    return -1;
}

ZAP_CPU_MAYBE_UNUSED static int zap_cpu__aarch64_name_to_idx(const char *name) {
    if (zap_cpu__ieq(name, "neon") || zap_cpu__ieq(name, "asimd")) return ZAP_CPU_AARCH64_NEON;
    if (zap_cpu__ieq(name, "aes")) return ZAP_CPU_AARCH64_AES;
    if (zap_cpu__ieq(name, "sha2")) return ZAP_CPU_AARCH64_SHA2;
    if (zap_cpu__ieq(name, "crc32")) return ZAP_CPU_AARCH64_CRC32;
    if (zap_cpu__ieq(name, "dotprod")) return ZAP_CPU_AARCH64_DOTPROD;
    if (zap_cpu__ieq(name, "i8mm")) return ZAP_CPU_AARCH64_I8MM;
    if (zap_cpu__ieq(name, "sve")) return ZAP_CPU_AARCH64_SVE;
    if (zap_cpu__ieq(name, "sve2")) return ZAP_CPU_AARCH64_SVE2;
    if (zap_cpu__ieq(name, "sme")) return ZAP_CPU_AARCH64_SME;
    return -1;
}

/* Is `s` a truthy env value? Present and not "0"/""/"false"/"no"/"off". */
static int zap_cpu__truthy(const char *s) {
    if (!s || !*s) return 0;
    if (zap_cpu__ieq(s, "0") || zap_cpu__ieq(s, "false") || zap_cpu__ieq(s, "no") ||
        zap_cpu__ieq(s, "off"))
        return 0;
    return 1;
}

/* Clear a feature bit in the 128-bit set. idx<0 (unknown name) is a no-op. */
static void zap_cpu__clear(uint64_t feat[2], int idx) {
    if (idx < 0 || idx >= 128) return;
    feat[idx >> 6] &= ~((uint64_t)1 << (idx & 63));
}

/* Apply the environment DOWNGRADE overrides. DOWNGRADE-ONLY by construction: it
 * only ever CLEARS bits (never sets one), so it can never reintroduce a SIGILL
 * by advertising a feature the silicon lacks — the one safety invariant of the
 * override surface (#920 c4). The arch baseline is re-asserted at the end so the
 * floor is never emptied, whatever the user disables.
 *
 *   ZAP_CPU=generic            -> force baseline only (the single knob the CI /
 *                                 qemu matrix uses to exercise the fallback
 *                                 clone on capable hardware).
 *   ZAP_NO_AVX512=1            -> drop AVX-512 (the Intel down-clock escape
 *                                 hatch; no-op off x86).
 *   ZAP_DISABLE_CPU_FEATURES="avx2 sse42 ..."  -> drop each named feature
 *                                 (space/comma/semicolon separated; NumPy's
 *                                 NPY_DISABLE_CPU_FEATURES precedent).
 *
 * Pure: the three env strings and the name resolver are passed in, so the unit
 * test drives it with synthetic values and either arch's table. Returns nonzero
 * if any bit was removed (the caller then stamps source = ENV_OVERRIDE). */
static int zap_cpu__apply_overrides(uint64_t feat[2], uint64_t baseline_mask,
                                    int (*name_to_idx)(const char *), const char *zap_cpu_env,
                                    const char *no_avx512_env, const char *disable_env) {
    uint64_t before0 = feat[0], before1 = feat[1];

    if (zap_cpu_env && (zap_cpu__ieq(zap_cpu_env, "generic") ||
                        zap_cpu__ieq(zap_cpu_env, "baseline"))) {
        feat[0] = 0;
        feat[1] = 0;
    }

    if (zap_cpu__truthy(no_avx512_env)) zap_cpu__clear(feat, name_to_idx("avx512f"));

    if (disable_env && *disable_env) {
        char tok[64];
        size_t ti = 0;
        for (const char *p = disable_env;; p++) {
            char c = *p;
            if (c == ' ' || c == ',' || c == ';' || c == '\t' || c == '\0') {
                if (ti > 0) {
                    tok[ti] = '\0';
                    zap_cpu__clear(feat, name_to_idx(tok));
                    ti = 0;
                }
                if (c == '\0') break;
            } else if (ti + 1 < sizeof tok) {
                tok[ti++] = c;
            }
        }
    }

    /* Never strand the floor: the baseline is always safe and always present. */
    feat[0] |= baseline_mask;

    return (feat[0] != before0) || (feat[1] != before1);
}

/* ---------------------------------------------------------------------------
 * Live probing — arch/OS gated. Not unit-tested directly (reads the real CPU);
 * the pure helpers above carry the logic the tests pin down.
 * ------------------------------------------------------------------------- */

#if defined(__x86_64__) || defined(_M_X64)
#define ZAP_CPU_ARCH_X86 1
#elif defined(__aarch64__) || defined(_M_ARM64)
#define ZAP_CPU_ARCH_AARCH64 1
#endif

#if defined(ZAP_CPU_ARCH_X86)
#include <cpuid.h>
#endif
#if defined(ZAP_CPU_ARCH_AARCH64) && defined(__linux__)
#include <sys/auxv.h>
#include <sys/prctl.h>
#ifndef PR_SVE_GET_VL
#define PR_SVE_GET_VL 51
#endif
#ifndef PR_SVE_VL_LEN_MASK
#define PR_SVE_VL_LEN_MASK 0xffff
#endif
#endif
#if defined(ZAP_CPU_ARCH_AARCH64) && defined(__APPLE__)
#include <sys/sysctl.h>
#endif
#if defined(ZAP_CPU_ARCH_AARCH64) && defined(_WIN32)
#include <windows.h>
#endif

#if defined(ZAP_CPU_ARCH_AARCH64) && defined(__APPLE__)
/* macOS has no HWCAP; each feature is a hw.optional.arm.FEAT_* sysctl (1/0). */
static int zap_cpu__darwin_has(const char *key) {
    int val = 0;
    size_t sz = sizeof val;
    if (sysctlbyname(key, &val, &sz, NULL, 0) != 0) return 0;
    return val != 0;
}
#endif

/* Probe the live CPU into `out` (baseline-correct even on total failure). */
static zap_cpu_t zap_cpu__detect(void) {
    zap_cpu_t c;
    c.feat[0] = 0;
    c.feat[1] = 0;
    c.sve_vl_bytes = 0;
    c.avx10_level = 0;
    c.source = ZAP_CPU_SRC_GENERIC;

#if defined(ZAP_CPU_ARCH_X86)
    {
        unsigned a = 0, b = 0, cc = 0, d = 0;
        uint32_t ecx1 = 0, ebx7 = 0;
        if (__get_cpuid(1, &a, &b, &cc, &d)) ecx1 = cc;
        unsigned a7 = 0, b7 = 0, c7 = 0, d7 = 0;
        if (__get_cpuid_count(7, 0, &a7, &b7, &c7, &d7)) ebx7 = b7;
        uint64_t xcr0 = 0;
        if ((ecx1 >> 27) & 1u) { /* OSXSAVE: XGETBV is now legal */
            unsigned eax = 0, edx = 0;
            __asm__ volatile("xgetbv" : "=a"(eax), "=d"(edx) : "c"(0));
            xcr0 = ((uint64_t)edx << 32) | eax;
        }
        c.feat[0] = zap_cpu__x86_bits(ecx1, ebx7, xcr0);
        c.source = ZAP_CPU_SRC_CPUID;
    }
#elif defined(ZAP_CPU_ARCH_AARCH64) && defined(__linux__)
    {
        unsigned long hw = getauxval(AT_HWCAP);
        unsigned long hw2 = getauxval(AT_HWCAP2);
        c.feat[0] = zap_cpu__aarch64_bits((uint64_t)hw, (uint64_t)hw2);
        c.source = ZAP_CPU_SRC_HWCAP;
        if (c.feat[0] & ((uint64_t)1 << ZAP_CPU_AARCH64_SVE)) {
            int vl = (int)prctl(PR_SVE_GET_VL);
            if (vl >= 0) {
                int bytes = vl & PR_SVE_VL_LEN_MASK;
                if (bytes > 0 && bytes <= 0xffff) c.sve_vl_bytes = (uint16_t)bytes;
            }
        }
    }
#elif defined(ZAP_CPU_ARCH_AARCH64) && defined(__APPLE__)
    {
        uint64_t f = (uint64_t)1 << ZAP_CPU_AARCH64_NEON; /* mandatory baseline */
        if (zap_cpu__darwin_has("hw.optional.arm.FEAT_AES")) f |= (uint64_t)1 << ZAP_CPU_AARCH64_AES;
        if (zap_cpu__darwin_has("hw.optional.arm.FEAT_SHA256"))
            f |= (uint64_t)1 << ZAP_CPU_AARCH64_SHA2;
        /* CRC32 is mandatory from ARMv8.1; Apple cores all have it. */
        if (zap_cpu__darwin_has("hw.optional.armv8_crc32"))
            f |= (uint64_t)1 << ZAP_CPU_AARCH64_CRC32;
        if (zap_cpu__darwin_has("hw.optional.arm.FEAT_DotProd"))
            f |= (uint64_t)1 << ZAP_CPU_AARCH64_DOTPROD;
        if (zap_cpu__darwin_has("hw.optional.arm.FEAT_I8MM"))
            f |= (uint64_t)1 << ZAP_CPU_AARCH64_I8MM;
        /* Apple Silicon (through M3) ships no SVE; M4 exposes SME2/SVE streaming.
         * Report SVE/SME per sysctl but leave sve_vl_bytes 0 (non-streaming VL is
         * not meaningful here) — the scalable clone reads width at use via the
         * streaming interface, not this field. */
        if (zap_cpu__darwin_has("hw.optional.arm.FEAT_SVE"))
            f |= (uint64_t)1 << ZAP_CPU_AARCH64_SVE;
        if (zap_cpu__darwin_has("hw.optional.arm.FEAT_SME")) f |= (uint64_t)1 << ZAP_CPU_AARCH64_SME;
        c.feat[0] = f;
        c.source = ZAP_CPU_SRC_SYSCTL;
    }
#elif defined(ZAP_CPU_ARCH_AARCH64) && defined(_WIN32)
    {
        uint64_t f = (uint64_t)1 << ZAP_CPU_AARCH64_NEON; /* mandatory baseline */
#ifdef PF_ARM_V8_CRYPTO_INSTRUCTIONS_AVAILABLE
        if (IsProcessorFeaturePresent(PF_ARM_V8_CRYPTO_INSTRUCTIONS_AVAILABLE)) {
            f |= (uint64_t)1 << ZAP_CPU_AARCH64_AES;
            f |= (uint64_t)1 << ZAP_CPU_AARCH64_SHA2;
        }
#endif
#ifdef PF_ARM_V8_CRC32_INSTRUCTIONS_AVAILABLE
        if (IsProcessorFeaturePresent(PF_ARM_V8_CRC32_INSTRUCTIONS_AVAILABLE))
            f |= (uint64_t)1 << ZAP_CPU_AARCH64_CRC32;
#endif
        c.feat[0] = f;
        c.source = ZAP_CPU_SRC_WIN;
    }
#else
    /* Unknown arch: GENERIC. No baseline bit is meaningful, so the set stays
     * empty and every has() returns 0 (every kernel then takes its scalar
     * path) — correct and crash-free. */
#endif

    /* Environment downgrade overrides (downgrade-only; see the helper). */
    {
        uint64_t baseline = 0;
#if defined(ZAP_CPU_ARCH_X86)
        baseline = (uint64_t)1 << ZAP_CPU_X86_SSE2;
        int (*n2i)(const char *) = zap_cpu__x86_name_to_idx;
#elif defined(ZAP_CPU_ARCH_AARCH64)
        baseline = (uint64_t)1 << ZAP_CPU_AARCH64_NEON;
        int (*n2i)(const char *) = zap_cpu__aarch64_name_to_idx;
#else
        int (*n2i)(const char *) = zap_cpu__x86_name_to_idx; /* unused on unknown arch */
#endif
        (void)baseline;
        (void)n2i;
#if defined(ZAP_CPU_ARCH_X86) || defined(ZAP_CPU_ARCH_AARCH64)
        if (zap_cpu__apply_overrides(c.feat, baseline, n2i, getenv("ZAP_CPU"),
                                     getenv("ZAP_NO_AVX512"), getenv("ZAP_DISABLE_CPU_FEATURES")))
            c.source = ZAP_CPU_SRC_ENV_OVERRIDE;
#endif
    }

    return c;
}

/* ---------------------------------------------------------------------------
 * Compute-once publication + public API.
 * ------------------------------------------------------------------------- */

static zap_cpu_t zap_cpu__g;
static volatile int zap_cpu__ready; /* 0 until published; acquire/release gated */

const zap_cpu_t *zap_cpu(void) {
    if (!__atomic_load_n(&zap_cpu__ready, __ATOMIC_ACQUIRE)) {
        /* Benign race: concurrent first-callers each compute the IDENTICAL
         * struct, so an overlapping store is harmless. No lock -> safe to call
         * from an ifunc resolver before libc init. */
        zap_cpu__g = zap_cpu__detect();
        __atomic_store_n(&zap_cpu__ready, 1, __ATOMIC_RELEASE);
    }
    return &zap_cpu__g;
}

int zap_cpu_has(unsigned idx) {
    if (idx >= 128) return 0;
    const zap_cpu_t *c = zap_cpu();
    return (int)((c->feat[idx >> 6] >> (idx & 63)) & 1u);
}

int zap_cpu_has_name(const char *name) {
    if (!name) return -1;
    int idx;
#if defined(ZAP_CPU_ARCH_X86)
    idx = zap_cpu__x86_name_to_idx(name);
#elif defined(ZAP_CPU_ARCH_AARCH64)
    idx = zap_cpu__aarch64_name_to_idx(name);
#else
    idx = -1;
#endif
    if (idx < 0) return -1;
    return zap_cpu_has((unsigned)idx);
}

#if defined(__GNUC__) && !defined(ZAP_CPU_TEST)
/* Warm the cache at startup where a constructor is available, so the common
 * case never pays the first-call probe on the hot path. Harmless if an ifunc
 * resolver already warmed it (idempotent). */
__attribute__((constructor)) static void zap_cpu__warm(void) { (void)zap_cpu(); }
#endif

/* ---------------------------------------------------------------------------
 * Zap `sys` surface — read-only introspection keyed by STABLE NAMES, the only
 * CPU surface a Zap program sees (the internal bit indices stay private; dispatch
 * stays hidden — #920 c4 / #943). These build Zap strings, so they depend on the
 * zaprt.c string helpers and are excluded from the standalone C unit test.
 * ------------------------------------------------------------------------- */
#ifndef ZAP_CPU_TEST

/* sys.cpu_has(name) -> bool. Lowered via call_bool, so the C return is an i64
 * 0/1. An unknown name (-1 from zap_cpu_has_name) collapses to false. */
int64_t zap_sys_cpu_has(void *name_) {
    const char *name = zap_sys_cstr((ZapStr *)name_);
    return zap_cpu_has_name(name) > 0 ? 1 : 0;
}

/* sys.cpu_source() -> str: which detection path produced the answer. */
void *zap_sys_cpu_source(void) {
    switch (zap_cpu()->source) {
    case ZAP_CPU_SRC_CPUID:
        return zap_str_new("cpuid", 5);
    case ZAP_CPU_SRC_HWCAP:
        return zap_str_new("hwcap", 5);
    case ZAP_CPU_SRC_SYSCTL:
        return zap_str_new("sysctl", 6);
    case ZAP_CPU_SRC_WIN:
        return zap_str_new("windows", 7);
    case ZAP_CPU_SRC_ENV_OVERRIDE:
        return zap_str_new("env_override", 12);
    default:
        return zap_str_new("generic", 7);
    }
}

/* sys.cpu_sve_vl() -> int64: SVE vector length in bytes (0 if no SVE). */
int64_t zap_sys_cpu_sve_vl(void) { return (int64_t)zap_cpu()->sve_vl_bytes; }

#endif /* ZAP_CPU_TEST */
