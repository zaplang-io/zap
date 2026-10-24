//! I#159 `zap repl` end-to-end tests: drive scripted sessions through the real
//! `zap repl` subprocess over stdin and assert on the transcript. Requires the
//! `llvm`/`codegen` feature (the REPL JITs each entry), so these run in the dev
//! container under both collectors.

#![cfg(feature = "codegen")]

use std::io::Write;
use std::process::{Command, Stdio};

/// Run `zap repl`, feed `script` on stdin, and return `(stdout, stderr, code)`.
fn repl(script: &str) -> (String, String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_zap"))
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn zap repl");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn define_a_value_and_use_it_next_line() {
    let (stdout, _stderr, code) = repl("auto x = 41\nx + 1\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("42"), "stdout: {stdout:?}");
}

#[test]
fn shadow_redeclare_with_a_new_type() {
    // `x` is int64, then re-bound to a str; the later reference sees the newest.
    let (stdout, _stderr, code) = repl("auto x = 1\nauto x = \"shadowed\"\nx\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("shadowed"), "stdout: {stdout:?}");
}

#[test]
fn type_error_is_reported_and_the_session_survives() {
    // A bad entry is rejected (with a ZAPnnnn code); the next entry still runs.
    let (stdout, stderr, code) = repl("auto bad = 1 + \"nope\"\nprint(7)\n");
    assert_eq!(code, 0);
    assert!(stderr.contains("error[ZAP"), "stderr: {stderr:?}");
    assert!(stdout.contains('7'), "session should survive: {stdout:?}");
}

#[test]
fn runtime_exception_is_caught_and_the_session_survives() {
    let (stdout, stderr, code) = repl("raise \"boom\"\nprint(123)\n");
    assert_eq!(code, 0);
    assert!(
        stderr.contains("Uncaught exception: boom"),
        "stderr: {stderr:?}"
    );
    assert!(stdout.contains("123"), "session should survive: {stdout:?}");
}

#[test]
fn multi_line_block_then_use_the_result() {
    // A multi-line `for` block (closed by a blank line) mutates a prior binding;
    // the next entry reads the accumulated value.
    let (stdout, _stderr, code) = repl(
        "auto total = 0\nfor i in range(4):\n    total = total + i\n\ntotal\n",
    );
    assert_eq!(code, 0);
    assert!(stdout.contains('6'), "0+1+2+3 == 6; stdout: {stdout:?}");
}

#[test]
fn closure_captures_a_session_binding() {
    // A closure binding (explicit `fn` type so it persists) captures a prior
    // session binding, then is invoked from a later entry through its slot.
    let (stdout, _stderr, code) = repl(
        "auto base = 10\nfn(int64) -> int64 adder = lambda y: base + y\nprint(adder(5))\n",
    );
    assert_eq!(code, 0);
    assert!(stdout.contains("15"), "stdout: {stdout:?}");
}

#[test]
fn type_meta_command_reports_inferred_type() {
    let (stdout, _stderr, code) = repl("auto n = 3\n:type n + 1\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("int64"), "stdout: {stdout:?}");
}
