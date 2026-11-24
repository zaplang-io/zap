//! End-to-end tests for the `zap` CLI binary.
//!
//! The parse-level behavior is unit-tested in `src/cli.rs`; here we drive the
//! real binary so dispatch, scaffolding, stubs, and (under the `llvm` feature)
//! the compile/run path are exercised as a user would hit them.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use zapc::json::{self, Json};

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

// --- package manager: `zap add` / `zap remove` (issue #69, D-0052) -----------

/// `zap add --path` edits `zap.toml` and writes a `zap.lock` with a checksum;
/// `zap remove` drops it again. Fully offline via a local `path` source.
#[test]
fn add_and_remove_a_path_dependency() {
    let dir = scratch("add_path");
    // A local dependency module with its own manifest.
    let dep = dir.join("mathx");
    std::fs::create_dir_all(dep.join("src")).unwrap();
    std::fs::write(
        dep.join("zap.toml"),
        "[package]\nname = \"mathx\"\nversion = \"0.2.0\"\n",
    )
    .unwrap();
    std::fs::write(dep.join("src").join("lib.zap"), "def area(): pass\n").unwrap();

    // A fresh project next to it.
    let app = dir.join("app");
    assert!(zap()
        .arg("new")
        .arg(&app)
        .output()
        .unwrap()
        .status
        .success());

    let out = zap()
        .args(["add", "mathx", "--path", "../mathx"])
        .current_dir(&app)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let manifest = std::fs::read_to_string(app.join("zap.toml")).unwrap();
    assert!(
        manifest.contains("mathx = { path = \"../mathx\" }"),
        "manifest:\n{manifest}"
    );
    let lock = std::fs::read_to_string(app.join("zap.lock")).unwrap();
    assert!(lock.contains("name = \"mathx\""), "lock:\n{lock}");
    assert!(lock.contains("version = \"0.2.0\""), "lock:\n{lock}");
    assert!(
        lock.contains("zap1:"),
        "lock should carry a checksum:\n{lock}"
    );

    // Remove it and re-lock.
    let out = zap()
        .args(["remove", "mathx"])
        .current_dir(&app)
        .output()
        .unwrap();
    assert!(out.status.success());
    let manifest = std::fs::read_to_string(app.join("zap.toml")).unwrap();
    assert!(!manifest.contains("mathx"), "manifest:\n{manifest}");
    let lock = std::fs::read_to_string(app.join("zap.lock")).unwrap();
    assert!(!lock.contains("name = \"mathx\""), "lock:\n{lock}");
}

#[test]
fn add_without_a_manifest_fails_cleanly() {
    let dir = scratch("add_nomani");
    let out = zap()
        .args(["add", "x", "--path", "y"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("zap init"), "got: {stderr}");
}

// --- `zenv` activate/deactivate flips module resolution (issue #69) -----------

/// `zap zenv create` writes an `activate` script and a `pkg/` cache;
/// `zap zenv path` reports a *different* resolution path when `ZAP_ZENV` is set
/// (as `source zenv/activate` would), demonstrating the activation flip.
#[test]
fn zenv_create_and_resolution_flip() {
    let dir = scratch("zenv");
    let app = dir.join("app");
    assert!(zap()
        .arg("new")
        .arg(&app)
        .output()
        .unwrap()
        .status
        .success());

    // Create the environment (skip the Python venv to stay fast/hermetic).
    let out = zap()
        .args(["zenv", "create", "--no-venv"])
        .current_dir(&app)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let activate = std::fs::read_to_string(app.join("zenv").join("activate")).unwrap();
    assert!(
        activate.contains("export ZAP_ZENV="),
        "activate:\n{activate}"
    );
    assert!(activate.contains("VIRTUAL_ENV"), "activate:\n{activate}");
    assert!(activate.contains("deactivate()"), "activate:\n{activate}");
    assert!(app.join("zenv").join("pkg").is_dir());

    // Deactivated: resolution uses the global cache (not under the project).
    let deactivated = zap()
        .args(["zenv", "path"])
        .current_dir(&app)
        .env_remove("ZAP_ZENV")
        .env("ZAP_CACHE", "/global/cache")
        .output()
        .unwrap();
    let deactivated = String::from_utf8_lossy(&deactivated.stdout);
    assert_eq!(deactivated.trim(), "/global/cache");

    // Activated: resolution flips to the project-local zenv cache.
    let activated = zap()
        .args(["zenv", "path"])
        .current_dir(&app)
        .env("ZAP_ZENV", "/proj/zenv")
        .env("ZAP_CACHE", "/global/cache")
        .output()
        .unwrap();
    let activated = String::from_utf8_lossy(&activated.stdout);
    assert_eq!(activated.trim(), "/proj/zenv/pkg");
    assert_ne!(deactivated.trim(), activated.trim());
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
fn fmt_formats_stdin_to_stdout() {
    let mut child = zap()
        .arg("fmt")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"x=1+2\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "x = 1 + 2\n");
}

#[test]
fn fmt_check_reports_unformatted() {
    // Unformatted stdin under --check exits non-zero and writes nothing to stdout.
    let mut child = zap()
        .arg("fmt")
        .arg("--check")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"x=1\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        out.stdout.is_empty(),
        "stdout should be empty under --check"
    );
}

#[test]
fn fmt_check_passes_formatted_file() {
    let dir = scratch("fmt");
    let f = dir.join("ok.zap");
    std::fs::write(&f, "x = 1\n").unwrap();
    let out = zap().arg("fmt").arg("--check").arg(&f).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn fmt_rewrites_file_in_place() {
    let dir = scratch("fmt");
    let f = dir.join("messy.zap");
    std::fs::write(&f, "x=1\n").unwrap();
    let out = zap().arg("fmt").arg(&f).output().unwrap();
    assert!(out.status.success());
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "x = 1\n");
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

#[cfg(feature = "codegen")]
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

#[cfg(feature = "codegen")]
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

#[cfg(feature = "codegen")]
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
#[cfg(feature = "codegen")]
fn write_file(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A whole project — inline `@test` in `src/main.zap` plus a `tests/` file that
/// exercises hooks, `@parametrize`, `@skip` and `@expect_raises` — runs green.
#[cfg(feature = "codegen")]
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
         @test\n@expect_raises(ValueError)\ndef test_raises():\n    raise ValueError(\"boom\")\n",
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
#[cfg(feature = "codegen")]
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

/// `assert` on a nullable reference struct field compared with `None` — the
/// #1184 reproducer — runs green under `zap test`, both while the field holds
/// its default `None` and after it is assigned a struct. Pins the test-mode
/// path (the issue reproduced under `zap test … --json`), complementing the
/// `assert_none_field` `zap run` golden.
#[cfg(feature = "codegen")]
#[test]
fn test_assert_nullable_field_vs_none_1184() {
    let dir = scratch("test_assert_none_1184");
    write_file(
        &dir,
        "tests/reader_test.zap",
        "struct Reader:\n    bytes pending\n\n\
         struct Client:\n    Reader reader = None\n\n\
         @test\ndef test_empty_reader_is_none():\n    auto c = Client()\n    assert c.reader == None\n\n\
         @test\ndef test_assigned_reader_is_not_none():\n    auto c = Client()\n\
         \x20   c.reader = Reader(pending=b\"x\")\n    assert c.reader != None\n",
    );

    let out = zap().arg("test").current_dir(&dir).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "zap test should pass; exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        stdout.contains("[PASS] test_empty_reader_is_none"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("[PASS] test_assigned_reader_is_not_none"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("0 failed"), "stdout:\n{stdout}");
}

/// `Optional.Some` built from an explicit integer conversion — the #1308
/// reproducer — runs green under `zap test`, in both the standalone and the
/// typed keyword-argument forms. Before the fix the native harness lowered its
/// transformed module with an empty `TypeMap`, so the monomorphizer could not
/// pin `T` and the run failed with `ZAP6001` ("cannot infer type argument 'T'
/// of generic 'Optional'") even though the identical program compiled under
/// `zap run`. Pins the test-mode path (the sema `TypeMap` is now threaded into
/// the harness build); the `harness_infers_*` host tests pin the mechanism.
#[cfg(feature = "codegen")]
#[test]
fn test_optional_some_explicit_cast_1308() {
    let dir = scratch("test_optional_cast_1308");
    write_file(
        &dir,
        "tests/optional_cast_test.zap",
        "@test\ndef explicit_integer_cast():\n\
         \x20   auto value = Optional.Some(int64(0))\n    assert value.is_some()\n\n\
         @test\ndef explicit_integer_cast_kwarg():\n\
         \x20   auto value = Optional.Some(value=int64(0))\n    assert value.is_some()\n\n\
         @test\ndef zero_is_present():\n\
         \x20   Optional[int64] value = Optional.Some(int64(0))\n\
         \x20   assert value.unwrap() == 0\n",
    );

    let out = zap().arg("test").current_dir(&dir).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "zap test should pass; exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        stdout.contains("[PASS] explicit_integer_cast"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("[PASS] explicit_integer_cast_kwarg"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("[PASS] zero_is_present"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("0 failed"), "stdout:\n{stdout}");
}

/// A generic reflected operator whose operand type sema inferred — the #1310
/// reproducer — runs green under `zap test`. The native harness used to pass an
/// empty `TypeMap` to codegen, so a `__radd__[T]` over a built-in / seam result
/// fell back to pre-#1167 inference and failed with `ZAP6001` ("cannot infer
/// type argument 'T' of operator method '__radd__'"), though `zap ir` compiled
/// the same body. Covers a built-in result (`len`) and an inline `datetime`
/// seam call with the annotation omitted, exercising reflected `+` and `-` with
/// a core Date operand.
#[cfg(feature = "codegen")]
#[test]
fn test_reflected_operator_sema_inference_1310() {
    let dir = scratch("test_reflected_op_1310");
    write_file(
        &dir,
        "tests/reflected_test.zap",
        "import datetime\n\n\
         value struct Delta:\n    int64 n\n\n\
         \x20   def __radd__[T](self, T x) -> T:\n        return x\n\n\
         \x20   def __rsub__[T](self, T x) -> T:\n        return x\n\n\
         @test\ndef inferred_from_builtin_result():\n\
         \x20   list[int64] xs = [1, 2, 3]\n\
         \x20   auto result = len(xs) + Delta(1)\n    assert result == 3\n\n\
         @test\ndef inferred_date_operand():\n\
         \x20   auto day = datetime.date(2024, 2, 29)\n\
         \x20   auto result = day + Delta(1)\n    assert result.day == 29\n\n\
         @test\ndef inferred_date_operand_sub():\n\
         \x20   auto day = datetime.date(2024, 2, 29)\n\
         \x20   auto result = day - Delta(1)\n    assert result.day == 29\n",
    );

    let out = zap().arg("test").current_dir(&dir).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "zap test should pass; exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        stdout.contains("[PASS] inferred_from_builtin_result"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("[PASS] inferred_date_operand"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("[PASS] inferred_date_operand_sub"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("0 failed"), "stdout:\n{stdout}");
}

/// Iterating a `bytes` value in a `for` loop — the #1349 reproducer — runs
/// green under `zap test`. The issue surfaced on the native test path (the
/// datetime-utils parser iterating a bytes token in a `@test`), where the
/// harness lowers with an empty `TypeMap`, so the loop variable's `int64`
/// element type must come from the monomorphizer's own inferer (cf. #1308).
/// Covers the issue's `b"UTC"` sum, unsigned 0x00/0xFF (never sign-extended
/// negative), empty bytes (zero iterations), and the loop variable used as an
/// int. Complements the `tests/golden/bytes_for_loop.zap` `zap run` golden.
#[cfg(feature = "codegen")]
#[test]
fn test_bytes_iteration_1349() {
    let dir = scratch("test_bytes_iter_1349");
    write_file(
        &dir,
        "tests/bytes_iter_test.zap",
        "@test\ndef sums_utc_bytes():\n\
         \x20   auto data = b\"UTC\"\n    int64 total = 0\n\
         \x20   for byte in data:\n        total += byte\n\
         \x20   assert total == 236\n\n\
         @test\ndef high_bytes_are_unsigned():\n\
         \x20   auto edges = bytes([0, 127, 128, 255])\n\
         \x20   list[int64] seen = []\n\
         \x20   for b in edges:\n        seen.append(b)\n\
         \x20   assert seen == [0, 127, 128, 255]\n\n\
         @test\ndef empty_bytes_zero_iterations():\n\
         \x20   int64 n = 0\n\
         \x20   for b in bytes():\n        n += 1\n\
         \x20   assert n == 0\n\n\
         @test\ndef loop_variable_is_an_int():\n\
         \x20   int64 first = -1\n\
         \x20   for b in b\"A\":\n        first = b + 1\n\
         \x20   assert first == 66\n",
    );

    let out = zap().arg("test").current_dir(&dir).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "zap test should pass; exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        stdout.contains("[PASS] sums_utc_bytes"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("[PASS] high_bytes_are_unsigned"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("[PASS] empty_bytes_zero_iterations"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("[PASS] loop_variable_is_an_int"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("0 failed"), "stdout:\n{stdout}");
}

/// Operations that take a `bytes` value directly — the #1355 reproducer — run
/// green under `zap test`: `sum(bytes)` (+ start), `sorted(bytes)` (ascending
/// list[int64], `reverse=`), and `x in bytes` membership (int byte-value and
/// bytes subsequence, with `not in`). These type-checked but failed code
/// generation before this change. Complements the `tests/golden/bytes_value_ops.zap`
/// `zap run` golden; exercises the native test path (empty `TypeMap`).
#[cfg(feature = "codegen")]
#[test]
fn test_bytes_value_ops_1355() {
    let dir = scratch("test_bytes_value_ops_1355");
    write_file(
        &dir,
        "tests/bytes_ops_test.zap",
        "@test\ndef sums_bytes():\n\
         \x20   assert sum(b\"UTC\") == 236\n\
         \x20   assert sum(b\"UTC\", 4) == 240\n\
         \x20   assert sum(bytes()) == 0\n\n\
         @test\ndef sorts_bytes():\n\
         \x20   assert sorted(b\"cab\") == [97, 98, 99]\n\
         \x20   assert sorted(b\"cab\", reverse=True) == [99, 98, 97]\n\
         \x20   assert sorted(bytes()) == []\n\n\
         @test\ndef int_membership():\n\
         \x20   assert 85 in b\"UTC\"\n\
         \x20   assert 0 not in b\"UTC\"\n\
         \x20   assert 255 in bytes([0, 255])\n\n\
         @test\ndef bytes_membership():\n\
         \x20   assert b\"TC\" in b\"UTC\"\n\
         \x20   assert b\"\" in b\"UTC\"\n\
         \x20   assert b\"xy\" not in b\"UTC\"\n",
    );

    let out = zap().arg("test").current_dir(&dir).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "zap test should pass; exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );
    assert!(stdout.contains("[PASS] sums_bytes"), "stdout:\n{stdout}");
    assert!(stdout.contains("[PASS] sorts_bytes"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("[PASS] int_membership"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("[PASS] bytes_membership"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("0 failed"), "stdout:\n{stdout}");
}

// --- `zap mcp` end-to-end (issue #72, D-0055) ---------------------------------

/// Drive the real `zap mcp` server over stdio: spawn it rooted at `root`, feed
/// one JSON-RPC request per line, close stdin, and parse each response line.
fn mcp_session(root: &Path, requests: &[&str]) -> Vec<Json> {
    let mut child = zap()
        .arg("mcp")
        .arg("--root")
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn zap mcp");
    {
        let mut stdin = child.stdin.take().unwrap();
        for r in requests {
            writeln!(stdin, "{r}").unwrap();
        }
        // `stdin` drops here, signaling EOF so the server exits.
    }
    let out = child.wait_with_output().expect("wait for zap mcp");
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| json::parse(l).expect("each response line is valid JSON"))
        .collect()
}

/// The text of a `tools/call` result's first content block.
fn tool_text(resp: &Json) -> String {
    resp.get("result")
        .unwrap()
        .get("content")
        .unwrap()
        .as_array()
        .unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

/// One session exercises three host-safe surfaces: language reference, a
/// fast check of a known-bad snippet, and a code-graph query over the project
/// the server is rooted at.
#[test]
fn mcp_reference_check_and_graph_over_stdio() {
    let dir = scratch("mcp_e2e");
    std::fs::write(
        dir.join("lib.zap"),
        "def greet(str name) -> str:\n    return name\n\nstruct Box:\n    int64 v\n",
    )
    .unwrap();

    let responses = mcp_session(
        &dir,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"zap_reference","arguments":{"name":"grammar"}}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"zap_check","arguments":{"code":"def main():\n    missing_fn()\n"}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"zap_code_graph","arguments":{"name":"greet"}}}"#,
        ],
    );
    assert_eq!(responses.len(), 4, "got: {responses:?}");

    // initialize
    assert_eq!(
        responses[0]
            .get("result")
            .unwrap()
            .get("serverInfo")
            .unwrap()
            .get("name")
            .unwrap()
            .as_str(),
        Some("zap-mcp")
    );

    // reference: the grammar body
    assert!(tool_text(&responses[1]).contains("EBNF"));

    // fast check: a bad snippet reports not-ok with diagnostics
    let check = json::parse(&tool_text(&responses[2])).unwrap();
    assert_eq!(check.get("ok"), Some(&Json::Bool(false)));
    assert!(!check
        .get("diagnostics")
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());

    // code graph: finds the function we defined
    let graph = json::parse(&tool_text(&responses[3])).unwrap();
    assert!(graph.get("matched").unwrap().as_i64().unwrap() >= 1);
    assert!(tool_text(&responses[3]).contains("greet"));
}

/// The targeted-test surface returns the runner's structured JSON (LLVM only).
#[cfg(feature = "codegen")]
#[test]
fn mcp_zap_test_returns_structured_results() {
    let dir = scratch("mcp_test");
    std::fs::write(
        dir.join("ok_test.zap"),
        "@test\ndef test_ok():\n    assert True\n",
    )
    .unwrap();

    let responses = mcp_session(
        &dir,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"zap_test","arguments":{"paths":["ok_test.zap"]}}}"#,
        ],
    );
    let text = tool_text(&responses[0]);
    assert!(text.contains("\"status\": \"pass\""), "got: {text}");
    assert!(text.contains("\"name\": \"test_ok\""), "got: {text}");
    assert!(text.contains("\"ok\": true"), "got: {text}");
}

/// `zap test --json` emits structured results consumable by `zap mcp` (#72).
#[cfg(feature = "codegen")]
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

// --- registry publish & discovery: `zap publish`/`search`/`info` (I#237) -----

/// A `file://` URL for a local path (absolute), matching the index base shape a
/// `[registries]` entry / `--registry` expects.
fn file_url(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    match p.strip_prefix('/') {
        Some(rest) => format!("file:///{rest}"),
        None => format!("file:///{p}"),
    }
}

/// Write a minimal publishable package project.
fn package_project(root: &Path, name: &str, version: &str) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("zap.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n"),
    )
    .unwrap();
    std::fs::write(
        root.join("src").join("lib.zap"),
        format!("# {name} {version}\ndef f(): pass\n"),
    )
    .unwrap();
}

/// End-to-end, offline: publish a package to a local `file://` static index,
/// discover it with `search`/`info`, then resolve + materialize it into a
/// consumer's zenv — closing the publish -> discover -> import loop through the
/// real binary, with no network and no hosting.
#[test]
fn publish_then_search_info_then_resolve_end_to_end() {
    let dir = scratch("registry_e2e");
    let index = dir.join("index");
    let cache = dir.join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    let index_url = file_url(&index);

    // 1. Publish `widgets` 1.0.0 to the local index.
    let widgets = dir.join("widgets");
    package_project(&widgets, "widgets", "1.0.0");
    let out = zap()
        .args(["publish", "--registry", &index_url])
        .current_dir(&widgets)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "publish failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("published widgets 1.0.0"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("zap1:"), "stdout:\n{stdout}");
    assert!(index.join("config.json").exists(), "index config written");

    // Re-publishing the same version is refused (immutability, ZAP6106).
    let dup = zap()
        .args(["publish", "--registry", &index_url])
        .current_dir(&widgets)
        .output()
        .unwrap();
    assert!(!dup.status.success(), "re-publish must fail");
    assert!(
        String::from_utf8_lossy(&dup.stderr).contains("ZAP6106"),
        "stderr:\n{}",
        String::from_utf8_lossy(&dup.stderr)
    );

    // Publish a second version so discovery has a range to report.
    package_project(&widgets, "widgets", "1.1.0");
    assert!(zap()
        .args(["publish", "--registry", &index_url])
        .current_dir(&widgets)
        .output()
        .unwrap()
        .status
        .success());

    // 2. `zap search` lists the package from the local index.
    let search = zap()
        .args(["search", "widg", "--registry", &index_url])
        .output()
        .unwrap();
    assert!(search.status.success());
    let s = String::from_utf8_lossy(&search.stdout);
    assert!(s.contains("widgets"), "search stdout:\n{s}");
    assert!(s.contains("1.1.0"), "search should report latest:\n{s}");

    // 3. `zap info` shows both published versions.
    let info = zap()
        .args(["info", "widgets", "--registry", &index_url])
        .output()
        .unwrap();
    assert!(info.status.success());
    let i = String::from_utf8_lossy(&info.stdout);
    assert!(
        i.contains("1.0.0") && i.contains("1.1.0"),
        "info stdout:\n{i}"
    );
    assert!(i.contains("latest: 1.1.0"), "info stdout:\n{i}");

    // 4. A consumer resolves + materializes the published package (import loop).
    let app = dir.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("zap.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
             [registries]\ntest = {{ index = \"{index_url}\" }}\n\n\
             [dependencies]\n\
             w = {{ registry = \"widgets\", version = \"1.0.0\", index = \"test\" }}\n"
        ),
    )
    .unwrap();
    std::fs::write(app.join("src").join("main.zap"), "def main(): pass\n").unwrap();

    let zenv = zap()
        .args(["zenv", "create", "--no-venv"])
        .current_dir(&app)
        .env("ZAP_CACHE", &cache)
        .output()
        .unwrap();
    assert!(
        zenv.status.success(),
        "zenv create failed: {}",
        String::from_utf8_lossy(&zenv.stderr)
    );
    // The published tree is downloaded, verified, and materialized under zenv.
    assert!(
        app.join("zenv/pkg/widgets/src/lib.zap").exists(),
        "widgets not materialized into the consumer's zenv"
    );
    let lock = std::fs::read_to_string(app.join("zap.lock")).unwrap();
    assert!(lock.contains("registry+test#widgets"), "lock:\n{lock}");
    assert!(lock.contains("version = \"1.0.0\""), "lock:\n{lock}");
}

/// Publishing to an `https://` index surfaces the hosting gap (ZAP6108) rather
/// than inventing a service.
#[test]
fn publish_to_https_index_is_gated() {
    let dir = scratch("registry_gated");
    let widgets = dir.join("widgets");
    package_project(&widgets, "widgets", "1.0.0");
    let out = zap()
        .args(["publish", "--registry", "https://index.zaplang.org/"])
        .current_dir(&widgets)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("ZAP6108"),
        "stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// --- line-oriented input: the `input([prompt])` builtin (#969, D-0212) -------
//
// input() reads one line from stdin, so it needs a piped child stdin that the
// golden/conformance harnesses do not provide. These drive the real binary.

/// Run `zap run <prog>` feeding `stdin_bytes`, returning (status-ok, stdout).
#[cfg(feature = "codegen")]
fn run_with_stdin(prog: &std::path::Path, stdin_bytes: &[u8]) -> (bool, String, String) {
    let mut child = zap()
        .arg("run")
        .arg(prog)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin_bytes).unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The prompt is written to stdout (no trailing newline), then one line is read;
/// the returned string excludes the trailing newline. A second `input()` with no
/// prompt reads the next line. CRLF is also trimmed.
#[cfg(feature = "codegen")]
#[test]
fn input_reads_lines_from_stdin_with_a_prompt() {
    let dir = scratch("input_prompt");
    let prog = dir.join("p.zap");
    std::fs::write(
        &prog,
        "def main():\n    \
         auto name = input(\"Name: \")\n    \
         auto rest = input()\n    \
         print(\"hi \" + name)\n    \
         print(\"len=\" + str(len(rest)))\n",
    )
    .unwrap();
    // Second line arrives as CRLF to prove the '\r' is stripped (len 3, "bar").
    let (ok, stdout, stderr) = run_with_stdin(&prog, b"Alice\nbar\r\n");
    assert!(ok, "stderr:\n{stderr}");
    assert_eq!(stdout, "Name: hi Alice\nlen=3\n");
}

/// At end-of-input with no bytes read, `input()` raises `EOFError` (#1036,
/// D-0215), matching CPython. Uncaught, it aborts with a non-zero status and
/// names the condition in the message.
#[cfg(feature = "codegen")]
#[test]
fn input_at_eof_raises() {
    let dir = scratch("input_eof");
    let prog = dir.join("p.zap");
    std::fs::write(&prog, "def main():\n    print(input())\n").unwrap();
    let (ok, _stdout, stderr) = run_with_stdin(&prog, b"");
    assert!(!ok, "expected a non-zero exit at EOF");
    assert!(stderr.contains("EOF"), "stderr:\n{stderr}");
}

/// The EOF error is a catchable `EOFError`, so a program can recover from it.
#[cfg(feature = "codegen")]
#[test]
fn input_eof_is_catchable() {
    let dir = scratch("input_eof_catch");
    let prog = dir.join("p.zap");
    std::fs::write(
        &prog,
        "def main():\n    \
         try:\n        \
         print(input())\n    \
         except EOFError:\n        \
         print(\"eof-caught\")\n",
    )
    .unwrap();
    let (ok, stdout, stderr) = run_with_stdin(&prog, b"");
    assert!(ok, "stderr:\n{stderr}");
    assert_eq!(stdout.trim_end(), "eof-caught");
}

// --- sys standard streams: sys.stdin() reader + sys.stderr() (#1035, D-0217) -
//
// stdin reading needs a piped child stdin; stderr needs a separate capture —
// neither is available to the golden harness, so these drive the real binary.

/// `sys.stdin()` is a buffered reader over fd 0: `io.readline` consumes it line
/// by line, exactly like a reader from `io.reader`.
#[cfg(feature = "codegen")]
#[test]
fn sys_stdin_reads_lines() {
    let dir = scratch("sys_stdin");
    let prog = dir.join("p.zap");
    std::fs::write(
        &prog,
        "import sys\nimport io\ndef main():\n    \
         auto r = sys.stdin()\n    \
         auto a = io.readline(r)\n    \
         auto b = io.readline(r)\n    \
         print(\"[\" + a.removesuffix(\"\\n\") + \"][\" + b.removesuffix(\"\\n\") + \"]\")\n",
    )
    .unwrap();
    let (ok, stdout, stderr) = run_with_stdin(&prog, b"foo\nbar\n");
    assert!(ok, "stderr:\n{stderr}");
    assert_eq!(stdout.trim_end(), "[foo][bar]");
}

/// `sys.stdout()` / `sys.stderr()` are writer streams over fds 1 / 2:
/// `sys.write` routes to the right descriptor.
#[cfg(feature = "codegen")]
#[test]
fn sys_stdout_stderr_route_to_their_fds() {
    let dir = scratch("sys_streams");
    let prog = dir.join("p.zap");
    std::fs::write(
        &prog,
        "import sys\ndef main():\n    \
         sys.write(sys.stdout(), \"to-out\\n\")\n    \
         sys.write(sys.stderr(), \"to-err\\n\")\n",
    )
    .unwrap();
    let (ok, stdout, stderr) = run_with_stdin(&prog, b"");
    assert!(ok, "stderr:\n{stderr}");
    assert_eq!(stdout, "to-out\n");
    assert!(stderr.contains("to-err"), "stderr:\n{stderr}");
}

// --- comptime surface: a derive generator can guard with `.kind` and abort via
// `comptime.error` (#1330). `zap ir` runs the full front-end + mono on the host,
// so the compile-time abort is observable without LLVM. ---

/// A library `Ser` generator that aborts on a nested-struct field and otherwise
/// prints each field, plus the `Ser` interface it satisfies.
const SER_LIB: &str = "\
@derivable(Ser)
def _derive_ser[T](T value):
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == \"struct\":
            comptime.error(f.span, \"Ser: nested struct field unsupported\")
        print(reflect.field(value, comptime f.index))

interface Ser:
    def ser(self)
";

fn write_zap(tag: &str, src: &str) -> PathBuf {
    let dir = scratch(tag);
    let f = dir.join("prog.zap");
    std::fs::write(&f, src).unwrap();
    f
}

#[test]
fn comptime_error_in_a_derive_generator_aborts_compilation() {
    // `Outer` has a nested-struct field (`Inner`), so the guard fires and the
    // generator aborts with the author's message — a clean compile error from
    // `comptime.error`, not a `comptime`-unknown-name error and not a panic.
    let src = format!(
        "{SER_LIB}\
@derive(Ser)
struct Inner:
    int64 x
@derive(Ser)
struct Outer:
    int64 a
    Inner b

def main():
    auto o = Outer(1, Inner(2))
    o.ser()
"
    );
    let f = write_zap("comptime-abort", &src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(!out.status.success(), "expected a compile abort");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Ser: nested struct field unsupported"),
        "author's message should surface; got: {stderr}"
    );
    assert!(
        !stderr.contains("unknown name 'comptime'")
            && !stderr.contains("unknown variable 'comptime'"),
        "comptime.error must be wired through sema+mono, not an unknown-name error: {stderr}"
    );
}

#[test]
fn a_derive_generator_compiles_for_a_supported_type() {
    // `Flat` has only primitive fields, so the `.kind == struct` guard prunes
    // away for every field and the generator compiles cleanly (the guarded
    // `comptime.error` is in a dead, comptime-false branch).
    let src = format!(
        "{SER_LIB}\
@derive(Ser)
struct Flat:
    int64 a
    int64 b

def main():
    auto x = Flat(10, 20)
    x.ser()
"
    );
    let f = write_zap("comptime-ok", &src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(
        out.status.success(),
        "a supported type should compile; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.stdout.is_empty(), "expected ZIR on stdout");
}

// --- E3d (#1293): a @derive that cannot apply is an average-programmer-grade
// compile error pointing at the user's field, not an internal symbol. ---

#[test]
fn an_inapplicable_derive_is_a_consumer_zap7003_at_the_users_field() {
    // `User` derives `Ser`, but its `Widget` field is a struct the generator
    // rejects via `comptime.error`. The error must name the user's type and
    // point at the user's field `w` — never the generator symbol or evaluator
    // internals — and carry the `required because of …` frame and the library's
    // own message.
    let src = "\
@derivable(Ser)
def _derive_ser[T](T value):
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == \"struct\":
            comptime.error(f.span, \"`Ser` is not implemented for this field's type\")

interface Ser:
    def ser(self)

@derive(Ser)
struct Widget:
    int64 w
@derive(Ser)
struct User:
    int64 id
    Widget w

def main():
    auto u = User(1, Widget(2))
    u.ser()
";
    let f = write_zap("e3d-inapplicable", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(!out.status.success(), "the derive must fail to compile");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error[ZAP7003]: cannot derive `Ser` for `User`"),
        "consumer header naming the user's type: {stderr}"
    );
    assert!(
        stderr.contains("required because of field `w` of `User`"),
        "backtrace frame at the user's field: {stderr}"
    );
    assert!(
        stderr.contains("the generator reported: `Ser` is not implemented"),
        "the library's own message as the root cause: {stderr}"
    );
    // The diagnostic must NOT leak the generator symbol or a mangled internal
    // name (the `$`-mangled specialization, e.g. `_derive_ser$User`).
    assert!(
        !stderr.contains("_derive_ser") && !stderr.contains('$'),
        "no comptime-internal symbol in the diagnostic: {stderr}"
    );
}

#[test]
fn a_derive_can_point_at_a_nested_element_use_site() {
    // #1346: a nested type carries its own *use-site* span, so a generator can
    // reject a list *element* (not just the whole field) via
    // `comptime.error(f.ty.elem.span, …)`. The primary error must underline the
    // element `Widget` inside `list[Widget] items` — a strictly deeper span than
    // the field — while the `required because of …` frame still attributes to the
    // enclosing field `items`. One diagnostic, two distinct spans on one line.
    let src = "\
@derivable(Ser)
def _derive_ser[T](T value):
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == \"list\":
            if f.ty.elem.kind.name == \"struct\":
                comptime.error(f.ty.elem.span, \"`Ser`: list element type is not serializable\")

interface Ser:
    def ser(self)

@derive(Ser)
struct Widget:
    int64 w
@derive(Ser)
struct Bag:
    list[Widget] items

def main():
    auto b = Bag([Widget(1)])
    b.ser()
";
    let f = write_zap("e3d-nested-element", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(!out.status.success(), "the derive must fail to compile");
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Consumer header + the library's own message about the element.
    assert!(
        stderr.contains("error[ZAP7003]: cannot derive `Ser` for `Bag`"),
        "consumer header naming the user's type: {stderr}"
    );
    assert!(
        stderr.contains("the generator reported: `Ser`: list element type is not serializable"),
        "the library's own message as the root cause: {stderr}"
    );
    // The chain still attributes to the enclosing field (mono matches the field
    // whose declaration contains the nested cause span, #1346).
    assert!(
        stderr.contains("required because of field `items` of `Bag`"),
        "backtrace frame at the enclosing field: {stderr}"
    );
    // Precision: the primary span is the element `Widget` (col 10 of the
    // `    list[Widget] items` line), strictly deeper than the field frame,
    // which points at the field start (col 5). Two distinct spans prove the
    // nested use-site is live, not a fallback to the whole field.
    assert!(
        stderr.contains(":16:10: error[ZAP7003]"),
        "primary error must underline the element use-site `Widget`: {stderr}"
    );
    assert!(
        stderr.contains(":16:5: note: required because of field `items`"),
        "the field frame points at the field declaration start: {stderr}"
    );
    // The caret underlines exactly `Widget` (6 columns), not the whole field.
    assert!(
        stderr.contains("         ^~~~~~\n"),
        "the element caret spans `Widget` alone: {stderr}"
    );
    // Never leak the generator symbol or a mangled internal name.
    assert!(
        !stderr.contains("_derive_ser") && !stderr.contains('$'),
        "no comptime-internal symbol in the diagnostic: {stderr}"
    );
}

#[test]
fn a_generator_bug_is_a_dual_span_error_not_a_crash() {
    // An engine fault inside a generator (here an out-of-range `reflect.field`)
    // is the library author's bug, not the consumer's: it renders as the
    // dual-span generator-fault form (the generator line AND "instantiated
    // here"), and the compiler exits with a diagnostic, never a panic (E3d,
    // #1293).
    let src = "\
@derivable(Ser)
def _derive_ser[T](T value):
    comptime for f in reflect.type_info(T).fields:
        print(reflect.field(value, comptime f.index + 100))

interface Ser:
    def ser(self)

@derive(Ser)
struct User:
    int64 id

def main():
    auto u = User(1)
    u.ser()
";
    let f = write_zap("e3d-generator-bug", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(
        !out.status.success(),
        "the generator fault must fail the compile"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error in comptime generator"),
        "dual-span generator-fault header: {stderr}"
    );
    assert!(
        stderr.contains("note: instantiated here"),
        "the second span points at the derive site: {stderr}"
    );
    assert!(
        !stderr.contains("panicked") && !stderr.contains("cannot derive"),
        "a generator bug is a clean dual-span diagnostic, not a panic or the consumer header: {stderr}"
    );
}
