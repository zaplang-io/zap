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

#[test]
fn reflected_type_argument_resolves_in_a_derive_generator() {
    // #1575: a derive generator names a nested derived element type through
    // reflection — `gen[f.ty.elem]` for a `list` element and `gen[f.ty.value]`
    // for a `dict` value. Comptime specialization rewrites the reflected
    // type-argument subscript to the concrete element type, so the enclosing
    // generic call resolves. Before #1575 the subscript kept its comptime
    // `f.ty.elem` reference and sema read the callee as an unknown value being
    // indexed (`unknown variable '_derive_rt'`).
    let src = "\
@derivable(Rt)
def _derive_rt[T](T value) -> int64:
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == \"list\":
            for x in reflect.field(value, comptime f.index):
                if f.ty.elem.kind.name == \"struct\":
                    return _derive_rt[f.ty.elem](x)
        elif f.ty.kind.name == \"dict\":
            auto d = reflect.field(value, comptime f.index)
            for k in d.keys():
                if f.ty.value.kind.name == \"struct\":
                    return _derive_rt[f.ty.value](d[k])
        elif f.ty.name == \"int64\":
            return reflect.field(value, comptime f.index)
    return 0

interface Rt:
    def rt(self) -> int64

@derive(Rt)
struct Item:
    int64 id

@derive(Rt)
struct C:
    list[Item] items
    dict[str, Item] m

def main():
    auto c = C([Item(7)], {\"a\": Item(9)})
    print(c.rt())
"
    .to_string();
    let f = write_zap("reflected-type-arg", &src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a reflected type argument must resolve; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("unknown variable") && !stderr.contains("cannot infer type argument"),
        "the generic element decoder must resolve, not read as an unknown/uninferred name: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("_derive_rt") && stdout.contains("Item"),
        "the element generator must be monomorphized for `Item`; ZIR:\n{stdout}"
    );
}

#[test]
fn reflect_construct_variant_builds_an_enum_variant() {
    // #1561: a decode generator builds an enum variant it selected at comptime
    // from `reflect.type_info(T).variants` via `reflect.construct_variant(T,
    // comptime v.index, payload…)` — the enum counterpart of `reflect.construct`.
    // A payload-free known variant and the `Unknown(str)` fallback both resolve.
    let src = "\
@derivable(EDec)
def _derive_edec[T](str s) -> T:
    comptime for v in reflect.type_info(T).variants:
        if len(v.fields) == 0:
            if s == v.name:
                return reflect.construct_variant(T, comptime v.index)
    comptime for v in reflect.type_info(T).variants:
        if len(v.fields) == 1:
            return reflect.construct_variant(T, comptime v.index, s)
    return reflect.construct_variant(T, comptime 0)

interface EDec:
    def edec(str s) -> Self

@derive(Json, EDec)
@json(open)
enum Flavor:
    Vanilla
    Chocolate
    Unknown(str value)

def main():
    print(Flavor.edec(\"Chocolate\").to_json())
    print(Flavor.edec(\"Mango\").to_json())
"
    .to_string();
    let f = write_zap("construct-variant", &src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "an enum variant must be constructible by reflection; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("unknown name") && !stderr.contains("not a non-generic"),
        "construct_variant must build the variant, not fail name resolution: {stderr}"
    );
}

#[test]
fn reflect_construct_builds_a_generic_struct_instance() {
    // #1561: `reflect.construct(T)` where `T` resolves to a generic instance
    // (`Pair[int64, str]`) zero-constructs that monomorphized instance — the
    // generic counterpart of the non-generic `reflect.construct`.
    let src = "\
@derivable(Mk)
def _derive_mk[T](str s) -> T:
    return reflect.construct(T)

interface Mk:
    def mk(str s) -> Self

struct Pair[A, B]:
    A a
    B b

def main():
    auto p = _derive_mk[Pair[int64, str]](\"\")
    print(p.a)
    print(p.b)
"
    .to_string();
    let f = write_zap("construct-generic", &src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a generic struct instance must be constructible by reflection; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("not supported yet"),
        "the generic-instance reflect.construct path must be implemented: {stderr}"
    );
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
    // The chain attributes to the enclosing field and the container level it
    // crossed (mono walks the field's declared type to the cause, #1293).
    assert!(
        stderr.contains("required because of field `items` of `Bag`"),
        "backtrace frame at the enclosing field: {stderr}"
    );
    assert!(
        stderr.contains("required because of the element type of `list[Widget]`"),
        "backtrace frame at the list element level: {stderr}"
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
fn a_deeply_nested_derive_failure_renders_the_full_required_because_chain() {
    // #1293 multi-level chain: when a generator rejects a type buried in a
    // container-of-containers (`dict[str, list[Widget]]`, element struct), the
    // diagnostic is a `required because of …` chain with one frame per level
    // crossed, ordered inner-first (nearest the failure), climbing out to the
    // field — then the library's own root cause.
    let src = "\
@derivable(Ser)
def _derive_ser[T](T value):
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == \"dict\":
            if f.ty.value.kind.name == \"list\":
                if f.ty.value.elem.kind.name == \"struct\":
                    comptime.error(f.ty.value.elem.span, \"`Ser` is not implemented for this type\")

interface Ser:
    def ser(self)

@derive(Ser)
struct Widget:
    int64 w
@derive(Ser)
struct Bag:
    dict[str, list[Widget]] buckets

def main():
    auto b = Bag({})
    b.ser()
";
    let f = write_zap("e3d-deep-chain", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(!out.status.success(), "the derive must fail to compile");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error[ZAP7003]: cannot derive `Ser` for `Bag`"),
        "consumer header: {stderr}"
    );
    // Every container level crossed is a frame, each naming its container type.
    for needle in [
        "required because of the element type of `list[Widget]`",
        "required because of the value type of `dict[str, list[Widget]]`",
        "required because of field `buckets` of `Bag`",
    ] {
        assert!(
            stderr.contains(needle),
            "missing frame `{needle}`: {stderr}"
        );
    }
    // The dict *key* (`str`) is not on the path to the cause, so no key frame.
    assert!(
        !stderr.contains("key type of"),
        "the unrelated dict key must not appear in the chain: {stderr}"
    );
    // Ordering: inner-first — element type, then value type, then the field,
    // then the library's own root cause last.
    let pos = |s: &str| {
        stderr
            .find(s)
            .unwrap_or_else(|| panic!("missing `{s}`: {stderr}"))
    };
    let elem = pos("the element type of `list[Widget]`");
    let value = pos("the value type of `dict[str, list[Widget]]`");
    let field = pos("field `buckets` of `Bag`");
    let cause = pos("the generator reported: `Ser` is not implemented");
    assert!(
        elem < value && value < field && field < cause,
        "chain must read inner-first (element < value < field < root cause): \
         elem={elem} value={value} field={field} cause={cause}\n{stderr}"
    );
    assert!(
        !stderr.contains("_derive_ser") && !stderr.contains('$'),
        "no comptime-internal symbol: {stderr}"
    );
}

#[test]
fn a_tuple_element_derive_failure_names_the_element_position() {
    // #1293: a tuple element on the path gives a positional frame
    // (`element 2 of (int64, Widget)`), 1-based, naming the tuple type.
    let src = "\
@derivable(Ser)
def _derive_ser[T](T value):
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == \"tuple\":
            comptime.error(f.ty.elems[1].span, \"`Ser` is not implemented for this type\")

interface Ser:
    def ser(self)

@derive(Ser)
struct Widget:
    int64 w
@derive(Ser)
struct Bag:
    (int64, Widget) pair

def main():
    auto b = Bag((1, Widget(2)))
    b.ser()
";
    let f = write_zap("e3d-tuple-chain", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(!out.status.success(), "the derive must fail to compile");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("required because of element 2 of `(int64, Widget)`"),
        "positional tuple frame (1-based): {stderr}"
    );
    assert!(
        stderr.contains("required because of field `pair` of `Bag`"),
        "field frame: {stderr}"
    );
}

#[test]
fn an_enum_library_derive_wires_through_to_the_generator() {
    // #1373 (epic #1371): `@derive(X)` on an enum is recognized, the library
    // trait is stripped, a forwarding stub is synthesized, and the generator
    // specialized at the enum compiles end-to-end (here a trivial generator —
    // the variant-iterating body is the codegen slice #1374). This must not
    // error as an unknown/deferred derive, and must not crash.
    let src = "\
@derivable(Dump)
def _derive_dump[T](T self):
    pass

interface Dump:
    def dump(self)

@derive(Dump)
enum Shape:
    Circle(float64 r)
    Rect(float64 w, float64 h)
    Unit

def main():
    auto s = Shape.Circle(1.0)
    s.dump()
";
    let f = write_zap("e3b-enum-wire", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "enum @derive must compile through to IR; stderr: {stderr}"
    );
    // Not left as a deferred/unknown derive, and no comptime-internal leak.
    assert!(
        !stderr.contains("cannot derive") && !stderr.contains("ZAP5035"),
        "enum derive must be recognized, not deferred: {stderr}"
    );
}

#[test]
fn an_enum_derive_generator_discriminates_variants_via_comptime_match() {
    // #1374 (epic #1371): the full enum-derive surface. A generator uses
    // `match value: case comptime v in reflect.type_info(T).variants as payload:`
    // to discriminate the active variant, with an inner `comptime for f in
    // v.fields:` and `reflect.field(payload, i)` reading the payload. This must
    // parse, type-check, and lower to IR on the host (the comptime arm expands to
    // one ordinary variant-pattern arm per variant, reusing the existing
    // enum-match lowering).
    let src = "\
@derivable(Sum)
def _derive_sum[T](T value) -> int64:
    match value:
        case comptime v in reflect.type_info(T).variants as payload:
            int64 acc = 0
            comptime for f in v.fields:
                acc = acc + reflect.field(payload, comptime f.index)
            return acc
    return 0

interface Sum:
    def sum(self) -> int64

@derive(Sum)
enum N:
    Zero
    One(int64 a)
    Two(int64 a, int64 b)

def main():
    auto n = N.Two(3, 4)
    print(n.sum())
";
    let f = write_zap("e3-enum-comptime-match", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the enum-derive comptime match must lower to IR; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The forwarding stub calls the specialized generator, which discriminates
    // via the tag field and loads payload fields (the existing enum-match
    // lowering) — no comptime construct survives to IR.
    assert!(
        stdout.contains("@_derive_sum$N") && stdout.contains("fieldget"),
        "specialized generator discriminates + loads payload fields: {stdout}"
    );
    assert!(
        !stderr.contains("not available") && !stderr.contains("reached lowering"),
        "no comptime construct should survive to lowering: {stderr}"
    );
}

#[test]
fn a_derive_generator_dispatches_on_kind_across_struct_and_enum() {
    // #1394: one `@derivable` generator handles both a struct and an enum by
    // comptime-dispatching on `reflect.type_info(T).kind` at its top level. The
    // dead-kind branch (the enum `match` when `T` is a struct, the struct field
    // fold when `T` is an enum) is pruned per instantiation, so a single trait
    // derives on both kinds — the prerequisite for a real codec and for
    // recursion where an enum payload is a struct.
    let src = "\
@derivable(Sum)
def _derive_sum[T](T value) -> int64:
    if reflect.type_info(T).kind.name == \"enum\":
        match value:
            case comptime v in reflect.type_info(T).variants as payload:
                int64 acc = 0
                comptime for f in v.fields:
                    acc = acc + reflect.field(payload, comptime f.index)
                return acc
        return 0
    else:
        int64 sacc = 0
        comptime for f in reflect.type_info(T).fields:
            sacc = sacc + reflect.field(value, comptime f.index)
        return sacc

interface Sum:
    def sum(self) -> int64

@derive(Sum)
struct Pair:
    int64 a
    int64 b
@derive(Sum)
enum E:
    Nothing
    Two(int64 a, int64 b)

def main():
    print(Pair(10, 20).sum())
    print(E.Two(3, 4).sum())
";
    let f = write_zap("e3-kind-dispatch", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a kind-dispatch generator must compile for both kinds; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Both kinds specialize from the one generator; the dead branch is pruned
    // (no stray `reflect`, no comptime construct at lowering).
    assert!(
        stdout.contains("@_derive_sum$Pair") && stdout.contains("@_derive_sum$E"),
        "both struct and enum specializations present: {stdout}"
    );
    assert!(
        !stderr.contains("unknown variable 'reflect'") && !stderr.contains("reached lowering"),
        "the dead-kind branch must be pruned, not lowered: {stderr}"
    );
}

#[test]
fn an_inapplicable_enum_derive_points_at_the_variant_payload_field() {
    // #1375 (E3d): a derive generator that rejects an enum variant's payload
    // field must render the consumer `ZAP7003` pointing at that payload field,
    // with a `required because of field … of <Enum>` frame — the enum analog of
    // the struct-field attribution (#1293/#1346), now that variant payload
    // fields are attributed in the chain.
    let src = "\
@derivable(Ser)
def _derive_ser[T](T value):
    comptime for v in reflect.type_info(T).variants:
        comptime for f in v.fields:
            if f.ty.kind.name == \"struct\":
                comptime.error(f.span, \"`Ser` is not implemented for a struct payload\")

interface Ser:
    def ser(self)

@derive(Ser)
struct W:
    int64 w
@derive(Ser)
enum E:
    Unit
    Bad(W inner)

def main():
    auto e = E.Bad(W(1))
    e.ser()
";
    let f = write_zap("e3d-enum-variant-frame", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(
        !out.status.success(),
        "the enum derive must fail to compile"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error[ZAP7003]: cannot derive `Ser` for `E`"),
        "consumer header naming the enum: {stderr}"
    );
    assert!(
        stderr.contains("required because of field `inner` of `E`"),
        "the frame attributes the offending variant payload field: {stderr}"
    );
    assert!(
        stderr.contains("the generator reported: `Ser` is not implemented for a struct payload"),
        "the library's own message as the root cause: {stderr}"
    );
    assert!(
        !stderr.contains("_derive_ser") && !stderr.contains('$'),
        "no comptime-internal symbol in the diagnostic: {stderr}"
    );
}

#[test]
fn an_enum_derive_recurses_through_an_enum_typed_variant_payload() {
    // #1401: a `@derive` generator may recurse through a variant payload that is
    // ITSELF a derived enum — the generated `$cvp` payload capture is enum-typed
    // and dispatches its method correctly. This was mis-filed as a core defect:
    // the original repro only failed because its generator called `.sum()` on a
    // scalar leaf field (an `int64`, which has no `sum` method) → `ZAP6001`. A
    // per-field comptime kind check routes scalar leaves to a direct add and
    // struct/enum payloads to a recursive `.sum()`, so an enum nesting another
    // enum lowers end-to-end. This pins that it compiles and that `Outer`'s
    // specialization dispatches through `Inner`'s derived method (the enum-typed
    // capture, not a scalar), so the behavior cannot silently regress.
    let src = "\
@derivable(Sum)
def _derive_sum[T](T value) -> int64:
    if reflect.type_info(T).kind.name == \"enum\":
        match value:
            case comptime v in reflect.type_info(T).variants as payload:
                int64 acc = 0
                comptime for f in v.fields:
                    if f.ty.kind.name == \"struct\" or f.ty.kind.name == \"enum\":
                        acc = acc + reflect.field(payload, comptime f.index).sum()
                    else:
                        acc = acc + reflect.field(payload, comptime f.index)
                return acc
        return 0
    else:
        int64 sacc = 0
        comptime for f in reflect.type_info(T).fields:
            sacc = sacc + reflect.field(value, comptime f.index)
        return sacc

interface Sum:
    def sum(self) -> int64

@derive(Sum)
enum Inner:
    Leaf(int64 a)

@derive(Sum)
enum Outer:
    O(Inner inner)

def main():
    print(Outer.O(Inner.Leaf(42)).sum())
";
    let f = write_zap("e3-enum-enum-recurse", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "enum->enum recursion must lower to IR; stderr: {stderr}"
    );
    // The receiver of the recursive call is the enum-typed payload capture, not a
    // struct or scalar — the exact dispatch the issue claimed was broken.
    assert!(
        !stderr.contains("non-struct value") && !stderr.contains("ZAP6001"),
        "no spurious non-struct method error on an enum-typed capture: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("@_derive_sum$Outer") && stdout.contains("@_derive_sum$Inner"),
        "both the outer and inner enum specialize from the one generator: {stdout}"
    );
    assert!(
        stdout.contains("call @Inner__sum"),
        "the outer specialization dispatches through the inner enum's derived \
         method on its enum-typed payload capture: {stdout}"
    );
}

#[test]
fn a_generic_enum_constructs_with_explicit_type_arguments() {
    // #1402: a generic enum is constructible with an explicit type argument and
    // a variant path — the data-carrying call form `Box[int64].Full(9)` and the
    // payload-free member form `Box[int64].Empty` — then matched. Before the
    // fix both failed in sema with `ZAP3001: unknown name 'Box'` (the base
    // `Box[int64]` was resolved as a value subscript). No variant mentions `T`,
    // so inference cannot recover the instance; the explicit argument is the
    // only way to name it, which is exactly the case the issue reports.
    let src = "\
enum Box[T]:
    Empty
    Full(int64 a)

def main():
    auto b = Box[int64].Full(9)
    match b:
        case Box.Full(n):
            print(n)
        case Box.Empty:
            print(-1)
    auto e = Box[int64].Empty
    match e:
        case Box.Full(n):
            print(n)
        case Box.Empty:
            print(0)
";
    let f = write_zap("gen-enum-ctor", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a generic enum must construct with explicit type arguments; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("ZAP3001") && !stderr.contains("unknown name"),
        "the type-argument subscript must not be read as a value: {stderr}"
    );
}

#[test]
fn a_generic_enum_derive_runs_on_an_explicitly_constructed_instance() {
    // #1402: generic-enum `@derive` works end-to-end now that an instance can be
    // built. The derived `sum()` is called on an explicitly-constructed generic
    // enum value (both a data variant and the payload-free one), and the
    // generator specializes to the concrete `Box[int64]` instance.
    let src = "\
@derivable(Sum)
def _derive_sum[T](T value) -> int64:
    match value:
        case comptime v in reflect.type_info(T).variants as payload:
            int64 acc = 0
            comptime for f in v.fields:
                acc = acc + reflect.field(payload, comptime f.index)
            return acc
    return 0

interface Sum:
    def sum(self) -> int64

@derive(Sum)
enum Box[T]:
    Empty
    Full(int64 a)

def main():
    print(Box[int64].Full(9).sum())
    print(Box[int64].Empty.sum())
";
    let f = write_zap("gen-enum-derive", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "generic-enum derive must lower end-to-end; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("@_derive_sum$Box$int64"),
        "the generator specializes to the concrete generic-enum instance: {stdout}"
    );
    assert!(
        !stderr.contains("not available") && !stderr.contains("reached lowering"),
        "no comptime construct should survive to lowering: {stderr}"
    );
}

#[test]
fn an_explicit_type_argument_subscript_picks_the_generic_enum_instance() {
    // #1402: the explicit type argument genuinely selects the instance — two
    // different arguments (`Box[int64]` vs `Box[str]`) specialize to two
    // distinct layouts, and a `T`-typed payload takes the argument's type. This
    // is the inference-free counterpart of `Box.Full(x)` (which pins `T` from
    // the payload).
    let src = "\
enum Box[T]:
    Empty
    Full(T a)

def main():
    auto a = Box[int64].Full(9)
    auto b = Box[str].Empty
    match a:
        case Box.Full(x):
            print(x)
        case Box.Empty:
            print(-1)
    match b:
        case Box.Full(y):
            print(y)
        case Box.Empty:
            print(0)
";
    let f = write_zap("gen-enum-two-instances", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "two distinct generic-enum instances must both lower; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("ZAP3001"),
        "neither type-argument subscript is read as a value: {stderr}"
    );
}

#[test]
fn a_type_argument_subscript_on_a_non_generic_enum_is_an_error() {
    // #1402: the explicit-type-argument form is gated to genuinely generic
    // enums — a type-argument subscript on a non-generic enum is not silently
    // accepted (it is a clear error, as it was before the fix).
    let src = "\
enum C:
    Red
    Blue

def main():
    auto x = C[int64].Red
    print(1)
";
    let f = write_zap("gen-enum-nongeneric", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(
        !out.status.success(),
        "a type-argument subscript on a non-generic enum must be rejected"
    );
}

#[test]
fn an_explicit_generic_enum_variant_is_arity_and_name_checked() {
    // #1402: the explicit form still type-checks the variant and its payload —
    // a data variant used payload-free is `ZAP4061`, and an unknown variant is
    // `ZAP4060`, exactly as the bare `Enum.Variant` forms are.
    let data_as_bare = "\
enum Box[T]:
    Empty
    Full(int64 a)

def main():
    auto x = Box[int64].Full
    print(1)
";
    let f = write_zap("gen-enum-data-bare", data_as_bare);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ZAP4061"),
        "a data variant used payload-free is ZAP4061: {stderr}"
    );

    let unknown = "\
enum Box[T]:
    Empty
    Full(int64 a)

def main():
    auto x = Box[int64].Nope
    print(1)
";
    let f = write_zap("gen-enum-unknown-variant", unknown);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ZAP4060"),
        "an unknown variant is ZAP4060: {stderr}"
    );
}

#[test]
fn a_library_generator_shadows_the_built_in_eq_without_replacing_it() {
    // E4 (#1431): a library `@derivable` generator can express the compiler's
    // structural equality. A struct derives BOTH the built-in `Eq` (reached via
    // `==`) and a library `StructEq`, and both equality functions are emitted —
    // the built-in `P____eq__` is untouched (this is a shadow proof, not a
    // replacement) and the library stub forwards to the specialized generator.
    // Runtime agreement between the two is pinned by the golden
    // `derive_eq_shadow` under the CI LLVM matrix.
    let src = "\
@derivable(StructEq)
def _derive_structeq[T](T a, T b) -> bool:
    comptime for f in reflect.type_info(T).fields:
        if reflect.field(a, comptime f.index) != reflect.field(b, comptime f.index):
            return False
    return True

interface StructEq:
    def structeq(self, Self other) -> bool

@derive(Eq, StructEq)
struct P:
    int64 x
    str s

def main():
    auto p = P(1, \"a\")
    print(p == p)
    print(p.structeq(p))
";
    let f = write_zap("e4-eq-shadow", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the shadow generator must lower alongside the built-in; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The built-in Eq is left in place (shadow, not replace)...
    assert!(
        stdout.contains("@P____eq__"),
        "the built-in `__eq__` is untouched: {stdout}"
    );
    // ...and the library trait specializes its generator and forwards to it.
    assert!(
        stdout.contains("@_derive_structeq$P"),
        "the library generator specializes at P: {stdout}"
    );
    assert!(
        stdout.contains("@P__structeq"),
        "the library forwarding stub is synthesized: {stdout}"
    );
}

#[test]
fn a_library_generator_shadows_the_built_in_ord_without_replacing_it() {
    // E4 (#1443): a library `@derivable` generator expresses three-way structural
    // comparison. A struct derives both built-in `Ord` (reached via `<`) and a
    // library `StructOrd`; both comparison functions are emitted, the built-in
    // `P____cmp__` untouched. Ordering agreement is pinned by the golden
    // `derive_ord_shadow` under the CI LLVM matrix.
    let src = "\
@derivable(StructOrd)
def _derive_structord[T](T a, T b) -> int64:
    comptime for f in reflect.type_info(T).fields:
        if reflect.field(a, comptime f.index) < reflect.field(b, comptime f.index):
            return -1
        if reflect.field(a, comptime f.index) > reflect.field(b, comptime f.index):
            return 1
    return 0

interface StructOrd:
    def structcmp(self, Self other) -> int64

@derive(Ord, Eq, StructOrd)
struct P:
    int64 x
    str s

def main():
    auto p = P(1, \"a\")
    print(p < p)
    print(p.structcmp(p))
";
    let f = write_zap("e4-ord-shadow", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the shadow Ord generator must lower alongside the built-in; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("@P____cmp__"),
        "the built-in `__cmp__` is untouched: {stdout}"
    );
    assert!(
        stdout.contains("@_derive_structord$P") && stdout.contains("@P__structcmp"),
        "the library generator specializes and forwards: {stdout}"
    );
}

#[test]
fn a_library_generator_shadows_the_built_in_hash_without_replacing_it() {
    // E4 (#1444): a library `@derivable` generator reproduces the built-in hash
    // fold byte-identically — seed 0, `hash_combine` (→ `zap_hash_combine`) over
    // each field's `.__hash__()` in declaration order. Both hash functions are
    // emitted, the built-in `P____hash__` untouched; value equality is pinned by
    // the golden `derive_hash_shadow` under the CI LLVM matrix.
    let src = "\
@derivable(StructHash)
def _derive_structhash[T](T value) -> int64:
    int64 h = 0
    comptime for f in reflect.type_info(T).fields:
        h = hash_combine(h, reflect.field(value, comptime f.index).__hash__())
    return h

interface StructHash:
    def structhash(self) -> int64

@derive(Hash, Eq, StructHash)
struct P:
    int64 x
    str s

def main():
    auto p = P(1, \"a\")
    print(p.__hash__() == p.structhash())
";
    let f = write_zap("e4-hash-shadow", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the shadow Hash generator must lower alongside the built-in; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("@P____hash__"),
        "the built-in `__hash__` is untouched: {stdout}"
    );
    assert!(
        stdout.contains("@_derive_structhash$P") && stdout.contains("@P__structhash"),
        "the library generator specializes and forwards: {stdout}"
    );
    // The shadow folds via the same `zap_hash_combine` primitive as the built-in.
    assert!(
        stdout.contains("@zap_hash_combine"),
        "the shadow uses the runtime hash-combine fold: {stdout}"
    );
}

#[test]
fn a_library_generator_shadows_the_built_in_repr_without_replacing_it() {
    // E4 (#1430): now that `repr()` is a builtin (#1384) and
    // `reflect.type_info(T).name` folds to the type name in a value position, a
    // library `@derivable` generator can express the built-in `Repr`'s
    // `TypeName(field=value, …)` form. A struct derives both the built-in `Repr`
    // (reached via the `repr()` builtin, which resolves its `__repr__`) and a
    // library `StructRepr`; both comparison functions are emitted, the built-in
    // `P____repr__` untouched. String agreement is pinned by the golden
    // `derive_repr_shadow` under the CI LLVM matrix.
    let src = "\
@derivable(StructRepr)
def _derive_structrepr[T](T value) -> str:
    str out = reflect.type_info(T).name + \"(\"
    comptime for f in reflect.type_info(T).fields:
        if comptime f.index > 0:
            out = out + \", \"
        out = out + f.name + \"=\" + repr(reflect.field(value, comptime f.index))
    return out + \")\"

interface StructRepr:
    def structrepr(self) -> str

@derive(Repr, StructRepr)
struct P:
    int64 x
    str s

def main():
    auto p = P(1, \"a\")
    print(repr(p))
    print(p.structrepr())
";
    let f = write_zap("e4-repr-shadow", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the shadow Repr generator must lower alongside the built-in; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("@P____repr__"),
        "the built-in `__repr__` is untouched: {stdout}"
    );
    assert!(
        stdout.contains("@_derive_structrepr$P") && stdout.contains("@P__structrepr"),
        "the library generator specializes and forwards: {stdout}"
    );
    // `reflect.type_info(T).name` folded to the concrete type name string.
    assert!(
        stdout.contains("strconst \"P\""),
        "the type name folds to a string literal in the shadow: {stdout}"
    );
}

#[test]
fn a_library_generator_shadows_the_built_in_json_codec_without_replacing_it() {
    // E4 (#1470): library `@derivable` generators can express the built-in
    // `@derive(Json)` *codec* — both encode and decode — over the
    // source-accessible JSON DOM (`JsonValue` + public `json.dumps`/`json.loads`),
    // using the two enablers that landed for E4: `reflect.construct` /
    // `reflect.set_field` (#1454) and derived-method `Self`-return typing (#1455).
    // A struct derives BOTH the built-in `Json` (`to_json`/`from_json`) and the
    // library `JsonEncode` (`to_jsoncodec`) + `JsonDecode` (`from_jsoncodec`); all
    // four functions are emitted, the built-in `Flat__to_json`/`Flat__from_json`
    // untouched (shadow, not replace). Round-trip agreement is pinned by the
    // golden `derive_json_codec_shadow` under the CI LLVM matrix.
    let src = "\
@derivable(JsonEncode)
def _derive_jsonencode[T](T value) -> str:
    dict[str, JsonValue] m = {}
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == \"struct\":
            m[f.name] = json.loads(reflect.field(value, comptime f.index).to_jsoncodec())
        elif f.ty.kind.name == \"optional\":
            Optional[int64] ov = reflect.field(value, comptime f.index)
            JsonValue jv = JsonValue.Null
            match ov:
                case Optional.Some(x):
                    jv = JsonValue.Int(x)
                case Optional.None:
                    jv = JsonValue.Null
            m[f.name] = jv
        elif f.ty.name == \"int64\":
            m[f.name] = JsonValue.Int(reflect.field(value, comptime f.index))
        elif f.ty.name == \"str\":
            m[f.name] = JsonValue.Str(reflect.field(value, comptime f.index))
        elif f.ty.name == \"bool\":
            m[f.name] = JsonValue.Bool(reflect.field(value, comptime f.index))
        elif f.ty.name == \"float64\":
            m[f.name] = JsonValue.Float(reflect.field(value, comptime f.index))
        else:
            comptime.error(f.span, \"JsonEncode: unsupported field kind\")
    return json.dumps(JsonValue.Object(m))

interface JsonEncode:
    def to_jsoncodec(self) -> str

def _jc_int(JsonValue v) -> int64:
    match v:
        case JsonValue.Int(i):
            return i
        case _:
            return 0

def _jc_str(JsonValue v) -> str:
    match v:
        case JsonValue.Str(x):
            return x
        case _:
            return \"\"

@derivable(JsonDecode)
def _derive_jsondecode[T](str s) -> T:
    auto out = reflect.construct(T)
    dict[str, JsonValue] m = {}
    match json.loads(s):
        case JsonValue.Object(fields):
            m = fields
        case _:
            m = {}
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == \"struct\":
            reflect.set_field(out, comptime f.index, _derive_jsondecode(json.dumps(m[f.name])))
        elif f.ty.kind.name == \"optional\":
            Optional[int64] dv = None
            if f.name in m:
                match m[f.name]:
                    case JsonValue.Int(i):
                        dv = Optional.Some(i)
                    case _:
                        dv = None
            reflect.set_field(out, comptime f.index, dv)
        elif f.ty.name == \"int64\":
            reflect.set_field(out, comptime f.index, _jc_int(m[f.name]))
        elif f.ty.name == \"str\":
            reflect.set_field(out, comptime f.index, _jc_str(m[f.name]))
        else:
            comptime.error(f.span, \"JsonDecode: unsupported field kind\")
    return out

interface JsonDecode:
    def from_jsoncodec(str s) -> Self

@derive(Hash, Eq, Json, JsonEncode, JsonDecode)
struct Inner:
    int64 a

@derive(Eq, Json, JsonEncode, JsonDecode)
struct Flat:
    int64 x
    str s

@derive(Eq, Json, JsonEncode, JsonDecode)
struct Nested:
    int64 n
    Inner inner
    Optional[int64] opt

def main():
    auto p = Flat(7, \"hi\")
    print(Flat.from_jsoncodec(p.to_jsoncodec()) == p)
    auto o = Nested(3, Inner(9), Optional.Some(5))
    print(Nested.from_jsoncodec(o.to_jsoncodec()) == o)
    print(Nested.from_json(o.to_jsoncodec()) == o)
    auto e = Nested(1, Inner(0), None)
    print(Nested.from_jsoncodec(e.to_jsoncodec()) == e)
";
    let f = write_zap("e4-json-codec-shadow", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the shadow Json codec generators must lower alongside the built-in; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The built-in Json codec is left in place (shadow, not replace).
    assert!(
        stdout.contains("@Flat__to_json") && stdout.contains("@Flat__from_json"),
        "the built-in Json codec is untouched: {stdout}"
    );
    // The library encode + decode specialize their generators and forward.
    assert!(
        stdout.contains("@_derive_jsonencode$Flat") && stdout.contains("@_derive_jsondecode$Flat"),
        "both library generators specialize at Flat: {stdout}"
    );
    assert!(
        stdout.contains("@Flat__to_jsoncodec") && stdout.contains("@Flat__from_jsoncodec"),
        "both library forwarding stubs are synthesized on Flat: {stdout}"
    );
    // A nested derived struct: library encode recurses (both specialize)...
    assert!(
        stdout.contains("@Nested__to_jsoncodec")
            && stdout.contains("@_derive_jsonencode$Nested")
            && stdout.contains("@_derive_jsonencode$Inner"),
        "the library encode recurses through the nested derived struct: {stdout}"
    );
    // ...and (since #1490 widened `reflect.construct` to zero-construct
    // nested-struct and `Optional` fields) decode is now ALSO derived on the
    // nested struct: `Nested__from_jsoncodec` recurses into `Inner`'s decoder.
    assert!(
        stdout.contains("@Nested__from_jsoncodec")
            && stdout.contains("@_derive_jsondecode$Nested")
            && stdout.contains("@_derive_jsondecode$Inner"),
        "the library decode recurses through the nested derived struct (#1490): {stdout}"
    );
    // #1522: the `Optional` decode is null-safe — the presence check (`in`)
    // lowers to a dict-membership test before the DOM read, so an absent key
    // (the built-in omits a `None` optional) maps to `None` rather than
    // dereferencing a missing entry.
    assert!(
        stdout.contains("@zap_dict_contains_str"),
        "the Optional decode presence-checks the key (#1522): {stdout}"
    );
}

#[test]
fn reflect_construct_and_set_field_build_a_value() {
    // #1454 (E4 enabler): the reflect value-construction + field-write surface.
    // A generator zero-constructs a value of T (`reflect.construct(T)`), writes
    // each field (`reflect.set_field`, desugared to `out.<name> = …`), then reads
    // the fields back — the primitive a codec decoder builds on. Returns a scalar
    // (not `Self`) to avoid the unrelated derived-method `Self`-return gap (#1455).
    let src = "\
@derivable(Build)
def _derive_build[T](T self, list[int64] src) -> int64:
    auto out = reflect.construct(T)
    comptime for f in reflect.type_info(T).fields:
        reflect.set_field(out, comptime f.index, src[comptime f.index])
    int64 total = 0
    comptime for f in reflect.type_info(T).fields:
        total = total + reflect.field(out, comptime f.index)
    return total

interface Build:
    def build(self, list[int64] src) -> int64

@derive(Build)
struct P:
    int64 x
    int64 y

def main():
    auto seed = P(0, 0)
    print(seed.build([10, 20]))
";
    let f = write_zap("e4-reflect-construct", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "reflect.construct + reflect.set_field must lower; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("unknown variable 'reflect'") && !stderr.contains("non-struct"),
        "no unresolved reflect / non-struct error: {stderr}"
    );
    // `reflect.construct(T)` zero-inits the concrete struct and `reflect.set_field`
    // lowers to ordinary field stores (there is no residual `reflect` symbol).
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("fieldset struct#") && stdout.contains("@_derive_build$P"),
        "the generator specializes and writes fields: {stdout}"
    );
}

#[test]
fn a_method_returning_self_is_usable_at_the_call_site() {
    // #1455: a method whose declared return is `Self` must be typed as the
    // enclosing type at the call site, so its result can be field-accessed /
    // chained. Before the fix the `Self` return resolved to a non-struct in
    // codegen and `p.clone().x` failed with `ZAP6001: field access on a
    // non-struct value`. Also covers a `Self`-typed parameter.
    let src = "\
struct P:
    int64 x

    def clone(self) -> Self:
        return self

    def pick(self, Self other) -> Self:
        return other

def main():
    auto p = P(5)
    print(p.clone().x)
    print(p.pick(P(9)).x)
";
    let f = write_zap("self-return-method", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a `Self`-returning method must lower and be field-accessible; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("non-struct"),
        "the `Self` return resolves to the struct, not a non-struct: {stderr}"
    );
}

#[test]
fn a_derived_method_returning_self_resolves_to_the_deriving_type() {
    // #1455: the same fix makes a library-derived method with a `Self` return
    // (the shape a codec decoder `from_json(…) -> Self` needs) typed as the
    // deriving struct — so the reflect-construct enabler (#1454) can be consumed
    // for decode: a generator zero-constructs, sets each field, returns the value,
    // and the caller reads it back.
    let src = "\
@derivable(Build)
def _derive_build[T](T self, list[int64] src) -> T:
    auto out = reflect.construct(T)
    comptime for f in reflect.type_info(T).fields:
        reflect.set_field(out, comptime f.index, src[comptime f.index])
    return out

interface Build:
    def build(self, list[int64] src) -> Self

@derive(Build)
struct P:
    int64 x
    int64 y

def main():
    auto seed = P(0, 0)
    auto p = seed.build([10, 20])
    print(p.x)
    print(p.y)
";
    let f = write_zap("self-return-derived", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a derived `Self`-returning decoder must lower and be field-accessible; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("non-struct") && !stderr.contains("unknown variable 'reflect'"),
        "the decoder result types as the deriving struct: {stderr}"
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

#[test]
fn a_library_generator_shadows_the_built_in_csv_codec_without_replacing_it() {
    // E4 (#1480): library `@derivable` generators can express the built-in
    // `@derive(Csv)` *codec* — both encode and decode — in PURE ZAP SOURCE.
    // Unlike Json (#1470), CSV has no source-accessible DOM or module (it is all
    // compiler-private codegen over `zap_csv_*` runtime), so the shadow uses only
    // string primitives (`str()`, `.split`, `",".join`, `int()`/`float()`, a
    // manual bool parse) plus the two E4 enablers: `reflect.construct` /
    // `reflect.set_field` (#1454) and derived-method `Self`-return typing (#1455).
    // A struct derives BOTH the built-in `Csv` (`to_csv`/`from_csv`) and the
    // library `CsvEncode` (`to_csvcodec`) + `CsvDecode` (`from_csvcodec`); all four
    // functions are emitted, the built-in `Row__to_csv`/`Row__from_csv` untouched
    // (shadow, not replace). Round-trip agreement is pinned by the golden
    // `derive_csv_codec_shadow` under the CI LLVM matrix.
    let src = "\
@derivable(CsvEncode)
def _derive_csvencode[T](T value) -> str:
    list[str] parts = []
    comptime for f in reflect.type_info(T).fields:
        if f.ty.name == \"int64\":
            parts.append(str(reflect.field(value, comptime f.index)))
        elif f.ty.name == \"str\":
            parts.append(reflect.field(value, comptime f.index))
        elif f.ty.name == \"bool\":
            parts.append(str(reflect.field(value, comptime f.index)))
        elif f.ty.name == \"float64\":
            parts.append(str(reflect.field(value, comptime f.index)))
        else:
            comptime.error(f.span, \"CsvEncode: unsupported field kind\")
    return \",\".join(parts)

interface CsvEncode:
    def to_csvcodec(self) -> str

def _csv_bool(str col) -> bool:
    return col == \"True\" or col == \"true\"

@derivable(CsvDecode)
def _derive_csvdecode[T](str s) -> T:
    auto out = reflect.construct(T)
    auto cols = s.split(\",\")
    comptime for f in reflect.type_info(T).fields:
        if f.ty.name == \"int64\":
            reflect.set_field(out, comptime f.index, int(cols[comptime f.index]))
        elif f.ty.name == \"str\":
            reflect.set_field(out, comptime f.index, cols[comptime f.index])
        elif f.ty.name == \"bool\":
            reflect.set_field(out, comptime f.index, _csv_bool(cols[comptime f.index]))
        elif f.ty.name == \"float64\":
            reflect.set_field(out, comptime f.index, float(cols[comptime f.index]))
        else:
            comptime.error(f.span, \"CsvDecode: unsupported field kind\")
    return out

interface CsvDecode:
    def from_csvcodec(str s) -> Self

@derive(Eq, Csv, CsvEncode, CsvDecode)
struct Row:
    str city
    int64 pop
    bool capital
    float64 lat

def main():
    auto r = Row(city=\"Paris\", pop=42, capital=True, lat=48.5)
    print(Row.from_csvcodec(r.to_csvcodec()) == r)
    print(Row.from_csvcodec(r.to_csv()) == r)
    print(Row.from_csv(r.to_csvcodec()) == r)
";
    let f = write_zap("e4-csv-codec-shadow", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the shadow Csv codec generators must lower alongside the built-in; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The built-in Csv codec is left in place (shadow, not replace).
    assert!(
        stdout.contains("@Row__to_csv") && stdout.contains("@Row__from_csv"),
        "the built-in Csv codec is untouched: {stdout}"
    );
    // The library encode + decode specialize their generators and forward.
    assert!(
        stdout.contains("@_derive_csvencode$Row") && stdout.contains("@_derive_csvdecode$Row"),
        "both library generators specialize at Row: {stdout}"
    );
    assert!(
        stdout.contains("@Row__to_csvcodec") && stdout.contains("@Row__from_csvcodec"),
        "both library forwarding stubs are synthesized on Row: {stdout}"
    );
}

#[test]
fn a_library_generator_shadows_the_built_in_yaml_codec_without_replacing_it() {
    // E4 (#1484): library `@derivable` generators can express the built-in
    // `@derive(Yaml)` *codec* — both encode and decode — over the
    // source-accessible YAML DOM. YAML 1.2 is a JSON superset, so the public
    // `yaml` module reuses the shared `JsonValue` DOM (`yaml.load(str) ->
    // JsonValue` / `yaml.dump(JsonValue) -> str`), exactly as json.loads/dumps —
    // this is the Json codec shadow (#1470) with those two swapped. Uses the two
    // E4 enablers: `reflect.construct` / `reflect.set_field` (#1454) and
    // derived-method `Self`-return typing (#1455). A struct derives BOTH the
    // built-in `Yaml` (`to_yaml`/`from_yaml`) and the library `YamlEncode`
    // (`to_yamlcodec`) + `YamlDecode` (`from_yamlcodec`); all four are emitted,
    // the built-in `Flat__to_yaml`/`Flat__from_yaml` untouched (shadow, not
    // replace). Round-trip agreement is pinned by the golden
    // `derive_yaml_codec_shadow` under the CI LLVM matrix.
    let src = "\
@derivable(YamlEncode)
def _derive_yamlencode[T](T value) -> str:
    dict[str, JsonValue] m = {}
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == \"struct\":
            m[f.name] = yaml.load(reflect.field(value, comptime f.index).to_yamlcodec())
        elif f.ty.kind.name == \"optional\":
            Optional[int64] ov = reflect.field(value, comptime f.index)
            JsonValue jv = JsonValue.Null
            match ov:
                case Optional.Some(x):
                    jv = JsonValue.Int(x)
                case Optional.None:
                    jv = JsonValue.Null
            m[f.name] = jv
        elif f.ty.name == \"int64\":
            m[f.name] = JsonValue.Int(reflect.field(value, comptime f.index))
        elif f.ty.name == \"str\":
            m[f.name] = JsonValue.Str(reflect.field(value, comptime f.index))
        elif f.ty.name == \"bool\":
            m[f.name] = JsonValue.Bool(reflect.field(value, comptime f.index))
        elif f.ty.name == \"float64\":
            m[f.name] = JsonValue.Float(reflect.field(value, comptime f.index))
        else:
            comptime.error(f.span, \"YamlEncode: unsupported field kind\")
    return yaml.dump(JsonValue.Object(m))

interface YamlEncode:
    def to_yamlcodec(self) -> str

def _yc_int(JsonValue v) -> int64:
    match v:
        case JsonValue.Int(i):
            return i
        case _:
            return 0

def _yc_str(JsonValue v) -> str:
    match v:
        case JsonValue.Str(x):
            return x
        case _:
            return \"\"

@derivable(YamlDecode)
def _derive_yamldecode[T](str s) -> T:
    auto out = reflect.construct(T)
    dict[str, JsonValue] m = {}
    match yaml.load(s):
        case JsonValue.Object(fields):
            m = fields
        case _:
            m = {}
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == \"struct\":
            reflect.set_field(out, comptime f.index, _derive_yamldecode(yaml.dump(m[f.name])))
        elif f.ty.kind.name == \"optional\":
            Optional[int64] dv = None
            if f.name in m:
                match m[f.name]:
                    case JsonValue.Int(i):
                        dv = Optional.Some(i)
                    case _:
                        dv = None
            reflect.set_field(out, comptime f.index, dv)
        elif f.ty.name == \"int64\":
            reflect.set_field(out, comptime f.index, _yc_int(m[f.name]))
        elif f.ty.name == \"str\":
            reflect.set_field(out, comptime f.index, _yc_str(m[f.name]))
        else:
            comptime.error(f.span, \"YamlDecode: unsupported field kind\")
    return out

interface YamlDecode:
    def from_yamlcodec(str s) -> Self

@derive(Hash, Eq, Yaml, YamlEncode, YamlDecode)
struct Inner:
    int64 a

@derive(Eq, Yaml, YamlEncode, YamlDecode)
struct Flat:
    int64 x
    str s

@derive(Eq, Yaml, YamlEncode, YamlDecode)
struct Nested:
    int64 n
    Inner inner
    Optional[int64] opt

def main():
    auto p = Flat(7, \"hi\")
    print(Flat.from_yamlcodec(p.to_yamlcodec()) == p)
    auto o = Nested(3, Inner(9), Optional.Some(5))
    print(Nested.from_yamlcodec(o.to_yamlcodec()) == o)
    print(Nested.from_yaml(o.to_yamlcodec()) == o)
    auto e = Nested(1, Inner(0), None)
    print(Nested.from_yamlcodec(e.to_yamlcodec()) == e)
";
    let f = write_zap("e4-yaml-codec-shadow", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the shadow Yaml codec generators must lower alongside the built-in; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The built-in Yaml codec is left in place (shadow, not replace).
    assert!(
        stdout.contains("@Flat__to_yaml") && stdout.contains("@Flat__from_yaml"),
        "the built-in Yaml codec is untouched: {stdout}"
    );
    // The library encode + decode specialize their generators and forward.
    assert!(
        stdout.contains("@_derive_yamlencode$Flat") && stdout.contains("@_derive_yamldecode$Flat"),
        "both library generators specialize at Flat: {stdout}"
    );
    assert!(
        stdout.contains("@Flat__to_yamlcodec") && stdout.contains("@Flat__from_yamlcodec"),
        "both library forwarding stubs are synthesized on Flat: {stdout}"
    );
    // A nested derived struct: library encode recurses (both specialize)...
    assert!(
        stdout.contains("@Nested__to_yamlcodec")
            && stdout.contains("@_derive_yamlencode$Nested")
            && stdout.contains("@_derive_yamlencode$Inner"),
        "the library encode recurses through the nested derived struct: {stdout}"
    );
    // ...and (since #1490 widened `reflect.construct` to zero-construct
    // nested-struct and `Optional` fields) decode is now ALSO derived on the
    // nested struct: `Nested__from_yamlcodec` recurses into `Inner`'s decoder.
    assert!(
        stdout.contains("@Nested__from_yamlcodec")
            && stdout.contains("@_derive_yamldecode$Nested")
            && stdout.contains("@_derive_yamldecode$Inner"),
        "the library decode recurses through the nested derived struct (#1490): {stdout}"
    );
    // #1522: the `Optional` decode presence-checks the key before the DOM read.
    assert!(
        stdout.contains("@zap_dict_contains_str"),
        "the Optional decode presence-checks the key (#1522): {stdout}"
    );
}

#[test]
fn reflect_construct_zero_sets_nested_optional_and_collection_fields() {
    // #1490 (E4): `reflect.construct(T)` widens the #1454 v1 zero set (scalar /
    // str / bytes) to also zero-construct a nested struct, an `Optional`, a
    // `list` / `dict` / `set`, and a tuple field. A `Self`-returning generator
    // (#1455) zero-constructs the value and `main` reads every field back with a
    // concrete accessor, so the readback path for each new kind lowers: the
    // nested struct is zero-valued, the collections are empty (`len == 0`), and
    // the optional is `None`. Runtime values are pinned by the codec goldens
    // under the CI LLVM matrix; here we assert the whole program lowers.
    let src = "\
@derivable(Make)
def _derive_make[T]() -> T:
    return reflect.construct(T)

interface Make:
    def make() -> Self

@derive(Eq, Hash)
struct Inner:
    int64 a
    str s

@derive(Make)
struct Wide:
    int64 n
    Inner inner
    Optional[int64] opt
    list[int64] xs
    dict[str, int64] m
    set[int64] st
    (int64, str) pair

def main():
    auto z = Wide.make()
    print(z.n)
    print(z.inner.a)
    print(z.inner.s)
    print(len(z.xs))
    print(len(z.m))
    print(len(z.st))
    match z.opt:
        case Optional.None:
            print(\"none\")
        case Optional.Some(v):
            print(v)
";
    let f = write_zap("e4-construct-zeroset-wide", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "reflect.construct must zero-construct nested / Optional / list / dict / \
         set / tuple fields and the readback must lower; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("does not support yet")
            && !stderr.contains("no zero-initializer")
            && !stderr.contains("unknown variable 'reflect'"),
        "no residual reflect / unsupported-field error for the widened kinds: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The generator specializes at `Wide` and the forwarding stub is synthesized;
    // the zero-construction lowers to an ordinary struct field-set (no residual
    // `reflect` symbol survives to IR).
    assert!(
        stdout.contains("@Wide__make") && stdout.contains("fieldset struct"),
        "the construct generator specializes and zero-initializes the struct: {stdout}"
    );
}

#[test]
fn reflect_construct_recurses_through_a_struct_reachable_only_by_indirection() {
    // #1490: a struct that contains itself *through an `Optional` / `list`* is NOT
    // a by-value cycle — the empty optional / empty list breaks the recursion, so
    // the zero-constructor terminates and the program lowers.
    let src = "\
@derivable(Make)
def _derive_make[T]() -> T:
    return reflect.construct(T)

interface Make:
    def make() -> Self

@derive(Make)
struct Node:
    int64 v
    Optional[Node] next
    list[Node] kids

def main():
    auto z = Node.make()
    print(z.v)
";
    let f = write_zap("e4-construct-zeroset-recursive-ok", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a self-reference behind Optional/list is not a cycle and must lower; stderr: {stderr}"
    );
}

#[test]
fn reflect_construct_rejects_a_by_value_struct_cycle_cleanly() {
    // #1490: a struct that contains itself *by value* (directly, or indirectly
    // A -> B -> A) has no finite zero value. The widened zero-constructor must
    // reject it with a clean compile error naming the cycle — never an infinite
    // recursion / hang (this test returning at all proves termination).
    let direct = "\
@derivable(Make)
def _derive_make[T]() -> T:
    return reflect.construct(T)

interface Make:
    def make() -> Self

@derive(Make)
struct Cyc:
    int64 n
    Cyc child

def main():
    auto z = Cyc.make()
    print(z.n)
";
    let f = write_zap("e4-construct-zeroset-cycle-direct", direct);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(
        !out.status.success(),
        "a by-value self-cycle must be a clean error, not success"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("contains itself by value"),
        "the cycle error names the by-value containment: {stderr}"
    );

    // Indirect: A -> B -> A, both by value (B is not itself derived, but is
    // reached as A's field type).
    let indirect = "\
@derivable(Make)
def _derive_make[T]() -> T:
    return reflect.construct(T)

interface Make:
    def make() -> Self

struct B:
    int64 x
    A a

@derive(Make)
struct A:
    int64 y
    B b

def main():
    auto z = A.make()
    print(z.y)
";
    let f = write_zap("e4-construct-zeroset-cycle-indirect", indirect);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(
        !out.status.success(),
        "an indirect A -> B -> A by-value cycle must be a clean error, not success"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("contains itself by value"),
        "the indirect cycle error names the by-value containment: {stderr}"
    );
}

#[test]
fn reflect_construct_still_rejects_an_unsupported_field_type_cleanly() {
    // #1490 widens the zero set but does NOT make it total: a field type with no
    // zero value — a function type — is still a clean "unsupported yet" error
    // (naming the field and type), not a panic and not silent acceptance.
    let src = "\
@derivable(Make)
def _derive_make[T]() -> T:
    return reflect.construct(T)

interface Make:
    def make() -> Self

@derive(Make)
struct HasFn:
    int64 n
    fn(int64) -> int64 f

def main():
    auto z = HasFn.make()
    print(z.n)
";
    let f = write_zap("e4-construct-zeroset-unsupported", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(
        !out.status.success(),
        "a function-typed field has no zero value and must be a clean error"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no zero-initializer")
            && stderr.contains("field `f` of `HasFn`")
            && stderr.contains("function types"),
        "the error names the field and that function types are unsupported: {stderr}"
    );
}

#[test]
fn derive_yaml_lowers_a_top_level_optional_field_without_a_dangling_symbol() {
    // #1514 regression (host-catchable): `@derive(Yaml)` used to miscompile a
    // top-level `Optional[T]` field into a call to a non-existent
    // `Optional$T__to_yaml` — an unresolved external symbol that only the LLVM
    // build caught, because `gen_yaml_to_method` recursed into the `Optional`
    // aggregate as if it were a nested `@derive(Yaml)` struct. The encoder now
    // handles `Optional` one level up (JSON/D-0065 parity), so `Rec__to_yaml`
    // lowers and emits NO `Optional$..__to_yaml` call. Runtime round-trip
    // (Some/None, scalar + nested payload) is pinned by the golden
    // `yaml_derive_optional` under the CI LLVM matrix.
    let src = "\
@derive(Yaml)
struct Addr:
    str city
    int64 zip

@derive(Yaml)
struct Rec:
    str name
    Optional[int64] age
    Optional[Addr] addr

def main():
    auto r = Rec(name=\"a\", age=Optional.Some(1), addr=Optional.Some(Addr(city=\"p\", zip=2)))
    print(r.to_yaml())
    auto r2 = Rec(name=\"b\", age=None, addr=None)
    print(r2.to_yaml())
";
    let f = write_zap("i1514-yaml-optional", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a top-level Optional Yaml field must lower cleanly; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("@Rec__to_yaml") && stdout.contains("@Rec__from_yaml"),
        "the Yaml codec is emitted for the struct: {stdout}"
    );
    // The defect: a call to the Optional instance's non-existent encoder. Note
    // `Optional$int64` / `Optional$Addr` appear legitimately as type names and
    // for the Optional helper methods (`__is_none` / `__unwrap` / …) — only a
    // `..__to_yaml` on an Optional instance is the miscompile.
    assert!(
        !stdout.contains("Optional$int64__to_yaml") && !stdout.contains("Optional$Addr__to_yaml"),
        "no call to a non-existent Optional$T__to_yaml is emitted (#1514): {stdout}"
    );
    // The Some arm encodes the nested struct payload through the real child
    // encoder; the None arm emits a literal `null` column.
    assert!(
        stdout.contains("@Addr__to_yaml"),
        "the Some arm of a nested Optional payload encodes through the child: {stdout}"
    );
}

#[test]
fn derive_yaml_rejects_a_nested_optional_payload_cleanly() {
    // #1514: a nested `Optional` *payload* (`Optional[Optional[T]]`) has no
    // value-position YAML form yet. It must fail with the clean "unsupported yet"
    // diagnostic (matching `@derive(Json)`), NOT fall into the `Nested` recursion
    // and emit a dangling symbol.
    let src = "\
@derive(Yaml)
struct Rec:
    Optional[Optional[int64]] opt

def main():
    auto r = Rec(opt=None)
    print(r.to_yaml())
";
    let f = write_zap("i1514-yaml-nested-optional", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(
        !out.status.success(),
        "a nested Optional Yaml payload must fail to compile"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("@derive(Yaml) does not support a nested Optional")
            && stderr.contains("yet"),
        "the error is the clean 'unsupported nested Optional payload' diagnostic: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "it is a clean diagnostic, not a panic: {stderr}"
    );
}

#[test]
fn reflect_set_field_and_field_desugar_inside_a_match_arm() {
    // #1506: a reflection intrinsic (`reflect.set_field` / `reflect.field`) used
    // inside a `match` case arm must be desugared like one at the top level of a
    // `comptime for` body. Before the fix the desugar walk recursed into
    // `if`/`while`/`for` blocks but fell through for `match`, so the `reflect`
    // receiver leaked to lowering and the program failed with a span-less
    // `error[ZAP6001]: unknown variable 'reflect'`. The shape below is the one a
    // DOM codec decoder uses: inspect a value with `match`, then populate a field
    // from within an arm.
    let src = "\
@derivable(Zero)
def _derive_zero[T](T self) -> int64:
    auto out = reflect.construct(T)
    int64 total = 0
    comptime for f in reflect.type_info(T).fields:
        if f.ty.name == \"int64\":
            match self:
                case _:
                    reflect.set_field(out, comptime f.index, 5)
                    total = total + reflect.field(out, comptime f.index)
    return total

interface Zero:
    def zero(self) -> int64

@derive(Zero)
struct S:
    int64 n

def main():
    print(S(1).zero())
";
    let f = write_zap("reflect-setfield-in-match-arm", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a reflect intrinsic inside a match arm must lower; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("unknown variable 'reflect'"),
        "the reflect receiver must not leak out of a match arm: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("fieldset struct") && !stdout.contains("call @reflect"),
        "the arm-local set_field/field desugar to ordinary field ops: {stdout}"
    );
}

#[test]
fn reflect_set_field_desugars_in_every_arm_of_an_optional_match() {
    // #1506, the realistic codec-decoder shape: both arms of an `Optional` match
    // write the field via `reflect.set_field` (the value is computed in the arm,
    // then set), and the scrutinee is a `reflect.field`-bound local. Every arm
    // body — and the scrutinee — must be desugared.
    let src = "\
@derivable(Zero)
def _derive_zero[T](T self) -> int64:
    auto out = reflect.construct(T)
    int64 total = 0
    comptime for f in reflect.type_info(T).fields:
        if f.ty.kind.name == \"optional\":
            Optional[int64] ov = reflect.field(self, comptime f.index)
            match ov:
                case Optional.Some(x):
                    reflect.set_field(out, comptime f.index, Optional.Some(x))
                    total = total + x
                case Optional.None:
                    reflect.set_field(out, comptime f.index, None)
    return total

interface Zero:
    def zero(self) -> int64

@derive(Zero)
struct S:
    Optional[int64] opt

def main():
    print(S(Optional.Some(7)).zero())
";
    let f = write_zap("reflect-setfield-in-both-match-arms", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "reflect.set_field in every arm of an Optional match must lower; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("unknown variable 'reflect'"),
        "no reflect receiver leaks out of any match arm: {stderr}"
    );
}

#[test]
fn derive_json_supports_list_fields() {
    // #1530 (epic #1529): the built-in `@derive(Json)` codec encodes a `list[T]`
    // field as a JSON array and decodes it back, for element types int / str /
    // nested @derive(Json) struct. Encode string-builds the array recursing into
    // the per-element encoder; decode reads the DOM array via the `__json_as_array`
    // prelude accessor and decodes each element. Round-trip output is pinned by
    // the golden `derive_json_list_field` under the CI LLVM matrix; here we assert
    // it lowers and wires the array accessor + element recursion.
    let src = "\
@derive(Json)
struct Item:
    int64 id
    str tag

@derive(Json)
struct Bag:
    list[int64] nums
    list[str] names
    list[Item] items

def main():
    auto b = Bag([1, 2, 3], [\"a\"], [Item(7, \"x\")])
    auto d = Bag.from_json(b.to_json())
    print(len(d.nums))
    print(d.items[0].id)
";
    let f = write_zap("derive-json-list-field", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a @derive(Json) struct with list fields must lower; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("@Bag__to_json") && stdout.contains("@Bag__from_json_value"),
        "the built-in list-field codec methods are emitted: {stdout}"
    );
    // Decode reads the DOM array through the prelude accessor...
    assert!(
        stdout.contains("__json_as_array"),
        "list decode reads the array via __json_as_array (#1530): {stdout}"
    );
    // ...and a nested-struct element recurses into its own codec both ways.
    assert!(
        stdout.contains("@Item__to_json") && stdout.contains("@Item__from_json_value"),
        "a nested-struct list element recurses into its own codec: {stdout}"
    );
}

#[test]
fn derive_json_rejects_unsupported_list_elements_and_other_collections_cleanly() {
    // #1530: list support is scoped to int/float/str/bytes/nested-struct and (as
    // of #1533) nested-collection elements. It does NOT extend to a bool/char
    // element (which collapses to a coarse i64 word) or to a value-position
    // `Optional` element (a separate follow-up); a `set` field also stays a
    // follow-up. Each must be a clean compile error, not a miscompile.
    let cases = [
        ("list[bool] x", "bool element"),
        ("list[Optional[int64]] x", "optional element"),
        ("set[int64] x", "set field"),
    ];
    for (field, what) in cases {
        let src = format!("@derive(Json)\nstruct S:\n    {field}\n\ndef main():\n    print(0)\n");
        let f = write_zap("derive-json-list-reject", &src);
        let out = zap().arg("ir").arg(&f).output().unwrap();
        assert!(
            !out.status.success(),
            "{what} must be a clean error, not success"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("@derive(Json)"),
            "{what} errors cleanly via the derive codec: {stderr}"
        );
    }
}

#[test]
fn derive_json_supports_dict_fields() {
    // #1531 (epic #1529): the built-in `@derive(Json)` codec encodes a
    // `dict[str, V]` field as a JSON object and decodes it back, for value types
    // int / str / nested @derive(Json) struct. Encode string-builds the object
    // recursing into the per-value encoder; decode reads the DOM object via the
    // `__json_as_object` prelude accessor and decodes each value keyed by the
    // object's own string keys. Round-trip output is pinned by the golden
    // `derive_json_dict_field` under the CI LLVM matrix.
    let src = "\
@derive(Json)
struct Item:
    int64 id

@derive(Json)
struct M:
    dict[str, int64] scores
    dict[str, Item] items

def main():
    auto m = M({\"a\": 1}, {\"i\": Item(7)})
    auto d = M.from_json(m.to_json())
    print(d.scores[\"a\"])
    print(d.items[\"i\"].id)
";
    let f = write_zap("derive-json-dict-field", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a @derive(Json) struct with dict[str,V] fields must lower; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("@M__to_json") && stdout.contains("@M__from_json_value"),
        "the built-in dict-field codec methods are emitted: {stdout}"
    );
    // Decode reads the DOM object through the prelude accessor + the str keys...
    assert!(
        stdout.contains("__json_as_object") && stdout.contains("zap_dict_keys_ptr"),
        "dict decode reads the object via __json_as_object + str keys (#1531): {stdout}"
    );
    // ...and a nested-struct value recurses into its own codec both ways.
    assert!(
        stdout.contains("@Item__to_json") && stdout.contains("@Item__from_json_value"),
        "a nested-struct dict value recurses into its own codec: {stdout}"
    );
}

#[test]
fn derive_json_rejects_non_str_dict_keys_and_unsupported_dict_values() {
    // #1531: a JSON object has string keys, so only `dict[str, V]` is supported —
    // a non-str key is a clean error. The value follows the same element allowlist
    // as a list, so a bool value or a value-position `Optional` is a clean error,
    // not a miscompile. (A nested-collection value — `dict[str, list[...]]` — is
    // supported as of #1533, so it is not a reject case.)
    let cases = [
        ("dict[int64, int64] m", "non-str key"),
        ("dict[str, bool] m", "bool value"),
        ("dict[str, Optional[int64]] m", "optional value"),
    ];
    for (field, what) in cases {
        let src = format!("@derive(Json)\nstruct S:\n    {field}\n\ndef main():\n    print(0)\n");
        let f = write_zap("derive-json-dict-reject", &src);
        let out = zap().arg("ir").arg(&f).output().unwrap();
        assert!(
            !out.status.success(),
            "{what} must be a clean error, not success"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("@derive(Json)"),
            "{what} errors cleanly via the derive codec: {stderr}"
        );
    }
}

#[test]
fn derive_yaml_supports_scalar_list_and_dict_fields() {
    // #1532 (epic #1529): @derive(Yaml) now encodes a `list[T]` / `dict[str,V]`
    // field in JSON flow (valid YAML, YAML 1.2 ⊃ JSON) and decodes it through the
    // shared `from_json_value` core (which gained list/dict in #1530/#1531), for
    // int / float / str / bytes element and value types. Round-trip output is
    // pinned by the golden `derive_yaml_list_dict_field` under the CI LLVM matrix.
    let src = "\
@derive(Yaml)
struct W:
    list[int64] nums
    dict[str, int64] scores

def main():
    auto w = W([1, 2], {\"k\": 5})
    auto d = W.from_yaml(w.to_yaml())
    print(len(d.nums))
    print(d.scores[\"k\"])
";
    let f = write_zap("derive-yaml-list-dict", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a @derive(Yaml) struct with scalar list/dict fields must lower; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("@W__to_yaml") && stdout.contains("@W__from_yaml"),
        "the built-in yaml collection codec methods are emitted: {stdout}"
    );
    // Encode reuses the JSON flow array/object builder; decode shares the JSON
    // DOM accessors through the from_json_value core.
    assert!(
        stdout.contains("__json_as_array") && stdout.contains("__json_as_object"),
        "yaml collection decode flows through the shared JSON DOM accessors (#1532): {stdout}"
    );
}

#[test]
fn derive_yaml_rejects_a_nested_struct_collection_element_cleanly() {
    // #1532: a nested-struct element/value in a yaml collection is a follow-up —
    // its flow encode would route through `{T}__to_json`, which @derive(Yaml) does
    // not emit, so it must be a clean compile error, not an unresolved symbol at
    // codegen (the #1514-class trap). A scalar element is fine (covered above).
    let src = "\
@derive(Yaml)
struct Item:
    int64 id

@derive(Yaml)
struct W:
    list[Item] items

def main():
    print(0)
";
    let f = write_zap("derive-yaml-nested-elem-reject", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(
        !out.status.success(),
        "a nested-struct yaml collection element must be a clean error"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("@derive(Yaml)") && stderr.contains("nested-struct"),
        "the nested-struct yaml element errors cleanly: {stderr}"
    );
}

#[test]
fn derive_json_supports_nested_collection_fields() {
    // #1533 (epic #1529): a collection whose element/value is itself a collection
    // — list[list[T]], dict[str, list[T]], list[dict[str, T]], and a struct leaf
    // (list[list[Nested]]). The codec recovers the nested element's full Ty from
    // the Elem::Nested interner (ctx.nested) and recurses through the same
    // per-element codec. Round-trip output is pinned by the golden
    // `derive_json_nested_collection_field` under the CI LLVM matrix.
    let src = "\
@derive(Json)
struct Item:
    int64 id

@derive(Json)
struct N:
    list[list[int64]] grid
    dict[str, list[int64]] groups
    list[list[Item]] items2d

def main():
    auto n = N([[1, 2]], {\"a\": [3]}, [[Item(9)]])
    auto d = N.from_json(n.to_json())
    print(d.grid[0][1])
    print(d.groups[\"a\"][0])
    print(d.items2d[0][0].id)
";
    let f = write_zap("derive-json-nested-collection", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a @derive(Json) struct with nested-collection fields must lower; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("@N__to_json") && stdout.contains("@N__from_json_value"),
        "the nested-collection codec methods are emitted: {stdout}"
    );
    // A struct leaf inside the nested collection recurses into its own codec.
    assert!(
        stdout.contains("@Item__to_json") && stdout.contains("@Item__from_json_value"),
        "a struct leaf in a nested collection recurses into its own codec (#1533): {stdout}"
    );
}

#[test]
fn derive_yaml_still_rejects_nested_collection_elements() {
    // #1533 lifts the nested-collection restriction for Json only. A yaml
    // collection's element flow-encodes and can bottom out in a struct leaf whose
    // `to_json` @derive(Yaml) does not emit, so a yaml nested-collection element
    // stays a clean compile error (scalar/str/bytes yaml collections are fine).
    let src = "\
@derive(Yaml)
struct W:
    list[list[int64]] grid

def main():
    print(0)
";
    let f = write_zap("derive-yaml-nested-collection-reject", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    assert!(
        !out.status.success(),
        "a yaml nested-collection element must be a clean error"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("@derive(Yaml)") && stderr.contains("nested-collection"),
        "the yaml nested-collection element errors cleanly: {stderr}"
    );
}

#[test]
fn the_serde_core_monomorphizes_serialize_deserialize_over_a_backend() {
    // E6.1 (#1562): the serde CORE — a format-agnostic `Serializer` /
    // `Deserializer` backend pair plus the shared generic `serialize` /
    // `deserialize` free functions — expresses (de)serialization as a PURE
    // LIBRARY over comptime reflection, with NO per-codec compiler change (the
    // payoff of epic #1248). The backend is a `[S: Serializer]` type parameter, so
    // dispatch is MONOMORPHIZED and UNBOXED (static, no fat pointer / vtable) and
    // the field walk is a compile-time `comptime for` over
    // `reflect.type_info(T).fields` — explicitly NOT runtime reflection. A
    // monomorphized backend cannot be threaded through a `@derive`'d method
    // (interface methods cannot be generic; the forwarding stub is non-generic),
    // so the core is free generic functions — the idiomatic Zap form. Runtime
    // round-trip agreement is pinned by the golden `serde_core_roundtrip` under the
    // CI LLVM matrix; here we assert the lowering specializes per (type, backend).
    let src = "\
interface Serializer:
    def begin_struct(self)
    def field_name(self, str name)
    def emit_int(self, int64 v)
    def emit_str(self, str v)
    def emit_bool(self, bool v)
    def end_struct(self)

interface Deserializer:
    def begin_struct(self)
    def read_int(self, str field) -> int64
    def read_str(self, str field) -> str
    def read_bool(self, str field) -> bool
    def end_struct(self)

def serialize[T, S: Serializer](T value, S s):
    s.begin_struct()
    comptime for f in reflect.type_info(T).fields:
        s.field_name(comptime f.name)
        if f.ty.name == \"int64\":
            s.emit_int(reflect.field(value, comptime f.index))
        elif f.ty.name == \"str\":
            s.emit_str(reflect.field(value, comptime f.index))
        elif f.ty.name == \"bool\":
            s.emit_bool(reflect.field(value, comptime f.index))
        elif f.ty.kind.name == \"struct\":
            serialize(reflect.field(value, comptime f.index), s)
        else:
            comptime.error(f.span, \"serialize: unsupported field kind\")
    s.end_struct()

def deserialize[T, D: Deserializer](D d) -> T:
    auto out = reflect.construct(T)
    d.begin_struct()
    comptime for f in reflect.type_info(T).fields:
        if f.ty.name == \"int64\":
            reflect.set_field(out, comptime f.index, d.read_int(comptime f.name))
        elif f.ty.name == \"str\":
            reflect.set_field(out, comptime f.index, d.read_str(comptime f.name))
        elif f.ty.name == \"bool\":
            reflect.set_field(out, comptime f.index, d.read_bool(comptime f.name))
        else:
            comptime.error(f.span, \"deserialize: unsupported field kind\")
    d.end_struct()
    return out

struct Sink:
    list[str] toks = []
    def begin_struct(self):
        self.toks.append(\"<\")
    def field_name(self, str name):
        pass
    def emit_int(self, int64 v):
        self.toks.append(str(v))
    def emit_str(self, str v):
        self.toks.append(v)
    def emit_bool(self, bool v):
        self.toks.append(str(v))
    def end_struct(self):
        self.toks.append(\">\")

struct Src:
    list[str] toks = []
    int64 pos = 0
    def begin_struct(self):
        pass
    def read_int(self, str field) -> int64:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return int(t)
    def read_str(self, str field) -> str:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return t
    def read_bool(self, str field) -> bool:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return t == \"True\"
    def end_struct(self):
        pass

struct Point:
    int64 x
    str label
    bool active

struct Outer:
    int64 id
    Point p

def main():
    auto p = Point(3, \"origin\", True)
    auto sink = Sink()
    serialize(p, sink)
    auto src = Src([\"3\", \"origin\", \"True\"], 0)
    auto p2 = deserialize[Point, Src](src)
    print(p2.x == 3 and p2.label == \"origin\" and p2.active == True)
    auto o = Outer(7, Point(1, \"q\", False))
    auto nsink = Sink()
    serialize(o, nsink)
    print(\",\".join(nsink.toks))
";
    let f = write_zap("e6-serde-core", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the serde core must lower; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The free generic drivers monomorphize per (type, backend) — a distinct
    // specialization for each, not one boxed path.
    assert!(
        stdout.contains("@serialize$Point$Sink"),
        "serialize specializes at (Point, Sink): {stdout}"
    );
    assert!(
        stdout.contains("@serialize$Outer$Sink"),
        "a nested struct field recurses into its own serialize specialization: {stdout}"
    );
    assert!(
        stdout.contains("@deserialize$Point$Src"),
        "deserialize specializes at (Point, Src): {stdout}"
    );
    // Backend methods resolve to DIRECT static calls (monomorphized, unboxed) —
    // no interface fat pointer / vtable.
    assert!(
        stdout.contains("@Sink__emit_int") && stdout.contains("@Src__read_int"),
        "backend methods are called directly through the [S: Serializer] bound: {stdout}"
    );
}

#[test]
fn a_binary_codec_is_a_pure_serde_backend_with_no_compiler_change() {
    // E6.2 (#1563): a REAL format backend validates the serde visitor model
    // (#1562) end-to-end. A compact self-describing TLV binary codec is
    // implemented purely as a `Serializer` / `Deserializer` backend and driven by
    // the SAME generic `serialize` / `deserialize` free functions — no compiler
    // change, no per-codec special-casing (the #1248 thesis). Byte-exact + round-
    // trip agreement is pinned by the golden `serde_binary_backend` under the CI
    // LLVM matrix; here we assert the lowering specializes the generic drivers per
    // (type, backend) and calls the backend methods directly (monomorphized).
    let src = "\
interface Serializer:
    def begin_struct(self)
    def field_name(self, str name)
    def emit_int(self, int64 v)
    def emit_str(self, str v)
    def emit_bool(self, bool v)
    def end_struct(self)

interface Deserializer:
    def begin_struct(self)
    def read_int(self, str field) -> int64
    def read_str(self, str field) -> str
    def read_bool(self, str field) -> bool
    def end_struct(self)

def serialize[T, S: Serializer](T value, S s):
    s.begin_struct()
    comptime for f in reflect.type_info(T).fields:
        s.field_name(comptime f.name)
        if f.ty.name == \"int64\":
            s.emit_int(reflect.field(value, comptime f.index))
        elif f.ty.name == \"str\":
            s.emit_str(reflect.field(value, comptime f.index))
        elif f.ty.name == \"bool\":
            s.emit_bool(reflect.field(value, comptime f.index))
        elif f.ty.kind.name == \"struct\":
            serialize(reflect.field(value, comptime f.index), s)
        else:
            comptime.error(f.span, \"serialize: unsupported field kind\")
    s.end_struct()

def deserialize[T, D: Deserializer](D d) -> T:
    auto out = reflect.construct(T)
    d.begin_struct()
    comptime for f in reflect.type_info(T).fields:
        if f.ty.name == \"int64\":
            reflect.set_field(out, comptime f.index, d.read_int(comptime f.name))
        elif f.ty.name == \"str\":
            reflect.set_field(out, comptime f.index, d.read_str(comptime f.name))
        elif f.ty.name == \"bool\":
            reflect.set_field(out, comptime f.index, d.read_bool(comptime f.name))
        else:
            comptime.error(f.span, \"deserialize: unsupported field kind\")
    d.end_struct()
    return out

struct BinWriter:
    list[int64] buf = []
    def begin_struct(self):
        self.buf.append(16)
    def field_name(self, str name):
        pass
    def emit_int(self, int64 v):
        self.buf.append(1)
        for k in range(8):
            auto shift = (7 - k) * 8
            self.buf.append((v >> shift) & 255)
    def emit_str(self, str v):
        self.buf.append(2)
        auto sb = bytes(v)
        self.buf.append(len(sb))
        for i in range(len(sb)):
            self.buf.append(sb[i])
    def emit_bool(self, bool v):
        self.buf.append(3)
        if v:
            self.buf.append(1)
        else:
            self.buf.append(0)
    def end_struct(self):
        self.buf.append(17)
    def done(self) -> bytes:
        return bytes(self.buf)

struct BinReader:
    bytes data = bytes([])
    int64 pos = 0
    def _byte(self) -> int64:
        auto b = self.data[self.pos]
        self.pos = self.pos + 1
        return b
    def begin_struct(self):
        auto _t = self._byte()
    def read_int(self, str field) -> int64:
        auto _t = self._byte()
        int64 v = 0
        for k in range(8):
            v = (v << 8) | self._byte()
        return v
    def read_str(self, str field) -> str:
        auto _t = self._byte()
        auto n = self._byte()
        list[int64] raw = []
        for i in range(n):
            raw.append(self._byte())
        return str(bytes(raw))
    def read_bool(self, str field) -> bool:
        auto _t = self._byte()
        auto b = self._byte()
        return b == 1
    def end_struct(self):
        auto _t = self._byte()

struct Point:
    int64 x
    str label
    bool active

struct Outer:
    int64 id
    Point p

def main():
    auto p = Point(3, \"hi\", True)
    auto w = BinWriter()
    serialize(p, w)
    auto wire = w.done()
    auto p2 = deserialize[Point, BinReader](BinReader(wire, 0))
    print(p2.x == 3 and p2.label == \"hi\" and p2.active == True)
    auto o = Outer(7, Point(1, \"q\", False))
    auto wo = BinWriter()
    serialize(o, wo)
    print(len(wo.done()))
";
    let f = write_zap("e6-serde-binary", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the binary serde backend must lower; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The generic drivers specialize per (type, backend) — a real codec is a pure
    // library over the serde core, no compiler edit.
    assert!(
        stdout.contains("@serialize$Point$BinWriter")
            && stdout.contains("@serialize$Outer$BinWriter"),
        "serialize specializes at the binary backend (incl. the nested field type): {stdout}"
    );
    assert!(
        stdout.contains("@deserialize$Point$BinReader"),
        "deserialize specializes at the binary backend: {stdout}"
    );
    // Backend methods are direct static calls — monomorphized, unboxed.
    assert!(
        stdout.contains("@BinWriter__emit_int") && stdout.contains("@BinReader__read_int"),
        "binary backend methods are called directly through the bound: {stdout}"
    );
}

#[test]
fn the_serde_core_is_a_bundled_pure_zap_stdlib_module() {
    // E6 (Option B, a D-0081 extension): the serde foundation ships BUNDLED with
    // the compiler as a pure-Zap stdlib module — `import serde` resolves it from
    // the compiler-embedded registry (crates/driver/src/bundled_stdlib.rs), with
    // no `zap add` / `zenv` lookup, so there is one blessed in-language version
    // for the whole ecosystem. Unlike a Rust seam, it is real Zap (interfaces +
    // generics + comptime reflection) merged like any module. This asserts that a
    // user program `import serde`s it and the generic drivers specialize across
    // the module boundary at the user's local backend + types; the runtime round
    // trip is pinned by the golden `serde_bundled_import` under the CI LLVM matrix.
    let src = "\
import serde

struct Sink:
    list[str] toks = []
    def begin_struct(self):
        self.toks.append(\"<\")
    def field_name(self, str name):
        pass
    def emit_int(self, int64 v):
        self.toks.append(str(v))
    def emit_float(self, float64 v):
        self.toks.append(str(v))
    def emit_str(self, str v):
        self.toks.append(v)
    def emit_bool(self, bool v):
        self.toks.append(str(v))
    def emit_bytes(self, bytes v):
        self.toks.append(str(v))
    def emit_uint(self, uint64 v):
        self.toks.append(str(v))
    def emit_nil(self):
        self.toks.append(\"nil\")
    def emit_variant(self, str name):
        self.toks.append(name)
    def begin_seq(self):
        self.toks.append(\"[\")
    def end_seq(self):
        self.toks.append(\"]\")
    def begin_map(self):
        self.toks.append(\"{\")
    def end_map(self):
        self.toks.append(\"}\")
    def end_struct(self):
        self.toks.append(\">\")

struct Src:
    list[str] toks = []
    int64 pos = 0
    def begin_struct(self):
        pass
    def read_int(self, str field) -> int64:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return int(t)
    def read_float(self, str field) -> float64:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return float(t)
    def read_str(self, str field) -> str:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return t
    def read_bool(self, str field) -> bool:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return t == \"True\"
    def read_bytes(self, str field) -> bytes:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return bytes(t)
    def read_uint(self, str field) -> uint64:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return uint64(int(t))
    def begin_optional(self, str field) -> bool:
        return True
    def read_variant(self) -> str:
        auto t = self.toks[self.pos]
        self.pos = self.pos + 1
        return t
    def begin_seq(self, str field) -> int64:
        return 0
    def end_seq(self):
        pass
    def begin_map(self, str field) -> int64:
        return 0
    def end_map(self):
        pass
    def end_struct(self):
        pass

struct Point:
    int64 x
    str label
    bool active

struct Coll:
    list[int64] nums
    dict[str, int64] tags
    uint64 big

enum Status:
    Active
    Paused
    Done

def main():
    auto p = Point(3, \"hi\", True)
    auto w = Sink()
    serde.serialize(p, w)
    print(\",\".join(w.toks))
    auto p2 = serde.deserialize[Point, Src](Src([\"3\", \"hi\", \"True\"], 0))
    print(p2.x == 3 and p2.label == \"hi\" and p2.active == True)
    auto c = Coll([1, 2], {\"k\": 9}, uint64(7))
    auto wc = Sink()
    serde.serialize(c, wc)
    print(\",\".join(wc.toks))
    auto ws = Sink()
    serde.serialize(Status.Paused, ws)
    print(\",\".join(ws.toks))
    auto st = serde.deserialize[Status, Src](Src([\"Active\"], 0))
    print(st)
";
    let f = write_zap("e6-serde-bundled", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    // `import serde` must resolve from the embedded registry (no file / zenv) and
    // the program must lower cleanly.
    assert!(
        out.status.success(),
        "`import serde` must resolve the bundled module and lower; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The bundled module merged under its own prefix and the generic drivers
    // specialized across the module boundary at the user's local backend + type.
    assert!(
        stdout.contains("serde__serialize$Point$Sink"),
        "serde.serialize specializes at (Point, user backend) across the module boundary: {stdout}"
    );
    assert!(
        stdout.contains("serde__deserialize$Point$Src"),
        "serde.deserialize specializes at (Point, user backend) across the module boundary: {stdout}"
    );
    // list / dict / uint64 breadth specializes too, and the sequence/map framing
    // dispatches to the backend directly.
    assert!(
        stdout.contains("serde__serialize$Coll$Sink"),
        "serde.serialize specializes at a struct with list/dict/uint fields: {stdout}"
    );
    assert!(
        stdout.contains("@Sink__begin_seq") && stdout.contains("@Sink__begin_map"),
        "list/dict framing calls the backend's begin_seq / begin_map directly: {stdout}"
    );
    // A constant (payload-free) enum specializes too: the driver dispatches on
    // kind == \"enum\" and reconstructs the variant by reflection on decode.
    assert!(
        stdout.contains("serde__serialize$Status$Sink"),
        "serde.serialize specializes at an enum across the module boundary: {stdout}"
    );
    assert!(
        stdout.contains("serde__deserialize$Status$Src"),
        "serde.deserialize specializes at an enum across the module boundary: {stdout}"
    );
    assert!(
        stdout.contains("@Sink__emit_variant") && stdout.contains("@Src__read_variant"),
        "enum (de)serialization drives the backend's emit_variant / read_variant: {stdout}"
    );
}

#[test]
fn comptime_for_dispatch_is_total_over_nameless_field_types() {
    // #1586: inside a `comptime for f in reflect.type_info(T).fields`, a dispatch
    // chain that compares `f.ty.name` must FOLD for every field kind, including
    // the nameless ones (list / dict / optional / tuple) whose `f.ty.name` is the
    // empty string. Previously `f.ty.name == <literal>` raised for a nameless type,
    // so the whole comptime `if` was left as a runtime `if` and the loop variable
    // leaked to lowering as `unknown variable 'f'`.
    let src = "\
def ser[T]():
    comptime for f in reflect.type_info(T).fields:
        if f.ty.name == \"int64\":
            print(\"int\")
        elif f.ty.kind.name == \"list\":
            print(\"list\")
        elif f.ty.kind.name == \"dict\":
            print(\"dict\")

struct R:
    dict[str, str] meta
    list[int64] nums
    int64 n

def main():
    ser[R]()
";
    let f = write_zap("comptime-nameless-dispatch", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a comptime dispatch that checks f.ty.name must lower even when a field is \
         nameless (list/dict); stderr: {stderr}"
    );
    assert!(
        !stderr.contains("unknown variable 'f'"),
        "the comptime for loop variable must not leak to lowering: {stderr}"
    );
}

#[test]
fn comptime_for_substitutes_loop_var_into_a_match_in_an_if_branch() {
    // #1584: a `match` nested inside an `if`/`elif` branch of a `comptime for` body
    // must still get the loop variable substituted. The failure mode was the same
    // root cause as #1586 — a preceding `f.ty.name == <literal>` check on a
    // nameless (optional) field did not fold, so the chain fell to the runtime-if
    // path and the branch holding the `match` lost `f`.
    let src = "\
def ser[T](T value):
    comptime for f in reflect.type_info(T).fields:
        if f.ty.name == \"int64\":
            print(str(reflect.field(value, comptime f.index)))
        elif f.ty.kind.name == \"optional\":
            Optional[int64] ov = reflect.field(value, comptime f.index)
            int64 got = -1
            match ov:
                case Optional.Some(x):
                    got = x
                case Optional.None:
                    got = -1
            print(comptime f.name + \"=\" + str(got))

struct T1:
    int64 a
    Optional[int64] b

def main():
    ser(T1(3, Optional.Some(9)))
";
    let f = write_zap("comptime-match-in-if", src);
    let out = zap().arg("ir").arg(&f).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "a match nested in an if branch of a comptime for must lower; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("unknown variable 'f'"),
        "the comptime for loop variable must reach the match branch: {stderr}"
    );
}
