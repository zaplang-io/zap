/* zaparch.inc.c — the `arch.x86_64` / `arch.aarch64` intrinsic shims and the
 * portable `hash.crc32c` wrapper (I#139 Tier 2, #962, D-0199).
 *
 * `#include`d by zaprt.c after zapcpu.inc.c. Each `arch.*` intrinsic is a thin
 * shim over a compiler builtin carrying the `target(...)` attribute the hardware
 * instruction needs, so the C compiler emits the exact instruction / LLVM
 * intrinsic and LTO inlines the shim into the caller (so the "function call"
 * disappears at -O2). The shim word ABI is uniform: integer args arrive as
 * `int64_t` words (the lowering coerces them) and the shim truncates to the
 * real width; results are the natural width (`uint32_t` / `uint64_t`).
 *
 * CRC32C IS CASTAGNOLI (poly 0x1EDC6F41), NOT the ISO/zlib CRC32 (0x04C11DB7)
 * that `zlib.crc32`/gzip use (D-0079 §6.6). These never share a value; the name
 * is always `crc32c`, never a bare `crc32`.
 *
 * There is NO silent software fallback in the `arch.*` shims (D-0079 rule 0.3):
 * the x86 shims run the SSE4.2 `crc32` instruction and the aarch64 shims the
 * CRC32C-extension instruction unconditionally. On a CPU lacking the feature the
 * instruction is an illegal-instruction trap (SIGILL) — the honest contract of
 * `import arch.x86_64` ("you asked for the hardware CRC; guard it with `cfg` or
 * use the portable `hash.crc32c`"). The one correct software fallback lives in
 * `hash.crc32c` below, whose result is bit-identical to the hardware path. */

/* ---- CRC32C (Castagnoli) single-step intrinsics, hardware, per-ISA -------- */

#if defined(__x86_64__) || defined(_M_X64)
__attribute__((target("sse4.2"))) uint32_t zap_arch_x86_crc32c_u8(int64_t crc,
                                                                  int64_t v) {
    return (uint32_t)__builtin_ia32_crc32qi((unsigned int)(uint32_t)crc,
                                            (unsigned char)v);
}
__attribute__((target("sse4.2"))) uint32_t zap_arch_x86_crc32c_u16(int64_t crc,
                                                                   int64_t v) {
    return (uint32_t)__builtin_ia32_crc32hi((unsigned int)(uint32_t)crc,
                                            (unsigned short)v);
}
__attribute__((target("sse4.2"))) uint32_t zap_arch_x86_crc32c_u32(int64_t crc,
                                                                   int64_t v) {
    return (uint32_t)__builtin_ia32_crc32si((unsigned int)(uint32_t)crc,
                                            (unsigned int)(uint32_t)v);
}
__attribute__((target("sse4.2"))) uint64_t zap_arch_x86_crc32c_u64(int64_t crc,
                                                                   int64_t v) {
    return (uint64_t)__builtin_ia32_crc32di((unsigned long long)crc,
                                            (unsigned long long)v);
}
#endif /* x86-64 */

#if defined(__aarch64__) || defined(_M_ARM64)
__attribute__((target("crc"))) uint32_t zap_arch_aarch64_crc32c_u8(int64_t crc,
                                                                   int64_t v) {
    return __builtin_arm_crc32cb((uint32_t)crc, (uint8_t)v);
}
__attribute__((target("crc"))) uint32_t zap_arch_aarch64_crc32c_u16(int64_t crc,
                                                                    int64_t v) {
    return __builtin_arm_crc32ch((uint32_t)crc, (uint16_t)v);
}
__attribute__((target("crc"))) uint32_t zap_arch_aarch64_crc32c_u32(int64_t crc,
                                                                    int64_t v) {
    return __builtin_arm_crc32cw((uint32_t)crc, (uint32_t)v);
}
/* aarch64 `crc32cd` returns the 32-bit accumulator; widen to a u64 word so the
 * value ABI matches x86 `crc32q` (low 32 bits carry the CRC32C on both). */
__attribute__((target("crc"))) uint64_t zap_arch_aarch64_crc32c_u64(int64_t crc,
                                                                    int64_t v) {
    return (uint64_t)__builtin_arm_crc32cd((uint32_t)crc, (uint64_t)v);
}
#endif /* aarch64 */

/* ---- popcount (architecture-universal) ------------------------------------ */

uint32_t zap_arch_popcount_u32(int64_t v) {
    return (uint32_t)__builtin_popcount((unsigned int)(uint32_t)v);
}
uint32_t zap_arch_popcount_u64(int64_t v) {
    return (uint32_t)__builtin_popcountll((unsigned long long)v);
}

/* ---- prefetch (architecture-universal locality hint; never faults) -------- */

/* `__builtin_prefetch(addr, rw, locality)` needs compile-time-constant rw and
 * locality, so the run-time locality word selects among constant calls. rw=0
 * (read). locality 0 = NTA (non-temporal) .. 3 = T0 (all cache levels). A hint
 * may be dropped by the hardware; the program is correct whether or not it is
 * honoured. The `buf` word is a managed `bytes`/`str` pointer, so it is always a
 * valid address. */
void zap_arch_prefetch(void *buf, int64_t locality) {
    switch (locality) {
    case 0:
        __builtin_prefetch(buf, 0, 0);
        break;
    case 1:
        __builtin_prefetch(buf, 0, 1);
        break;
    case 2:
        __builtin_prefetch(buf, 0, 2);
        break;
    default:
        __builtin_prefetch(buf, 0, 3);
        break;
    }
}

/* ---- hash.crc32c — the portable friendly wrapper (D-0079 §1/§6.8) --------- */

/* A full CRC32C (Castagnoli) checksum over a `bytes`/`str` buffer, using the
 * standard iSCSI convention: reflected input/output, initial value 0xFFFFFFFF,
 * final XOR 0xFFFFFFFF. This is a *software* implementation on purpose — it is
 * portable (compiles and runs on every target, inside a `when`-free program) and
 * its result is BIT-IDENTICAL to folding the hardware `arch.*.crc32c_u8`
 * intrinsic over the same bytes from the same 0xFFFFFFFF seed with the same
 * final XOR. That identity is why a software fallback is correct *here* (the
 * answer is the same either way) but never inside the raw `arch.*` intrinsics
 * (there the question is "did I get the hardware instruction?"). A hardware
 * acceleration of this wrapper is an internal optimisation left for later; it
 * must not change the result. The reflected CRC32C polynomial is 0x82F63B78. */

static uint32_t zap_crc32c_table[256];
static int zap_crc32c_table_ready = 0;

static void zap_crc32c_build_table(void) {
    for (uint32_t i = 0; i < 256; i++) {
        uint32_t c = i;
        for (int k = 0; k < 8; k++) {
            c = (c & 1u) ? (0x82F63B78u ^ (c >> 1)) : (c >> 1);
        }
        zap_crc32c_table[i] = c;
    }
    zap_crc32c_table_ready = 1;
}

uint32_t zap_hash_crc32c(void *data) {
    ZapStr *s = (ZapStr *)data;
    if (!zap_crc32c_table_ready) {
        zap_crc32c_build_table();
    }
    const unsigned char *p = (const unsigned char *)s->data;
    int64_t n = s->len;
    uint32_t crc = 0xFFFFFFFFu;
    for (int64_t i = 0; i < n; i++) {
        crc = zap_crc32c_table[(crc ^ p[i]) & 0xFFu] ^ (crc >> 8);
    }
    return crc ^ 0xFFFFFFFFu;
}
