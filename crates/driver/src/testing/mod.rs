//! The `zap test` implementation (D-0054, issue #73).
//!
//! The pieces split along the LLVM boundary so the host build (no backend) can
//! still unit-test everything mechanical:
//!
//! - [`discover`] — which files to look in.
//! - [`harness`] — the AST→AST transform that makes a test module runnable.
//! - [`report`] — the result model, stdout-sentinel parser, and renderers.
//! - [`strip_test_items`] — removes `@test` functions from a *normal* build, so
//!   release binaries never contain them (and never hit the deferred
//!   function-decorator error).
//! - [`run`] (LLVM only) — orchestrates discover → harness → compile → run →
//!   report, with `--json` and `--watch`.

pub mod discover;
/// Doctests: runnable examples extracted from docstrings (`zap test --doc`,
/// issue #234). The extraction + matching is host-safe; the runner needs the
/// REPL `Session`, so it is codegen-gated inside the module.
pub mod doctest;
pub mod harness;
pub mod report;

use zapc_syntax::ast::{Item, Module};

/// Remove every top-level `@test`/hook function from a module (D-0054): these
/// are compiled only under `zap test`, never into a release binary. Called on
/// the normal `run`/`build`/`ir` path.
pub fn strip_test_items(module: &mut Module) {
    module.items.retain(|item| match item {
        Item::Function(f) => !zapc_sema::is_test_item(f),
        _ => true,
    });
}

#[cfg(feature = "codegen")]
pub use run_impl::{run, run_json};

#[cfg(feature = "codegen")]
mod run_impl {
    use super::{discover, harness, report};
    use report::{Status, TestResult};
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitCode};
    use std::time::{Duration, SystemTime};

    /// `zap test [file] [--json] [--watch]`.
    pub fn run(file: Option<String>, json: bool, watch: bool) -> ExitCode {
        let files = discover::discover(file.as_deref());
        if files.is_empty() {
            eprintln!("zap test: no test files found (looked for *_test.zap and tests/)");
            return ExitCode::from(1);
        }

        if watch {
            return watch_loop(file.as_deref(), json);
        }

        let results = run_all(&files);
        emit(&results, json)
    }

    /// Run targeted tests and return the structured `--json` report plus an
    /// `ok` flag. Reused by `zap mcp`'s `zap_test` tool (#72): `paths` are the
    /// changed/affected files or directories to test; empty means the whole
    /// project. No output is printed — the JSON string is returned to the
    /// caller.
    pub fn run_json(paths: &[PathBuf]) -> (String, bool) {
        let mut files = Vec::new();
        if paths.is_empty() {
            files = discover::discover(None);
        } else {
            for p in paths {
                let s = p.to_string_lossy();
                files.extend(discover::discover(Some(&s)));
            }
        }
        files.sort();
        files.dedup();
        if files.is_empty() {
            return (report::format_json(&[]), true);
        }
        let results = run_all(&files);
        let ok = report::Summary::of(&results).ok();
        (report::format_json(&results), ok)
    }

    /// Compile and run every file, returning the flattened results.
    fn run_all(files: &[PathBuf]) -> Vec<TestResult> {
        let mut results = Vec::new();
        for f in files {
            run_file(f, &mut results);
        }
        results
    }

    /// Compile one file's harness and run it, appending its results.
    fn run_file(path: &Path, results: &mut Vec<TestResult>) {
        let display = path.display().to_string();
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                results.push(file_error(&display, format!("cannot read file: {e}")));
                return;
            }
        };

        // Front-end: a test file must parse and type-check like any source.
        let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(&src);
        if !lerrs.is_empty() || !perrs.is_empty() {
            let msg = lerrs
                .iter()
                .map(|e| format!("error[{}]: {}", e.code, e.message))
                .chain(
                    perrs
                        .iter()
                        .map(|e| format!("error[{}]: {}", e.code, e.message)),
                )
                .collect::<Vec<_>>()
                .join("\n");
            results.push(file_error(&display, msg));
            return;
        }
        let sema_errs = zapc_sema::check(&module);
        if !sema_errs.is_empty() {
            let msg = sema_errs
                .iter()
                .map(|e| format!("error[{}]: {}", e.code, e.message))
                .collect::<Vec<_>>()
                .join("\n");
            results.push(file_error(&display, msg));
            return;
        }

        let (harness_module, n_tests) = harness::build_harness(&module);
        if n_tests == 0 {
            return; // no @test functions here
        }

        // Compile the harness to a temporary executable.
        let exe = temp_exe(path);
        // Test harnesses always use the full (non-portable) link: they run on the
        // build host, where CPython and the dynamic libs are present (I#287).
        if let Err(e) = zapc_codegen::build_executable(&harness_module, &exe, false) {
            results.push(file_error(&display, format!("codegen error: {e}")));
            return;
        }

        let output = match Command::new(&exe).output() {
            Ok(o) => o,
            Err(e) => {
                results.push(file_error(&display, format!("failed to run tests: {e}")));
                let _ = std::fs::remove_file(&exe);
                return;
            }
        };
        let _ = std::fs::remove_file(&exe);

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut parsed = report::parse_output(&display, &stdout);

        // A test binary that exits non-zero crashed mid-test (an uncaught raise
        // escaping a test the harness could not frame, a hard fault, …). Surface
        // it so the run fails even if some RESULT lines were printed.
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = if stderr.trim().is_empty() {
                format!("test binary exited with status {}", output.status)
            } else {
                stderr.trim_end().to_string()
            };
            parsed.push(TestResult {
                file: display.clone(),
                name: "<crashed>".to_string(),
                status: Status::Fail,
                message: detail,
                output: String::new(),
            });
        }

        results.extend(parsed);
    }

    /// A synthetic failing result standing in for a whole-file error.
    fn file_error(file: &str, message: String) -> TestResult {
        TestResult {
            file: file.to_string(),
            name: "<file>".to_string(),
            status: Status::Fail,
            message,
            output: String::new(),
        }
    }

    /// A unique temp path for a file's test binary.
    fn temp_exe(src: &Path) -> PathBuf {
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("test");
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("zaptest-{stem}-{nonce}"))
    }

    /// Render and return an exit code for one run.
    fn emit(results: &[TestResult], json: bool) -> ExitCode {
        if json {
            print!("{}", report::format_json(results));
            let ok = report::Summary::of(results).ok();
            return if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            };
        }
        let (text, ok) = report::format_human(results);
        print!("{text}");
        if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        }
    }

    /// `--watch`: re-run on change. Watches every `.zap` under the current tree;
    /// a changed test file re-runs on its own, while a changed shared source
    /// re-runs everything (there is no dependency graph yet — flagged for #69).
    fn watch_loop(explicit: Option<&str>, json: bool) -> ExitCode {
        eprintln!("zap test --watch: watching for changes (Ctrl-C to stop)");
        let mut snapshot = scan_mtimes();
        // Initial run.
        let files = discover::discover(explicit);
        let _ = emit(&run_all(&files), json);
        loop {
            std::thread::sleep(Duration::from_millis(400));
            let current = scan_mtimes();
            let changed: Vec<PathBuf> = current
                .iter()
                .filter(|(p, t)| snapshot.get(*p) != Some(*t))
                .map(|(p, _)| p.clone())
                .collect();
            if changed.is_empty() {
                continue;
            }
            snapshot = current;

            let all = discover::discover(explicit);
            // If a changed path is itself a discovered test file, re-run just the
            // changed ones; otherwise a shared source changed, so re-run all.
            let changed_tests: Vec<PathBuf> = changed
                .iter()
                .filter(|c| all.contains(c))
                .cloned()
                .collect();
            let to_run = if changed_tests.len() == changed.len() && !changed_tests.is_empty() {
                changed_tests
            } else {
                all
            };
            eprintln!("\nzap test --watch: change detected, re-running…");
            let _ = emit(&run_all(&to_run), json);
        }
    }

    /// mtimes of every `.zap` file under the current directory.
    fn scan_mtimes() -> std::collections::BTreeMap<PathBuf, SystemTime> {
        let mut map = std::collections::BTreeMap::new();
        collect(Path::new("."), &mut map);
        map
    }

    fn collect(dir: &Path, map: &mut std::collections::BTreeMap<PathBuf, SystemTime>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if matches!(
                    name.as_ref(),
                    "target" | ".git" | ".claude" | "node_modules"
                ) || name.starts_with('.')
                {
                    continue;
                }
                collect(&path, map);
            } else if path.extension().and_then(|e| e.to_str()) == Some("zap") {
                if let Ok(mt) = entry.metadata().and_then(|m| m.modified()) {
                    map.insert(path, mt);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zapc_syntax::parser::parse_str;

    fn module(src: &str) -> Module {
        let (m, perrs, lerrs) = parse_str(src);
        assert!(
            perrs.is_empty() && lerrs.is_empty(),
            "parse errors: {perrs:?} {lerrs:?}"
        );
        m
    }

    #[test]
    fn strip_removes_tests_and_hooks_keeps_rest() {
        let mut m = module(
            "@test\ndef test_a():\n    assert True\n\
             @before\ndef setup():\n    print(1)\n\
             def helper():\n    return 1\n\
             def main():\n    print(2)\n",
        );
        strip_test_items(&mut m);
        let names: Vec<_> = m
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Function(f) => Some(f.name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["helper", "main"]);
    }

    #[test]
    fn harness_counts_tests_and_drops_user_main() {
        let m = module(
            "@test\ndef test_a():\n    assert True\n\
             @test\ndef test_b():\n    assert 1 == 1\n\
             def main():\n    print(99)\n",
        );
        let (h, n) = harness::build_harness(&m);
        assert_eq!(n, 2);
        // The synthesized main is present; the user's is gone, and the test
        // functions survive (decorator-free) so the harness can call them.
        let fns: Vec<_> = h
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Function(f) => Some((f.name.clone(), f.decorators.len())),
                _ => None,
            })
            .collect();
        assert!(fns.iter().any(|(n, _)| n == "main"));
        let user_mains = fns.iter().filter(|(n, _)| n == "main").count();
        assert_eq!(user_mains, 1, "exactly one (generated) main");
        assert!(fns.iter().any(|(n, d)| n == "test_a" && *d == 0));
        assert!(fns.iter().any(|(n, d)| n == "test_b" && *d == 0));
    }

    #[test]
    fn harness_lowers_and_typechecks() {
        // The generated harness must itself be a well-formed program.
        let m = module(
            "@test\ndef test_ok():\n    assert 2 + 2 == 4\n\
             @test\n@skip(\"later\")\ndef test_skip():\n    assert False\n\
             @before\ndef setup():\n    print(0)\n",
        );
        let (h, _n) = harness::build_harness(&m);
        assert!(zapc_sema::check(&h).is_empty(), "harness should type-check");
        let (_zir, errs) = zapc_ir::lower_module(&h);
        assert!(errs.is_empty(), "harness should lower: {errs:?}");
    }

    #[test]
    fn harness_parametrize_expands_cases() {
        let m = module(
            "@test\n@parametrize(cases=[(1, 2, 3), (4, 5, 9)])\n\
             def test_add(int64 a, int64 b, int64 want):\n    assert a + b == want\n",
        );
        let (h, n) = harness::build_harness(&m);
        assert_eq!(n, 1);
        assert!(zapc_sema::check(&h).is_empty());
        let (_zir, errs) = zapc_ir::lower_module(&h);
        assert!(
            errs.is_empty(),
            "parametrized harness should lower: {errs:?}"
        );
    }

    #[test]
    fn harness_expect_raises_lowers() {
        let m = module(
            "@test\n@expect_raises(ValueError)\ndef test_boom():\n    raise ValueError(\"nope\")\n",
        );
        let (h, _n) = harness::build_harness(&m);
        assert!(zapc_sema::check(&h).is_empty());
        let (_zir, errs) = zapc_ir::lower_module(&h);
        assert!(
            errs.is_empty(),
            "expect_raises harness should lower: {errs:?}"
        );
    }
}
