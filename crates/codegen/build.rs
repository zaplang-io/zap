//! Build script for `zapc_codegen`.
//!
//! The crate embeds the C runtime sources into the compiler binary with
//! `include_str!` (see `src/lib.rs`: `RUNTIME_C`, `RUNTIME_GC_C`,
//! `RUNTIME_RACE_C`, and the Windows / netpoller translation units). Rust's own
//! `include_str!` dependency tracking is not reliably honoured across a CI
//! `target/` cache restore (restored artifacts can carry mtimes that make Cargo
//! treat the crate as fresh), so a change to a runtime `.c` / `.inc.c` / `.h`
//! file **alone** — without also touching a `.rs` file — could leave the old
//! runtime baked into the rebuilt compiler. That silently shipped a stale
//! runtime (e.g. the data-race detector's no-op slice-1 stubs instead of the
//! slice-2 engine, #678).
//!
//! Emitting an explicit `rerun-if-changed` for every embedded runtime file makes
//! Cargo rebuild this crate whenever any of them changes, so the embedded
//! runtime always matches the source tree.
fn main() {
    // Every file `src/lib.rs` pulls in with `include_str!` / `include_bytes!`.
    // Keep this list in sync with those macros.
    const EMBEDDED_RUNTIME_FILES: &[&str] = &[
        "src/zaprt.c",
        "src/zapgc.inc.c",
        "src/zaprace.inc.c",
        "src/zapblas.inc.c",
        "src/zap_ctx_win_x64.c",
        "src/zap_win_a3.c",
        "src/zap_np_iocp.c",
        "src/zap_np_iocp.h",
    ];
    for f in EMBEDDED_RUNTIME_FILES {
        println!("cargo:rerun-if-changed={f}");
    }
    // Re-run if the build script itself changes.
    println!("cargo:rerun-if-changed=build.rs");
}
