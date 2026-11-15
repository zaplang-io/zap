/* zap_cpu_features.h — shared CPU-feature taxonomy (ISA-acceleration epic #139,
 * sub-issue #943, D-0192; design snapshot Discussion #920 comment 4).
 *
 * SINGLE SOURCE OF TRUTH for the feature-bit indices read by the `cpu_features`
 * runtime detector (`zapcpu.inc.c`) AND, on the Rust side, by codegen's taxonomy
 * mirror (`crates/codegen/src/cpu_features.rs`). The two are locked together by
 * a unit test (`cpu_features::tests::header_indices_match_rust`) that parses the
 * `#define ZAP_CPU_<ARCH>_<FEAT>` lines below and asserts they equal the Rust
 * constants. This is the same cross-boundary-drift footgun class as `inst_dest`
 * / the `zapc_ir` re-exports: a disagreement between "codegen thinks AVX2 is bit
 * 2" and "the runtime sets it as bit 3" is a silent mis-dispatch that no single
 * build catches, so the numbers live HERE and nowhere else.
 *
 * Parser contract (do not reformat the index block without updating the Rust
 * parser): every feature-index line is exactly
 *     #define ZAP_CPU_<ARCH>_<NAME> <decimal-int>
 * with ARCH in {X86, AARCH64}, one per line, no arithmetic in the value.
 *
 * Per-arch namespacing (#920 c4): a compiled binary runs on exactly ONE arch, so
 * the x86 and aarch64 feature sets reuse the same bit positions (bit 2 = AVX2_FMA
 * on x86, = SHA2 on aarch64). No collision is possible within one binary, and it
 * doubles the headroom. Access is ALWAYS via zap_cpu_has(idx) / a name lookup —
 * never a bare `feat & BIT` at a call site — so the representation can grow and
 * the taxonomy renumber without touching any resolver.
 *
 * Curated, not raw silicon: a feature earns a bit only when a clone/kernel will
 * actually dispatch on it (#920 c4), so bit growth tracks kernels written (a
 * handful a year), not the hundreds of flags the CPU ships. New features are
 * appended with the next free index for their arch; indices are never reused.
 */
#ifndef ZAP_CPU_FEATURES_H
#define ZAP_CPU_FEATURES_H

#include <stdint.h>

/* --- feature-bit indices (THE source of truth; mirrored in Rust) ---------- */

/* x86-64. SSE2 is the AMD64-guaranteed baseline (always set); everything above
 * is probed via CPUID leaf 1 / leaf 7 and gated on XCR0 for the vector tiers.
 * AVX2 and FMA3 co-arrived on Haswell and are bundled as one tier — a kernel
 * never wants one without the other (#920 c4). */
#define ZAP_CPU_X86_SSE2 0
#define ZAP_CPU_X86_SSE42 1
#define ZAP_CPU_X86_AVX2_FMA 2
#define ZAP_CPU_X86_AVX512F 3

/* aarch64. NEON/ASIMD is the ARMv8-A-mandatory baseline (always set); the
 * optional axis is crypto/dotprod/i8mm and the scalable SVE family. SVE is a
 * single vector-length-agnostic clone whose width is reported separately via
 * sve_vl_bytes, NOT a width-indexed set of bits (#920 comment 3). */
#define ZAP_CPU_AARCH64_NEON 0
#define ZAP_CPU_AARCH64_AES 1
#define ZAP_CPU_AARCH64_SHA2 2
#define ZAP_CPU_AARCH64_CRC32 3
#define ZAP_CPU_AARCH64_DOTPROD 4
#define ZAP_CPU_AARCH64_I8MM 5
#define ZAP_CPU_AARCH64_SVE 6
#define ZAP_CPU_AARCH64_SVE2 7
#define ZAP_CPU_AARCH64_SME 8

/* --- `source` field values (how the answer was arrived at) ---------------- */
#define ZAP_CPU_SRC_GENERIC 0 /* baseline only (no detection path, or failure) */
#define ZAP_CPU_SRC_CPUID 1   /* x86 CPUID + XGETBV/XCR0                        */
#define ZAP_CPU_SRC_HWCAP 2   /* aarch64 Linux getauxval(AT_HWCAP/AT_HWCAP2)   */
#define ZAP_CPU_SRC_SYSCTL 3  /* aarch64 macOS sysctlbyname(hw.optional.arm.*) */
#define ZAP_CPU_SRC_WIN 4     /* aarch64 Windows IsProcessorFeaturePresent     */
#define ZAP_CPU_SRC_ENV_OVERRIDE 5 /* a ZAP_CPU=* / ZAP_NO_* override downgraded the result */

/* --- the process-lifetime-constant capability struct (#920 c4) ------------ */
typedef struct {
    uint64_t feat[2]; /* 128-bit bitset, future-proofed; indexed by the macros above */
    uint16_t sve_vl_bytes; /* SVE vector length in BYTES (runtime value; 0 if no SVE) */
    uint8_t avx10_level;   /* AVX10 version as a small int (0 = none); reserved       */
    uint8_t source;        /* one of ZAP_CPU_SRC_*                                     */
} zap_cpu_t;

/* Cached, compute-once, thread-safe, malloc-free & reentrant (safe to call from
 * an ifunc resolver before libc init — see zapcpu.inc.c). */
const zap_cpu_t *zap_cpu(void);

/* Index-based query; hides the word-split across feat[2]. `idx` is a
 * ZAP_CPU_<ARCH>_<FEAT> for the build arch. Out-of-range -> 0. */
int zap_cpu_has(unsigned idx);

/* Stable-NAME query (the only CPU surface exposed to Zap programs; the churning
 * internal bit indices are NOT public — #920 c4). `name` is a documented,
 * case-insensitive feature name for the build arch; a name this arch does not
 * define (e.g. "avx2" on aarch64) returns 0, never an error. Returns -1 only for
 * a name that is not in the taxonomy at all, so callers can distinguish
 * "feature absent" (0) from "unknown name" (-1); the Zap `sys.cpu_has` wrapper
 * collapses both to false.
 *
 * Stable documented names (stability is on the NAMES, not the bit positions):
 *   x86-64 : "sse2", "sse4.2"/"sse42", "avx2", "fma", "avx512f"/"avx512"
 *   aarch64: "neon"/"asimd", "aes", "sha2", "crc32", "dotprod", "i8mm",
 *            "sve", "sve2", "sme"
 * ("avx2" and "fma" both map to the bundled AVX2_FMA tier.) */
int zap_cpu_has_name(const char *name);

#endif /* ZAP_CPU_FEATURES_H */
