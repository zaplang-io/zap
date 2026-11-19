//! The single source of truth for the C runtime fragments embedded into the
//! compiler.
//!
//! The Zap C runtime is ONE translation unit: `zaprt.c` textually `#include`s
//! every `runtime/*.inc.c` fragment (there is a single `cc` compile over
//! `zaprt.c`, and the fragments are not separate objects). Historically, adding
//! or renaming a fragment required editing it in *six* hand-maintained places
//! (the `include_str!` const, the `#include` in `zaprt.c`, the object-link write
//! site in `lib.rs`, the REPL/JIT write site in `jit.rs`, `build.rs`, and the
//! runtime cache-key hash — twice). Those drifted: the cpu/memchr/arch fragments
//! shipped without being added to the cache-key hash, so editing only one of them
//! could reuse a stale runtime object (the #678 class of bug).
//!
//! This manifest collapses the write sites and the cache-key hashes into one list.
//! Adding a fragment now takes exactly two edits: a `RuntimeFile` row here (plus
//! its `include_str!` const in `lib.rs`), and the `#include` line in `zaprt.c`
//! (the one place `#include` *order* is semantic, so it stays explicit and
//! reviewable there). `build.rs` globs `src/runtime/**` for `rerun-if-changed`, so
//! no third list to maintain. The drift-guard test in `lib.rs` asserts that the
//! on-disk tree, this manifest, and the `#include`s in `zaprt.c` all agree, so a
//! missing wiring fails `cargo test -p zapc_codegen` on a laptop (no LLVM needed)
//! instead of only in full/arm64 CI or `zap test --doc`.
//!
//! See `crates/codegen/FOLDER_ARCHITECTURE.md` for the full model + the checklist
//! for adding a new stdlib module.

use std::hash::{Hash, Hasher};
use std::path::Path;

/// One embedded runtime C file.
pub(crate) struct RuntimeFile {
    /// Path RELATIVE to the per-build runtime dir, exactly matching how `zaprt.c`
    /// (or a fragment) `#include`s it. The write loop reproduces this layout under
    /// the temp build dir so every relative `#include` resolves.
    pub path: &'static str,
    /// The embedded file text (an `include_str!` const from `lib.rs`).
    pub text: &'static str,
    /// `true` when the file is part of the single `zaprt.c` translation unit and
    /// must be written beside `zaprt.c` on EVERY executable/REPL link. `false` for
    /// files that are separate objects compiled conditionally elsewhere (the SIMD
    /// sort kernels) but whose bytes still determine the runtime, so they take part
    /// in the cache key even though the unconditional write loop skips them.
    pub in_tu: bool,
}

/// Every runtime file whose bytes determine the compiled runtime. Order of the
/// `in_tu` rows here is irrelevant (files are written independently); the semantic
/// `#include` order lives in `zaprt.c`.
pub(crate) static RUNTIME_FRAGMENTS: &[RuntimeFile] = &[
    RuntimeFile {
        path: "zaprt.c",
        text: crate::RUNTIME_C,
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/gc.inc.c",
        text: crate::RUNTIME_GC_C,
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/race.inc.c",
        text: crate::RUNTIME_RACE_C,
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/accel/blas.inc.c",
        text: crate::RUNTIME_BLAS_C,
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/accel/random.inc.c",
        text: crate::RUNTIME_RANDOM_C,
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/accel/ziggurat_constants.h",
        text: crate::ZIGGURAT_H,
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/accel/zap_cpu_features.h",
        text: crate::CPU_FEATURES_H,
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/accel/cpu.inc.c",
        text: crate::RUNTIME_CPU_C,
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/accel/memchr.inc.c",
        text: crate::RUNTIME_MEMCHR_C,
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/accel/arch.inc.c",
        text: crate::RUNTIME_ARCH_C,
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/accel/sortaccel.inc.c",
        text: crate::RUNTIME_SORTACCEL_C,
        in_tu: true,
    },
    // Per-stdlib-module bodies lifted out of zaprt.c (#1023). Each is `#include`d
    // by zaprt.c at its original position (order is semantic). Inline `include_str!`
    // keeps "add a fragment" to two edits: a row here + the `#include` in zaprt.c.
    RuntimeFile {
        path: "runtime/array.inc.c",
        text: include_str!("runtime/array.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/str.inc.c",
        text: include_str!("runtime/str.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/os_fs.inc.c",
        text: include_str!("runtime/os_fs.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/crypto.inc.c",
        text: include_str!("runtime/crypto.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        // `uuid` module (#975, D-0232/D-0233): must be `#include`d AFTER
        // crypto.inc.c — it reuses that file's static CSPRNG / MD5 / SHA-1.
        path: "runtime/uuid.inc.c",
        text: include_str!("runtime/uuid.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/bytes.inc.c",
        text: include_str!("runtime/bytes.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        // Bundled IANA timezone tables (#1045, D-0225), generated by
        // tools/gen_tzdata.py. Included by zaprt.c immediately before
        // time_datetime.inc.c, which binary-searches these tables.
        path: "runtime/tzdata_generated.inc.c",
        text: include_str!("runtime/tzdata_generated.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/time_datetime.inc.c",
        text: include_str!("runtime/time_datetime.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/math_random.inc.c",
        text: include_str!("runtime/math_random.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/statistics.inc.c",
        text: include_str!("runtime/statistics.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/json.inc.c",
        text: include_str!("runtime/json.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/xml.inc.c",
        text: include_str!("runtime/xml.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/url.inc.c",
        text: include_str!("runtime/url.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/collections.inc.c",
        text: include_str!("runtime/collections.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/logging_argparse.inc.c",
        text: include_str!("runtime/logging_argparse.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/csv.inc.c",
        text: include_str!("runtime/csv.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/yaml.inc.c",
        text: include_str!("runtime/yaml.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/zlib.inc.c",
        text: include_str!("runtime/zlib.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/sync.inc.c",
        text: include_str!("runtime/sync.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/net_http.inc.c",
        text: include_str!("runtime/net_http.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/tls.inc.c",
        text: include_str!("runtime/tls.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/crypto_jose.inc.c",
        text: include_str!("runtime/crypto_jose.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/threading.inc.c",
        text: include_str!("runtime/threading.inc.c"),
        in_tu: true,
    },
    RuntimeFile {
        path: "runtime/profile.inc.c",
        text: include_str!("runtime/profile.inc.c"),
        in_tu: true,
    },
    // Separate C++ translation units (the per-ISA SIMD sort kernels): NOT
    // `#include`d into `zaprt.c`, written conditionally by their own sites, but a
    // change to one must still rebuild the runtime object, so they are hashed.
    RuntimeFile {
        path: "zapxsort.cpp",
        text: crate::RUNTIME_XSORT_CPP,
        in_tu: false,
    },
    RuntimeFile {
        path: "zaphwysort.cpp",
        text: crate::RUNTIME_HWYSORT_CPP,
        in_tu: false,
    },
];

/// Hash every runtime file's text into the runtime cache key. Replaces the two
/// hand-maintained `RUNTIME_*.hash(&mut h)` blocks in `lib.rs`, so a new fragment
/// can never silently miss the cache key again.
pub(crate) fn hash_runtime_fragments(h: &mut impl Hasher) {
    for f in RUNTIME_FRAGMENTS {
        f.text.hash(h);
    }
}

/// Write every in-TU fragment beside `zaprt.c` under `dir`, recreating the
/// `runtime/` subdirectory layout so each relative `#include` resolves. Replaces
/// both the object-link write site (`lib.rs`) and the REPL/JIT write site
/// (`jit.rs`), so the two can no longer drift.
pub(crate) fn write_tu_fragments(dir: &Path) -> std::io::Result<()> {
    for f in RUNTIME_FRAGMENTS {
        if !f.in_tu {
            continue;
        }
        let p = dir.join(f.path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, f.text)?;
    }
    Ok(())
}
