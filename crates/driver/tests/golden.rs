//! Golden-test harness.
//!
//! Golden programs live in `<repo>/tests/golden/*.zap`, each paired with a
//! `*.expected` file holding the program's expected stdout. Until Phase 1
//! produces runnable binaries, the end-to-end run is `#[ignore]`d; the
//! fixtures-present check keeps the harness and fixtures honest today.

use std::path::PathBuf;
use std::process::Command;

/// `<repo>/tests/golden` — this crate lives at `<repo>/crates/driver`.
fn golden_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/driver has a two-level parent")
        .join("tests")
        .join("golden")
}

fn zap_fixtures() -> Vec<PathBuf> {
    let dir = golden_dir();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("golden dir should exist") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "zap") {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// Every `.zap` fixture has a matching `.expected` file. Runs today.
#[test]
fn fixtures_are_paired() {
    let fixtures = zap_fixtures();
    assert!(
        !fixtures.is_empty(),
        "expected at least one golden fixture in {}",
        golden_dir().display()
    );
    for zap in fixtures {
        let expected = zap.with_extension("expected");
        assert!(
            expected.exists(),
            "missing expected output for {}",
            zap.display()
        );
    }
}

/// Compile + run each fixture and compare stdout. Enabled once Phase 1 codegen
/// exists: `cargo test -- --ignored`.
#[test]
#[ignore = "enable once Phase 1 produces a runnable binary"]
fn golden_programs_match() {
    let bin = env!("CARGO_BIN_EXE_zapc");
    for zap in zap_fixtures() {
        let expected = std::fs::read_to_string(zap.with_extension("expected"))
            .expect("expected output present");
        let output = Command::new(bin)
            .arg(&zap)
            .output()
            .expect("run zapc on fixture");
        let got = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            got.trim_end(),
            expected.trim_end(),
            "stdout mismatch for {}",
            zap.display()
        );
    }
}
