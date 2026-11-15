//! CPU-feature taxonomy — Rust mirror of the shared C header
//! `zap_cpu_features.h` (ISA-acceleration epic #139, sub-issue #943, D-0192;
//! design snapshot Discussion #920 comment 4).
//!
//! The feature-bit indices are defined ONCE, in `zap_cpu_features.h`, and read
//! by the C runtime detector (`zapcpu.inc.c`). The function-multiversioning
//! cloner and its resolvers (#944) are Rust and need the same numbers, so this
//! module mirrors them — and [`tests::header_indices_match_rust`] parses the
//! header at test time and asserts the two agree, bit for bit. A drift between
//! "codegen thinks AVX2 is bit 2" and "the runtime sets it as bit 3" is a silent
//! mis-dispatch that no single build catches (the same cross-boundary footgun
//! class as `inst_dest` and the `zapc_ir` re-exports), so this lock is the point
//! of the module, not an afterthought.
//!
//! Access is always by index through `zap_cpu_has(idx)` in the runtime; nothing
//! here encodes the word-split across `feat[2]`. Per-arch namespacing means x86
//! and aarch64 reuse the same bit positions (a binary is one arch), so the two
//! blocks below are independent numberings that happen to overlap.

/// x86-64 feature-bit indices (mirror of `ZAP_CPU_X86_*`).
pub mod x86 {
    pub const SSE2: u32 = 0;
    pub const SSE42: u32 = 1;
    pub const AVX2_FMA: u32 = 2;
    pub const AVX512F: u32 = 3;
}

/// aarch64 feature-bit indices (mirror of `ZAP_CPU_AARCH64_*`).
pub mod aarch64 {
    pub const NEON: u32 = 0;
    pub const AES: u32 = 1;
    pub const SHA2: u32 = 2;
    pub const CRC32: u32 = 3;
    pub const DOTPROD: u32 = 4;
    pub const I8MM: u32 = 5;
    pub const SVE: u32 = 6;
    pub const SVE2: u32 = 7;
    pub const SME: u32 = 8;
}

/// `source` field values (mirror of `ZAP_CPU_SRC_*`): how `zap_cpu()` arrived at
/// its answer. Kept in sync for #944's diagnostics and the `sys.cpu_source()`
/// surface.
pub mod source {
    pub const GENERIC: u8 = 0;
    pub const CPUID: u8 = 1;
    pub const HWCAP: u8 = 2;
    pub const SYSCTL: u8 = 3;
    pub const WIN: u8 = 4;
    pub const ENV_OVERRIDE: u8 = 5;
}

/// Every `(C macro name, index)` pair the header is expected to define, by arch.
/// The agreement test checks this list against the header exactly — same names,
/// same values, no extras on either side.
#[cfg(test)]
const X86_TAXONOMY: &[(&str, u32)] = &[
    ("ZAP_CPU_X86_SSE2", x86::SSE2),
    ("ZAP_CPU_X86_SSE42", x86::SSE42),
    ("ZAP_CPU_X86_AVX2_FMA", x86::AVX2_FMA),
    ("ZAP_CPU_X86_AVX512F", x86::AVX512F),
];

#[cfg(test)]
const AARCH64_TAXONOMY: &[(&str, u32)] = &[
    ("ZAP_CPU_AARCH64_NEON", aarch64::NEON),
    ("ZAP_CPU_AARCH64_AES", aarch64::AES),
    ("ZAP_CPU_AARCH64_SHA2", aarch64::SHA2),
    ("ZAP_CPU_AARCH64_CRC32", aarch64::CRC32),
    ("ZAP_CPU_AARCH64_DOTPROD", aarch64::DOTPROD),
    ("ZAP_CPU_AARCH64_I8MM", aarch64::I8MM),
    ("ZAP_CPU_AARCH64_SVE", aarch64::SVE),
    ("ZAP_CPU_AARCH64_SVE2", aarch64::SVE2),
    ("ZAP_CPU_AARCH64_SME", aarch64::SME),
];

/// The CPU architecture a feature tier belongs to. A compiled Zap binary targets
/// exactly one arch, so a `@multiversion` tier for the *other* arch can never be
/// selected; codegen (#944) ignores it when building the resolver and leaves its
/// clone at baseline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arch {
    X86,
    Aarch64,
}

/// Resolved spec for a `@multiversion` tier name (#944): which arch it belongs
/// to, its `cpu_features` bit index (what the resolver passes to `zap_cpu_has`),
/// and the LLVM `target-features` string codegen stamps on the clone (empty for
/// a baseline tier that needs no extra feature). The `"default"` tier has no
/// spec — it is the universal fallback, handled directly by codegen.
#[derive(Clone, Copy, Debug)]
pub struct TierSpec {
    pub arch: Arch,
    pub idx: u32,
    pub llvm_features: &'static str,
}

/// Map a stable tier name (as `@multiversion(...)` / sema's `MULTIVERSION_TIERS`
/// spell it, aliases included) to its [`TierSpec`]. `None` for `"default"` (the
/// fallback, which has no feature gate) and for any unknown name. The bit
/// indices come from [`x86`] / [`aarch64`] — the same mirror the C header is
/// locked to — so the resolver's `zap_cpu_has(idx)` and the runtime detector
/// never disagree. LLVM `target-features` spellings follow the backend's own
/// names (e.g. aarch64 CRC32 is `+crc`).
pub fn tier_spec(name: &str) -> Option<TierSpec> {
    use Arch::{Aarch64, X86};
    let (arch, idx, feats) = match name {
        // x86-64. SSE2/NEON are the baselines: a real clone, but no extra feature.
        "sse2" => (X86, x86::SSE2, ""),
        "sse4.2" | "sse42" => (X86, x86::SSE42, "+sse4.2"),
        "avx2" | "fma" => (X86, x86::AVX2_FMA, "+avx2,+fma"),
        "avx512f" | "avx512" => (X86, x86::AVX512F, "+avx512f"),
        // aarch64.
        "neon" | "asimd" => (Aarch64, aarch64::NEON, ""),
        "aes" => (Aarch64, aarch64::AES, "+aes"),
        "sha2" => (Aarch64, aarch64::SHA2, "+sha2"),
        "crc32" => (Aarch64, aarch64::CRC32, "+crc"),
        "dotprod" => (Aarch64, aarch64::DOTPROD, "+dotprod"),
        "i8mm" => (Aarch64, aarch64::I8MM, "+i8mm"),
        "sve" => (Aarch64, aarch64::SVE, "+sve"),
        "sve2" => (Aarch64, aarch64::SVE2, "+sve2"),
        "sme" => (Aarch64, aarch64::SME, "+sme"),
        _ => return None,
    };
    Some(TierSpec {
        arch,
        idx,
        llvm_features: feats,
    })
}

/// The target [`Arch`] of an LLVM triple string (`x86_64-…`, `aarch64-…` /
/// `arm64-…`). `None` for a triple whose arch carries no multiversioning tiers,
/// in which case codegen emits only the `"default"` clone for every group.
pub fn arch_of_triple(triple: &str) -> Option<Arch> {
    if triple.starts_with("x86_64") || triple.starts_with("amd64") {
        Some(Arch::X86)
    } else if triple.starts_with("aarch64") || triple.starts_with("arm64") {
        Some(Arch::Aarch64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// The exact text of the shared header, embedded at compile time.
    const HEADER: &str = include_str!("zap_cpu_features.h");

    /// Parse every `#define ZAP_CPU_<ARCH>_<NAME> <int>` line whose arch is X86
    /// or AARCH64 (the feature-index block), returning macro -> value. The
    /// `ZAP_CPU_SRC_*` and include-guard macros are deliberately excluded.
    fn parse_feature_indices() -> BTreeMap<String, u32> {
        let mut out = BTreeMap::new();
        for line in HEADER.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("#define ") else {
                continue;
            };
            let mut it = rest.split_whitespace();
            let (Some(name), Some(val)) = (it.next(), it.next()) else {
                continue;
            };
            if !(name.starts_with("ZAP_CPU_X86_") || name.starts_with("ZAP_CPU_AARCH64_")) {
                continue;
            }
            let v: u32 = val
                .parse()
                .unwrap_or_else(|_| panic!("non-integer feature index for {name}: {val:?}"));
            assert!(
                out.insert(name.to_string(), v).is_none(),
                "duplicate #define for {name} in zap_cpu_features.h"
            );
        }
        out
    }

    #[test]
    fn header_indices_match_rust() {
        let header = parse_feature_indices();

        let mut rust: BTreeMap<String, u32> = BTreeMap::new();
        for (n, v) in X86_TAXONOMY.iter().chain(AARCH64_TAXONOMY) {
            rust.insert((*n).to_string(), *v);
        }

        // Every macro the header defines is one the Rust mirror knows, same value.
        for (name, hv) in &header {
            match rust.get(name) {
                Some(rv) => assert_eq!(
                    hv, rv,
                    "feature index drift: header has {name}={hv}, Rust mirror has {rv}"
                ),
                None => panic!("header defines {name} but the Rust mirror is missing it"),
            }
        }
        // ...and every macro the Rust mirror names is actually in the header.
        for name in rust.keys() {
            assert!(
                header.contains_key(name),
                "Rust mirror names {name} but zap_cpu_features.h does not define it"
            );
        }
    }

    /// Within one arch the indices must be a dense, collision-free numbering
    /// from 0 (bits are a scarce, append-only resource; a gap or a reuse is a
    /// taxonomy bug).
    #[test]
    fn tier_spec_matches_taxonomy_and_aliases() {
        // Aliases resolve to the same bit as the canonical name.
        assert_eq!(tier_spec("sse42").unwrap().idx, x86::SSE42);
        assert_eq!(tier_spec("sse4.2").unwrap().idx, x86::SSE42);
        assert_eq!(tier_spec("avx2").unwrap().idx, x86::AVX2_FMA);
        assert_eq!(tier_spec("fma").unwrap().idx, x86::AVX2_FMA);
        assert_eq!(tier_spec("avx512").unwrap().idx, x86::AVX512F);
        assert_eq!(tier_spec("asimd").unwrap().idx, aarch64::NEON);
        assert_eq!(tier_spec("sve2").unwrap().idx, aarch64::SVE2);
        // Arch is classified correctly.
        assert_eq!(tier_spec("avx2").unwrap().arch, Arch::X86);
        assert_eq!(tier_spec("sve").unwrap().arch, Arch::Aarch64);
        // The AVX2+FMA bundle carries both LLVM features.
        assert_eq!(tier_spec("avx2").unwrap().llvm_features, "+avx2,+fma");
        // aarch64 CRC32's LLVM feature name is `+crc`, not `+crc32`.
        assert_eq!(tier_spec("crc32").unwrap().llvm_features, "+crc");
        // Baseline tiers resolve but add no feature.
        assert_eq!(tier_spec("sse2").unwrap().llvm_features, "");
        assert_eq!(tier_spec("neon").unwrap().llvm_features, "");
        // `default` and unknown names have no spec.
        assert!(tier_spec("default").is_none());
        assert!(tier_spec("bogus").is_none());
        // Every sema-accepted tier name (besides `default`) must resolve here, so
        // sema and codegen never disagree about what names are valid.
        for name in [
            "sse2", "sse4.2", "sse42", "avx2", "fma", "avx512f", "avx512", "neon", "asimd", "aes",
            "sha2", "crc32", "dotprod", "i8mm", "sve", "sve2", "sme",
        ] {
            assert!(tier_spec(name).is_some(), "tier {name:?} must resolve");
        }
    }

    #[test]
    fn arch_of_triple_classifies() {
        assert_eq!(arch_of_triple("x86_64-unknown-linux-gnu"), Some(Arch::X86));
        assert_eq!(
            arch_of_triple("aarch64-unknown-linux-gnu"),
            Some(Arch::Aarch64)
        );
        assert_eq!(arch_of_triple("arm64-apple-darwin"), Some(Arch::Aarch64));
        assert_eq!(arch_of_triple("wasm32-unknown-unknown"), None);
    }

    #[test]
    fn indices_are_dense_and_unique_per_arch() {
        for taxonomy in [X86_TAXONOMY, AARCH64_TAXONOMY] {
            let mut idxs: Vec<u32> = taxonomy.iter().map(|(_, v)| *v).collect();
            idxs.sort_unstable();
            for (expected, got) in idxs.iter().enumerate() {
                assert_eq!(
                    expected as u32, *got,
                    "arch taxonomy must be dense from 0 with no gaps/dupes: {idxs:?}"
                );
            }
            assert!(
                *idxs.last().unwrap() < 128,
                "a feature index must fit the 128-bit set"
            );
        }
    }
}
