/* zaparch_test.c — unit test for the arch intrinsic shims + hash.crc32c (#962,
 * epic #139, D-0199). Compiles `zaparch.inc.c` standalone and exercises:
 *   - popcount_u32/u64 against a reference, across edge values;
 *   - the hardware CRC32C single-step intrinsics (arch-gated: only the build's
 *     own arch's symbols exist) folded over bytes, checked against an
 *     independent bitwise CRC32C ground truth and the canonical check value;
 *   - crc32c_u16/u32/u64 consistency with folding crc32c_u8 over the little-
 *     endian bytes (the instruction's documented semantics);
 *   - hash.crc32c against known vectors and bit-identity with the folded raw
 *     intrinsic (the "friendly wrapper == raw intrinsic" acceptance).
 * A single CI host has only one arch's CRC instruction, so the crc32c intrinsic
 * checks are arch-gated; popcount / hash run everywhere. Exits 0 on success.
 *
 *   cc crates/codegen/tests/zaparch_test.c -o t && ./t */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Minimal ZapStr matching zaprt.c's layout (len @0, inline bytes @8), so the
 * `hash.crc32c` wrapper can be tested without the full runtime. */
typedef struct {
    int64_t len;
    char data[];
} ZapStr;

#include "../src/runtime/accel/arch.inc.c"

static int zt_fail = 0;
#define CHECK(cond, msg)                                                                           \
    do {                                                                                           \
        if (!(cond)) {                                                                             \
            printf("FAIL: %s  (%s:%d)\n", (msg), __FILE__, __LINE__);                              \
            zt_fail++;                                                                             \
        }                                                                                          \
    } while (0)

/* Independent bitwise CRC32C (Castagnoli) ground truth: reflected, init
 * 0xFFFFFFFF, final XOR 0xFFFFFFFF, reflected poly 0x82F63B78. No lookup table,
 * so it cannot share a bug with the wrapper's table. */
static uint32_t ref_crc32c(const uint8_t *p, size_t n) {
    uint32_t crc = 0xFFFFFFFFu;
    for (size_t i = 0; i < n; i++) {
        crc ^= p[i];
        for (int k = 0; k < 8; k++) {
            crc = (crc & 1u) ? (0x82F63B78u ^ (crc >> 1)) : (crc >> 1);
        }
    }
    return crc ^ 0xFFFFFFFFu;
}

/* Build a heap ZapStr from a C buffer. Leaked (test process is short-lived). */
static ZapStr *mkstr(const void *buf, int64_t n) {
    ZapStr *s = (ZapStr *)malloc(sizeof(ZapStr) + (size_t)n);
    s->len = n;
    memcpy(s->data, buf, (size_t)n);
    return s;
}

/* Fold the raw hardware crc32c_u8 step over bytes with the standard init/xorout
 * — the arch-specific symbol is chosen by the build's architecture. */
static uint32_t fold_raw_crc32c(const uint8_t *p, size_t n) {
    uint32_t crc = 0xFFFFFFFFu;
    for (size_t i = 0; i < n; i++) {
#if defined(__x86_64__) || defined(_M_X64)
        crc = zap_arch_x86_crc32c_u8((int64_t)crc, (int64_t)p[i]);
#elif defined(__aarch64__) || defined(_M_ARM64)
        crc = zap_arch_aarch64_crc32c_u8((int64_t)crc, (int64_t)p[i]);
#endif
    }
    return crc ^ 0xFFFFFFFFu;
}

int main(void) {
    /* ---- popcount (universal) ---- */
    CHECK(zap_arch_popcount_u64(0) == 0, "popcount_u64(0)");
    CHECK(zap_arch_popcount_u64(255) == 8, "popcount_u64(255)");
    CHECK(zap_arch_popcount_u64(0xFFFFFFFFFFFFFFFFull) == 64, "popcount_u64(~0)");
    CHECK(zap_arch_popcount_u32(0) == 0, "popcount_u32(0)");
    CHECK(zap_arch_popcount_u32(0xFFFFFFFFu) == 32, "popcount_u32(~0)");
    CHECK(zap_arch_popcount_u32(0xAAAAAAAAu) == 16, "popcount_u32(0xAAAAAAAA)");
    /* The u64 shim must count all 64 bits, not just the low 32. */
    CHECK(zap_arch_popcount_u64(0xFFFFFFFF00000000ull) == 32, "popcount_u64 high word");

    /* ---- hash.crc32c (universal software wrapper), known vectors ---- */
    {
        ZapStr *empty = mkstr("", 0);
        CHECK(zap_hash_crc32c(empty) == 0u, "crc32c(\"\") == 0");
        const char *check = "123456789";
        ZapStr *cs = mkstr(check, 9);
        CHECK(zap_hash_crc32c(cs) == 0xE3069283u, "crc32c(\"123456789\") == 0xE3069283");
        /* The wrapper agrees with the independent bitwise ground truth. */
        const char *msg = "The quick brown fox jumps over the lazy dog";
        ZapStr *ms = mkstr(msg, (int64_t)strlen(msg));
        CHECK(zap_hash_crc32c(ms) == ref_crc32c((const uint8_t *)msg, strlen(msg)),
              "crc32c(fox) == bitwise ref");
    }

#if (defined(__x86_64__) || defined(_M_X64) || defined(__aarch64__) || defined(_M_ARM64))
    /* ---- hardware crc32c single-step intrinsics (this arch) ---- */
    {
        /* Folding the raw hardware step reproduces the canonical check value and
         * is bit-identical to the portable wrapper — the acceptance invariant. */
        const char *check = "123456789";
        CHECK(fold_raw_crc32c((const uint8_t *)check, 9) == 0xE3069283u,
              "raw crc32c fold == 0xE3069283");
        ZapStr *cs = mkstr(check, 9);
        CHECK(fold_raw_crc32c((const uint8_t *)check, 9) == zap_hash_crc32c(cs),
              "raw crc32c fold == hash.crc32c");
        const char *msg = "The quick brown fox jumps over the lazy dog";
        size_t mn = strlen(msg);
        CHECK(fold_raw_crc32c((const uint8_t *)msg, mn) == ref_crc32c((const uint8_t *)msg, mn),
              "raw crc32c fold == bitwise ref (fox)");

        /* crc32c_u16/u32/u64 process a 2/4/8-byte little-endian word in one step,
         * which must equal folding crc32c_u8 over those LE bytes. */
        uint32_t seed = 0x12345678u;
        uint16_t v16 = 0xBEEFu;
        uint32_t v32 = 0xDEADBEEFu;
        uint64_t v64 = 0x0123456789ABCDEFull;
        uint8_t le16[2], le32[4], le64[8];
        for (int i = 0; i < 2; i++)
            le16[i] = (uint8_t)(v16 >> (8 * i));
        for (int i = 0; i < 4; i++)
            le32[i] = (uint8_t)(v32 >> (8 * i));
        for (int i = 0; i < 8; i++)
            le64[i] = (uint8_t)(v64 >> (8 * i));
        uint32_t step_from_bytes = seed;
        for (int i = 0; i < 2; i++) {
#if defined(__x86_64__) || defined(_M_X64)
            step_from_bytes = zap_arch_x86_crc32c_u8((int64_t)step_from_bytes, le16[i]);
#else
            step_from_bytes = zap_arch_aarch64_crc32c_u8((int64_t)step_from_bytes, le16[i]);
#endif
        }
#if defined(__x86_64__) || defined(_M_X64)
        CHECK((uint32_t)zap_arch_x86_crc32c_u16((int64_t)seed, (int64_t)v16) == step_from_bytes,
              "crc32c_u16 == fold of LE bytes");
#else
        CHECK((uint32_t)zap_arch_aarch64_crc32c_u16((int64_t)seed, (int64_t)v16) == step_from_bytes,
              "crc32c_u16 == fold of LE bytes");
#endif
        step_from_bytes = seed;
        for (int i = 0; i < 4; i++) {
#if defined(__x86_64__) || defined(_M_X64)
            step_from_bytes = zap_arch_x86_crc32c_u8((int64_t)step_from_bytes, le32[i]);
#else
            step_from_bytes = zap_arch_aarch64_crc32c_u8((int64_t)step_from_bytes, le32[i]);
#endif
        }
#if defined(__x86_64__) || defined(_M_X64)
        CHECK((uint32_t)zap_arch_x86_crc32c_u32((int64_t)seed, (int64_t)v32) == step_from_bytes,
              "crc32c_u32 == fold of LE bytes");
#else
        CHECK((uint32_t)zap_arch_aarch64_crc32c_u32((int64_t)seed, (int64_t)v32) == step_from_bytes,
              "crc32c_u32 == fold of LE bytes");
#endif
        step_from_bytes = seed;
        for (int i = 0; i < 8; i++) {
#if defined(__x86_64__) || defined(_M_X64)
            step_from_bytes = zap_arch_x86_crc32c_u8((int64_t)step_from_bytes, le64[i]);
#else
            step_from_bytes = zap_arch_aarch64_crc32c_u8((int64_t)step_from_bytes, le64[i]);
#endif
        }
#if defined(__x86_64__) || defined(_M_X64)
        CHECK((uint32_t)zap_arch_x86_crc32c_u64((int64_t)seed, (int64_t)v64) == step_from_bytes,
              "crc32c_u64 low32 == fold of LE bytes");
#else
        CHECK((uint32_t)zap_arch_aarch64_crc32c_u64((int64_t)seed, (int64_t)v64) == step_from_bytes,
              "crc32c_u64 low32 == fold of LE bytes");
#endif
    }
#endif /* has a CRC arch */

    if (zt_fail == 0) {
        printf("zaparch_test: all checks passed\n");
        return 0;
    }
    printf("zaparch_test: %d FAILURES\n", zt_fail);
    return 1;
}
