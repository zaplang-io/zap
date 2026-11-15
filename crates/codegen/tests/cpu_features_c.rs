//! Compiles and runs the standalone C unit test for the `cpu_features` detector
//! (`zapcpu_test.c`, which `#define ZAP_CPU_TEST`s and includes `zapcpu.inc.c`),
//! so the pure detection helpers — x86 XCR0 gating, aarch64 HWCAP mapping, the
//! name tables, and the downgrade-only env overrides — are exercised with
//! synthetic inputs under the normal `cargo test` run (#943, epic #139, D-0192).
//!
//! The harness itself holds the assertions; this just builds it, runs it, and
//! fails the Rust test if the harness exits non-zero (printing its output).

use std::path::PathBuf;
use std::process::Command;

/// A C compiler to use: `$CC` if set, else the first of cc/clang/gcc that runs.
fn c_compiler() -> Option<String> {
    if let Ok(cc) = std::env::var("CC") {
        if !cc.trim().is_empty() {
            return Some(cc);
        }
    }
    for cand in ["cc", "clang", "gcc"] {
        if Command::new(cand)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Some(cand.to_string());
        }
    }
    None
}

#[test]
fn cpu_features_c_unit_test() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = manifest.join("tests").join("zapcpu_test.c");
    assert!(src.exists(), "missing C test harness: {}", src.display());

    let Some(cc) = c_compiler() else {
        // No C toolchain on this host (should not happen in CI, where the runtime
        // is compiled anyway). Skip rather than fail so a pure-Rust dev box works.
        eprintln!("cpu_features_c_unit_test: no C compiler found; skipping");
        return;
    };

    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let bin = out_dir.join("zapcpu_test_bin");

    let compile = Command::new(&cc)
        .args([
            "-DZAP_CPU_TEST",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-std=c11",
        ])
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn C compiler");
    assert!(
        compile.status.success(),
        "compiling zapcpu_test.c failed:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&bin)
        .output()
        .expect("failed to run compiled C test");
    print!("{}", String::from_utf8_lossy(&run.stdout));
    assert!(
        run.status.success(),
        "zapcpu_test reported failures:\n{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}
