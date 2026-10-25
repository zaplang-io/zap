//! I#234 `zap test --doc` end-to-end tests: run the doctest runner over fixture
//! `.zap` files whose docstrings carry example blocks, and assert on the report
//! and exit code. Requires the `codegen` feature (each example JITs through the
//! REPL `Session`), so these run in the dev container under both collectors.

#![cfg(feature = "codegen")]

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/doctest_fixtures")
        .join(name)
}

/// Run `zap test --doc <args>` and return `(stdout, stderr, exit_code)`.
fn doctest(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_zap"))
        .arg("test")
        .arg("--doc")
        .args(args)
        .output()
        .expect("spawn zap test --doc");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn passing_examples_report_pass_and_exit_zero() {
    let path = fixture("pass.zap");
    let (stdout, stderr, code) = doctest(&[path.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    // stdout-match, auto-print, multi-line, compile-error and runtime-raise
    // examples all pass; the module docstring example counts too.
    assert!(stdout.contains("0 failed"), "stdout: {stdout}");
    assert!(stdout.contains("[PASS]"), "stdout: {stdout}");
    // Items are named by their qualified name + example index.
    assert!(stdout.contains("pass.greet #1"), "stdout: {stdout}");
    assert!(stdout.contains("pass.checked #1"), "stdout: {stdout}");
    assert!(stdout.contains("pass.checked #2"), "stdout: {stdout}");
}

#[test]
fn an_item_without_a_fenced_block_is_skipped() {
    let path = fixture("pass.zap");
    let (stdout, _stderr, _code) = doctest(&[path.to_str().unwrap()]);
    // `undocumented_example` has a docstring but no ```zap block, so it produces
    // no doctest result at all (not a pass, not a fail).
    assert!(
        !stdout.contains("undocumented_example"),
        "an item with no example should not appear: {stdout}"
    );
}

#[test]
fn a_wrong_example_fails_with_a_diff_and_nonzero_exit() {
    let path = fixture("fail.zap");
    let (stdout, stderr, code) = doctest(&[path.to_str().unwrap()]);
    assert_eq!(code, 1, "a failing doctest must exit nonzero: {stdout}");
    assert!(stdout.contains("[FAIL]"), "stdout: {stdout}");
    assert!(stdout.contains("output mismatch"), "stdout: {stdout}");
    // The diff names both the claimed and the actual output.
    assert!(stdout.contains("expected"), "stdout: {stdout}");
    assert!(stdout.contains("actual"), "stdout: {stdout}");
    // The module example (`print(1 + 1)` claiming `3`) and `wrong` both fail.
    assert!(stdout.contains("2 failed"), "stdout: {stdout}");
    let _ = stderr;
}

#[test]
fn json_report_is_well_formed() {
    let path = fixture("pass.zap");
    let (stdout, _stderr, code) = doctest(&["--json", path.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout: {stdout}");
    assert!(stdout.contains("\"ok\": true"), "stdout: {stdout}");
    assert!(stdout.contains("\"status\": \"pass\""), "stdout: {stdout}");
    assert!(stdout.contains("\"failed\": 0"), "stdout: {stdout}");
}

#[test]
fn json_report_marks_failures() {
    let path = fixture("fail.zap");
    let (stdout, _stderr, code) = doctest(&["--json", path.to_str().unwrap()]);
    assert_eq!(code, 1, "stdout: {stdout}");
    assert!(stdout.contains("\"ok\": false"), "stdout: {stdout}");
    assert!(stdout.contains("\"status\": \"fail\""), "stdout: {stdout}");
}
