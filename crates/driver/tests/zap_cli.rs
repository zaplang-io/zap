//! End-to-end tests for the `zap` CLI binary.
//!
//! The parse-level behavior is unit-tested in `src/cli.rs`; here we drive the
//! real binary so dispatch, scaffolding, stubs, and (under the `llvm` feature)
//! the compile/run path are exercised as a user would hit them.

use std::path::{Path, PathBuf};
use std::process::Command;

fn zap() -> Command {
    Command::new(env!("CARGO_BIN_EXE_zap"))
}

/// A fresh, unique scratch directory under the test target dir.
fn scratch(tag: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).unwrap();
    base
}

#[test]
fn version_prints_the_toolchain_version() {
    let out = zap().arg("version").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("zap "), "got: {stdout}");
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")), "got: {stdout}");
}

#[test]
fn help_succeeds_and_lists_commands() {
    let out = zap().arg("help").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for verb in [
        "run", "build", "ir", "expand", "new", "init", "fmt", "test", "mcp",
    ] {
        assert!(stdout.contains(verb), "help missing `{verb}`:\n{stdout}");
    }
}

#[test]
fn no_subcommand_exits_2() {
    let out = zap().output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn unknown_subcommand_exits_2() {
    let out = zap().arg("frobnicate").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown subcommand"), "got: {stderr}");
}

#[test]
fn stubs_name_their_owning_tickets() {
    for (args, ticket) in [
        (vec!["add", "foo"], "#69"),
        (vec!["remove", "foo"], "#69"),
        (vec!["zenv", "create"], "#69"),
        (vec!["mcp"], "#72"),
    ] {
        let out = zap().args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "args: {args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(ticket),
            "args {args:?} should name {ticket}, got: {stderr}"
        );
    }
}

#[test]
fn test_with_no_tests_fails_cleanly() {
    // In an empty project `zap test` finds nothing to run. Without a backend it
    // reports that codegen is missing; with one it reports no test files. Either
    // way it is a clean, non-crashing failure (exit 1).
    let dir = scratch("empty_tests");
    let out = zap().arg("test").current_dir(&dir).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn fmt_reports_not_implemented() {
    let out = zap().arg("fmt").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("D-0037"), "got: {stderr}");
}

#[test]
fn new_scaffolds_a_project() {
    let dir = scratch("new");
    let proj = dir.join("widgets");
    let out = zap().arg("new").arg(&proj).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let manifest = std::fs::read_to_string(proj.join("zap.toml")).unwrap();
    assert!(manifest.contains("name = \"widgets\""), "got: {manifest}");
    assert!(manifest.contains("[dependencies]"));

    let main = std::fs::read_to_string(proj.join("src").join("main.zap")).unwrap();
    assert!(main.contains("def main"), "got: {main}");

    // A second `new` on the same path refuses to clobber.
    let again = zap().arg("new").arg(&proj).output().unwrap();
    assert_eq!(again.status.code(), Some(1));
}

#[test]
fn init_scaffolds_in_place() {
    let dir = scratch("init");
    let out = zap().arg("init").current_dir(&dir).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.join("zap.toml").exists());
    assert!(dir.join("src").join("main.zap").exists());

    // A second `init` refuses to overwrite the manifest.
    let again = zap().arg("init").current_dir(&dir).output().unwrap();
    assert_eq!(again.status.code(), Some(1));
}

#[test]
fn run_without_file_or_manifest_errors() {
    let dir = scratch("empty");
    let out = zap().arg("run").current_dir(&dir).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no input file"), "got: {stderr}");
}

/// `tests/golden` — this crate lives at `<repo>/crates/driver`.
fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("tests")
        .join("golden")
}

/// `zap ir` runs the front-end + lowering without LLVM.
#[test]
fn ir_dumps_without_llvm() {
    let out = zap()
        .arg("ir")
        .arg(golden_dir().join("fib.zap"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.stdout.is_empty(), "expected ZIR on stdout");
}

// --- LLVM-only: the real compile/run path. ---

#[cfg(feature = "llvm")]
#[test]
fn run_compiles_and_runs_a_file() {
    let out = zap()
        .arg("run")
        .arg(golden_dir().join("hello.zap"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim_end(), "Hello, Zap\ndone");
}

#[cfg(feature = "llvm")]
#[test]
fn build_produces_a_runnable_binary() {
    let dir = scratch("build");
    let exe = dir.join("hello");
    let out = zap()
        .arg("build")
        .arg(golden_dir().join("hello.zap"))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(exe.exists(), "no binary at {}", exe.display());

    let ran = Command::new(&exe).output().unwrap();
    assert!(ran.status.success());
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim_end(),
        "Hello, Zap\ndone"
    );
}

#[cfg(feature = "llvm")]
#[test]
fn run_in_project_mode_uses_src_main() {
    let dir = scratch("project");
    let proj = dir.join("app");
    assert!(zap()
        .arg("new")
        .arg(&proj)
        .output()
        .unwrap()
        .status
        .success());

    let out = zap().arg("run").current_dir(&proj).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end(),
        "Hello from app!"
    );
}

// --- `zap test` end-to-end (issue #73, D-0054) --------------------------------

/// Write `contents` to `dir/rel`, creating parent directories.
#[cfg(feature = "llvm")]
fn write_file(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A whole project — inline `@test` in `src/main.zap` plus a `tests/` file that
/// exercises hooks, `@parametrize`, `@skip` and `@expect_raises` — runs green.
#[cfg(feature = "llvm")]
#[test]
fn test_runs_a_project_green() {
    let dir = scratch("test_green");
    write_file(
        &dir,
        "src/main.zap",
        "@test\ndef test_inline():\n    assert 1 + 1 == 2\n\n\
         def main():\n    print(\"app\")\n",
    );
    write_file(
        &dir,
        "tests/math_test.zap",
        "@before\ndef setup():\n    print(\"setup ran\")\n\n\
         @test\ndef test_add():\n    assert 2 + 3 == 5\n\n\
         @test\n@parametrize(cases=[(1, 2, 3), (10, 20, 30)])\n\
         def test_param(int64 a, int64 b, int64 want):\n    assert a + b == want\n\n\
         @test\n@skip(\"not ready\")\ndef test_skipped():\n    assert False\n\n\
         @test\n@expect_raises(ValueError)\ndef test_raises():\n    raise \"boom\"\n",
    );

    let out = zap().arg("test").current_dir(&dir).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "zap test should pass; exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );
    assert!(stdout.contains("[PASS] test_inline"), "stdout:\n{stdout}");
    assert!(stdout.contains("[PASS] test_add"), "stdout:\n{stdout}");
    assert!(stdout.contains("[PASS] test_param[0]"), "stdout:\n{stdout}");
    assert!(stdout.contains("[PASS] test_param[1]"), "stdout:\n{stdout}");
    assert!(stdout.contains("[SKIP] test_skipped"), "stdout:\n{stdout}");
    assert!(stdout.contains("[PASS] test_raises"), "stdout:\n{stdout}");
    assert!(stdout.contains("0 failed"), "stdout:\n{stdout}");
}

/// A deliberately failing test shows power-assert operand values and fails the
/// run with a non-zero exit code.
#[cfg(feature = "llvm")]
#[test]
fn test_reports_power_assert_on_failure() {
    let dir = scratch("test_fail");
    write_file(
        &dir,
        "tests/bad_test.zap",
        "@test\ndef test_math():\n    int64 x = 2\n    assert x * 2 == 5\n",
    );

    let out = zap().arg("test").current_dir(&dir).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "stdout:\n{stdout}");
    assert!(stdout.contains("[FAIL] test_math"), "stdout:\n{stdout}");
    assert!(stdout.contains("assertion failed"), "stdout:\n{stdout}");
    assert!(stdout.contains("assert x * 2 == 5"), "stdout:\n{stdout}");
    assert!(stdout.contains("left:  4"), "stdout:\n{stdout}");
    assert!(stdout.contains("right: 5"), "stdout:\n{stdout}");
    assert!(stdout.contains("1 failed"), "stdout:\n{stdout}");
}

/// `zap test --json` emits structured results consumable by `zap mcp` (#72).
#[cfg(feature = "llvm")]
#[test]
fn test_json_output() {
    let dir = scratch("test_json");
    write_file(
        &dir,
        "tests/ok_test.zap",
        "@test\ndef test_ok():\n    assert True\n",
    );

    let out = zap()
        .arg("test")
        .arg("--json")
        .current_dir(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout:\n{stdout}");
    assert!(stdout.contains("\"status\": \"pass\""), "stdout:\n{stdout}");
    assert!(
        stdout.contains("\"name\": \"test_ok\""),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("\"ok\": true"), "stdout:\n{stdout}");
}
