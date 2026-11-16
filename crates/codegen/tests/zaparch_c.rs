//! Compiles and runs the standalone C unit test for the `arch.*` intrinsic shims
//! and the `hash.crc32c` wrapper (`zaparch_test.c`, which includes
//! `zaparch.inc.c`), so the hardware CRC32C single-step intrinsics, popcount, and
//! the software wrapper's bit-identity with the raw intrinsic are exercised under
//! the normal `cargo test` run (#962, epic #139, D-0199).
//!
//! The harness holds the assertions; this builds it, runs it, and fails if it
//! exits non-zero. It needs only a C compiler (no LLVM), so it runs on any CI
//! host; the crc32c intrinsic checks inside are arch-gated to the build arch.

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
fn arch_c_unit_test() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = manifest.join("tests").join("zaparch_test.c");
    assert!(src.exists(), "missing C test harness: {}", src.display());

    let Some(cc) = c_compiler() else {
        eprintln!("arch_c_unit_test: no C compiler found; skipping");
        return;
    };

    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let bin = out_dir.join("zaparch_test_bin");

    let compile = Command::new(&cc)
        .args(["-O2", "-Wall", "-Wextra", "-Werror", "-std=c11"])
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn C compiler");
    assert!(
        compile.status.success(),
        "compiling zaparch_test.c failed:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&bin)
        .output()
        .expect("failed to run compiled C test");
    print!("{}", String::from_utf8_lossy(&run.stdout));
    assert!(
        run.status.success(),
        "zaparch_test reported failures:\n{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}
