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
    let (stdout, _stderr, code) =
        repl("auto total = 0\nfor i in range(4):\n    total = total + i\n\ntotal\n");
    assert_eq!(code, 0);
    assert!(stdout.contains('6'), "0+1+2+3 == 6; stdout: {stdout:?}");
}

#[test]
fn closure_captures_a_session_binding() {
    // A closure binding (explicit `fn` type so it persists) captures a prior
    // session binding, then is invoked from a later entry through its slot.
    let (stdout, _stderr, code) =
        repl("auto base = 10\nfn(int64) -> int64 adder = lambda y: base + y\nprint(adder(5))\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("15"), "stdout: {stdout:?}");
}

#[test]
fn type_meta_command_reports_inferred_type() {
    let (stdout, _stderr, code) = repl("auto n = 3\n:type n + 1\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("int64"), "stdout: {stdout:?}");
}

// ---------------------------------------------------------------------------
// I#252: cross-entry top-level declaration persistence (def/enum/type/interface)
// ---------------------------------------------------------------------------

#[test]
fn define_a_function_and_call_it_next_line() {
    // A `def` in one entry is callable in a later entry (the headline I#252 gap).
    let (stdout, _stderr, code) =
        repl("def double(int64 x) -> int64:\n    return x * 2\n\ndouble(21)\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("42"), "stdout: {stdout:?}");
}

#[test]
fn define_an_enum_and_a_function_over_it_across_entries() {
    // An `enum` defined in entry 1 and a `def` matching on it in entry 2 both
    // persist, and a call in entry 3 sees both.
    let script = "enum Color:\n    Red\n    Green\n\n\
                  def rank(Color c) -> int64:\n    match c:\n        case Color.Red:\n            return 0\n        case Color.Green:\n            return 1\n\n\
                  rank(Color.Green)\n";
    let (stdout, stderr, code) = repl(script);
    assert_eq!(code, 0, "stderr: {stderr:?}");
    assert_eq!(stdout.trim(), "1", "stdout: {stdout:?}");
}

#[test]
fn define_an_interface_and_struct_use_next_line() {
    // An `interface` and a `struct` that satisfies it persist; a later entry
    // constructs the struct and calls the method. (Explicit `print` — auto-print
    // of a bare `recv.method()` result is limited by the queryable analysis not
    // yet typing struct-method calls; `print` types through IR lowering.)
    let script = "interface Named:\n    def name(self) -> str\n\n\
                  struct Dog:\n    str n\n    def name(self) -> str:\n        return self.n\n\n\
                  print(Dog(\"Rex\").name())\n";
    let (stdout, stderr, code) = repl(script);
    assert_eq!(code, 0, "stderr: {stderr:?}");
    assert!(stdout.contains("Rex"), "stdout: {stdout:?}");
}

#[test]
fn redefining_a_function_uses_the_newest() {
    // A redefinition replaces the prior decl (newest wins), like `let` shadowing.
    let script = "def f() -> int64:\n    return 1\n\n\
                  def f() -> int64:\n    return 2\n\n\
                  f()\n";
    let (stdout, _stderr, code) = repl(script);
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "2", "stdout: {stdout:?}");
}

// ---------------------------------------------------------------------------
// I#252: float session bindings persisted across entries
// ---------------------------------------------------------------------------

#[test]
fn float_binding_persists_across_entries() {
    // A `float64` binding (inferred from a float literal) carries into a later
    // entry via its runtime slot.
    let (stdout, _stderr, code) = repl("auto half = 0.5\nhalf + 0.5\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("1.000000"), "stdout: {stdout:?}");
}

#[test]
fn float32_binding_persists_across_entries() {
    // An explicit `float32` binding round-trips through the shared slot.
    let (stdout, _stderr, code) = repl("float32 q = 1.5\nq + q\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("3.000000"), "stdout: {stdout:?}");
}

// ---------------------------------------------------------------------------
// I#252: user-type auto-print (minimal; the general repr protocol is a follow-up)
// ---------------------------------------------------------------------------

#[test]
fn struct_with_derived_repr_renders() {
    // A struct with a value renderer (`@derive(Repr)`) auto-prints via it.
    let script = "@derive(Repr)\nstruct Tagged:\n    int64 n\n\nTagged(7)\n";
    let (stdout, stderr, code) = repl(script);
    assert_eq!(code, 0, "stderr: {stderr:?}");
    assert!(stdout.contains("Tagged(n=7)"), "stdout: {stdout:?}");
}

#[test]
fn struct_without_renderer_prints_type_placeholder() {
    // A user value with no `__str__`/`__repr__`/`@derive(Repr)` falls back to a
    // `<TypeName>` placeholder instead of a confusing compile error.
    let script = "struct Bare:\n    int64 n\n\nBare(7)\n";
    let (stdout, stderr, code) = repl(script);
    assert_eq!(code, 0, "stderr: {stderr:?}");
    assert!(stdout.contains("<Bare>"), "stdout: {stdout:?}");
}

// ---------------------------------------------------------------------------
// I#252: line editing degrades cleanly under piped (non-interactive) stdin
// ---------------------------------------------------------------------------

#[test]
fn piped_stdin_uses_the_plain_reader_no_prompt() {
    // Piped stdin (the scripted-stdin harness) must not engage the line editor:
    // no prompt text leaks into stdout, and evaluation is byte-for-byte normal.
    let (stdout, _stderr, code) = repl("auto x = 40\nx + 2\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("42"), "stdout: {stdout:?}");
    assert!(!stdout.contains("zap>"), "no prompt on stdout: {stdout:?}");
}
