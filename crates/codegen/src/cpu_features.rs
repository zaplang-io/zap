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
