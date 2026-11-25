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

/// Front-end for a `zap test` entry file: run the SAME whole-program module
/// resolution as `zap run`/`zap build` (modlink → decorator desugar → sema;
/// I#283/D-0081, #1167) so that an imported dependency (installed in the project
/// `zenv`), a sibling source module, and their qualified exception types are all
/// in scope before the harness is built (#1232; D-0054, SPEC §16 — tests use the
/// same module resolution as builds). Returns the merged, import-qualified module
/// with `@test`/hook functions preserved (they survive the build front-end — only
/// a real `zap run`/`build` strips them, via [`strip_test_items`]), or a rendered
/// file-level diagnostic string.
///
/// Before this, `zap test` parsed the single entry file and type-checked it in
/// isolation, so any `import <dep>` resolved to nothing and the test body failed
/// at sema with `error[ZAP3001]: unknown name '<dep>'` — even though `zap run`
/// compiled the identical program. Routing through the one resolver fixes that
/// without a parallel, test-only import path.
///
/// Host-safe (no LLVM): the resolution and type-check happen entirely in the
/// front end, so the import-resolution fix is verifiable without a backend.
/// Gated to the paths that use it (the codegen-only runner and the tests) so the
/// no-codegen, non-test build does not warn it dead — like [`temp_exe_path`].
#[cfg(any(feature = "codegen", test))]
fn front_end_test_module(path: &std::path::Path) -> Result<(Module, zapc_sema::TypeMap), String> {
    let entry = path.display().to_string();
    // `zap test` takes no `--target-feature`s today, so the `when target.*` cfg is
    // resolved against the bare host target, exactly as a plain `zap build` would.
    let target = crate::build_target(&[]);
    match crate::front_end_reports(&entry, &target) {
        // The sema `TypeMap` is kept and threaded into the harness build (#1308,
        // #1310): monomorphization consumes it to pin a generic argument its own
        // best-effort inferer cannot derive (#1167), exactly as a plain `zap
        // build`/`zap run` does. Warnings (the third tuple field) are still not
        // surfaced on the `zap test` path: the stdout sentinel protocol is the
        // test report, and a warning is neither a test nor a failure. A hard
        // error is returned below.
        Ok((module, types, _warns)) => Ok((module, types)),
        Err(reports) => Err(render_front_end_errors(&reports)),
    }
}

/// Render front-end [`Report`](crate::Report)s into the one-error-per-line
/// `error[CODE]: message` form the file-level test result already uses, so a
/// resolution failure (an unknown dependency, a missing sibling module, an
/// unknown name) reports cleanly as a failing file rather than crashing the run.
#[cfg(any(feature = "codegen", test))]
fn render_front_end_errors(reports: &[crate::Report]) -> String {
    reports
        .iter()
        .map(|r| format!("error[{}]: {}", r.code, r.message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Remove every top-level `@test`/hook function from a module (D-0054): these
/// are compiled only under `zap test`, never into a release binary. Called on
/// the normal `run`/`build`/`ir` path.
pub fn strip_test_items(module: &mut Module) {
    module.items.retain(|item| match item {
        Item::Function(f) => !zapc_sema::is_test_item(f),
        _ => true,
    });
}

/// A process-unique path for a file's throwaway test binary, under the system
/// temp dir, named `zaptest-<stem>-<pid>-<seq>`.
///
/// The name combines the process id with a process-global `AtomicU64` sequence
/// and reads **no clock** (#917): the pid distinguishes concurrent `zap test`
/// processes and the counter distinguishes repeated calls within one process —
/// even for two entry files with the same stem. The previous
/// `zaptest-<stem>-<nanos>` name had neither pid nor counter, so two concurrent
/// runs (or one run compiling two same-named entry files) landing in the same
/// clock tick were handed the same throwaway binary path and clobbered each
/// other. Lives here (not in the codegen-gated `run_impl`) so its uniqueness is
/// guarded without an LLVM build; gated to the paths that use it so the
/// no-codegen non-test build does not warn it dead.
#[cfg(any(feature = "codegen", test))]
fn temp_exe_path(src: &std::path::Path) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("test");
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("zaptest-{stem}-{}-{seq}", std::process::id()))
}

#[cfg(test)]
mod temp_exe_tests {
    use super::temp_exe_path;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Barrier, Mutex};

    /// Issue #917 scenario guard: two (here, many) threads compiling entry files
    /// with the *same* stem under a barrier must be handed distinct throwaway
    /// binary paths. The old `zaptest-<stem>-<nanos>` name had no pid and no
    /// counter, so on a microsecond-quantised clock these collided and clobbered
    /// each other. With pid + process-global counter every path is distinct, so
    /// this passes deterministically; a regression to clock-based naming fails it
    /// here instead of flaking a real `zap test` run.
    #[test]
    fn same_stem_exe_paths_are_distinct_under_parallel_allocation() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 256;

        let barrier = Arc::new(Barrier::new(THREADS));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    (0..PER_THREAD)
                        // Every thread uses the identical entry stem, so only the
                        // pid+seq suffix can tell the paths apart — exactly the
                        // same-file, same-tick collision the bug produced.
                        .map(|_| temp_exe_path(Path::new("same_entry.zap")))
                        .collect::<Vec<_>>()
                })
            })
            .collect();

        let seen: Arc<Mutex<HashSet<PathBuf>>> = Arc::new(Mutex::new(HashSet::new()));
        for h in handles {
            let paths = h.join().expect("allocation thread panicked");
            let mut guard = seen.lock().unwrap();
            for p in paths {
                assert!(
                    guard.insert(p.clone()),
                    "temp exe path collided for same stem under parallel allocation: {}",
                    p.display()
                );
            }
        }
        assert_eq!(seen.lock().unwrap().len(), THREADS * PER_THREAD);
    }

    /// The generated name keeps the `zaptest-<stem>-` prefix so anything that
    /// greps for throwaway test binaries still matches.
    #[test]
    fn name_keeps_zaptest_stem_prefix() {
        let p = temp_exe_path(Path::new("/some/dir/widget_test.zap"));
        let name = p.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("zaptest-widget_test-"),
            "prefix/stem lost: {name}"
        );
    }
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

    /// `zap test [file] [--json] [--watch] [--race]`.
    ///
    /// `race` enables the runtime data-race detector (#665, D-0114; epic #661):
    /// the test harness is built with the shared-access instrumentation (#677) so
    /// a `@test` that drives racing coroutines is checked at run time. The hooks
    /// are no-op stubs until the happens-before engine lands (#678), so `--race`
    /// runs the suite identically for now.
    pub fn run(file: Option<String>, json: bool, watch: bool, race: bool) -> ExitCode {
        let files = discover::discover(file.as_deref());
        if files.is_empty() {
            eprintln!("zap test: no test files found (looked for *_test.zap and tests/)");
            return ExitCode::from(1);
        }

        if watch {
            return watch_loop(file.as_deref(), json, race);
        }

        let results = run_all(&files, race);
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
        // The `zap mcp` tool surface has no `--race` flag, so run without it.
        let results = run_all(&files, false);
        let ok = report::Summary::of(&results).ok();
        (report::format_json(&results), ok)
    }

    /// Compile and run every file, returning the flattened results. `race`
    /// builds each harness with the data-race instrumentation (#677).
    fn run_all(files: &[PathBuf], race: bool) -> Vec<TestResult> {
        let mut results = Vec::new();
        for f in files {
            run_file(f, &mut results, race);
        }
        results
    }

    /// Compile one file's harness and run it, appending its results. `race`
    /// builds the harness with the data-race instrumentation (#677).
    fn run_file(path: &Path, results: &mut Vec<TestResult>, race: bool) {
        let display = path.display().to_string();

        // Front-end: a test file must parse, resolve its imports, and type-check
        // like any `zap run`/`zap build` program (#1232; D-0054, SPEC §16). This
        // runs the SAME whole-program module resolution — so an imported
        // dependency from the project `zenv`, a sibling source module, and their
        // qualified exception types are in scope — and surfaces any lex/parse/
        // resolution/sema error as a single failing file result (unchanged shape).
        let (module, types) = match super::front_end_test_module(path) {
            Ok(m) => m,
            Err(msg) => {
                results.push(file_error(&display, msg));
                return;
            }
        };

        let (harness_module, n_tests) = harness::build_harness(&module);
        if n_tests == 0 {
            return; // no @test functions here
        }

        // Compile the harness to a temporary executable.
        let exe = super::temp_exe_path(path);
        // Entry source for located runtime diagnostics (#1493): a null-deref
        // `NullReferenceError` in a test body carries `... at <file>:<line>`,
        // resolved against the test file's source. Best-effort (unreadable →
        // `<unknown>:0`).
        let entry_src = std::fs::read_to_string(path).unwrap_or_default();
        let loc = (!entry_src.is_empty()).then_some(zapc_ir::SourceLoc {
            file: display.as_str(),
            src: entry_src.as_str(),
        });
        // Test harnesses always use the full (non-portable) link: they run on the
        // build host, where CPython and the dynamic libs are present (I#287). The
        // harness binary builds with the optimized (`-O2`) runtime — the default,
        // not `--debug` — since it is executed, not iterated on (I#867/D-0157).
        if let Err(e) = zapc_codegen::build_executable(
            &harness_module,
            // Thread sema's `TypeMap` into the harness build (#1308, #1310). The
            // `@test` harness is an AST→AST transform, but it *clones* the user's
            // test/hook/helper function bodies verbatim (only shedding their
            // decorators), so every user expression keeps its original span and
            // still keys into this map. Monomorphization consumes those types to
            // pin a generic argument its own best-effort inferer cannot derive
            // (#1167) — e.g. `Optional.Some(int64(0))` (#1308) or a generic
            // reflected operator over a built-in result like `date + Delta(1)`
            // (#1310) — exactly as a plain `zap build`/`zap run` does. Before this
            // the harness passed an empty map, so those forms fell back to the
            // pre-#1167 inference and failed to lower with ZAP6001 in `zap test`
            // even though the identical program compiled under `zap run`. The
            // harness-synthesized `main`/wrappers carry no entries here (their
            // spans are not in the map) and still fall back to mono inference, so
            // this is a pure augmentation on the user-body expressions.
            &types,
            &exe,
            false,
            race,
            false,
            zapc_ir::FpPolicy::default(),
            loc,
        ) {
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
    fn watch_loop(explicit: Option<&str>, json: bool, race: bool) -> ExitCode {
        eprintln!("zap test --watch: watching for changes (Ctrl-C to stop)");
        let mut snapshot = scan_mtimes();
        // Initial run.
        let files = discover::discover(explicit);
        let _ = emit(&run_all(&files, race), json);
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
            let _ = emit(&run_all(&to_run, race), json);
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

    /// #1308 / #1310: the native `@test` harness must lower with sema's inferred
    /// types so a generic argument the monomorphizer's own best-effort inferer
    /// cannot pin (#1167) still resolves — exactly as `zap run`/`zap build` do.
    ///
    /// Asserts `src` (which must contain at least one `@test`) type-checks, that
    /// the harness module built from it lowers cleanly **with** sema's `TypeMap`,
    /// and that the identical harness module **without** it still fails to lower
    /// with `ZAP6001` — proving the threaded `TypeMap` (not some other change) is
    /// what closes the gap, and guarding against a regression to the old empty-map
    /// behavior. Host-only (no LLVM): the harness build is an AST transform and
    /// lowering runs the mono pass. The `build_harness` clone preserves the test
    /// body's spans, so sema's types key into the harness module unchanged.
    fn harness_needs_sema_types(src: &str) {
        use zapc_ir::{lower_module, lower_module_opts_with_types};
        use zapc_sema::analyze_types;

        let m = module(src);
        let (serrs, _warns, types) = analyze_types(&m);
        assert!(serrs.is_empty(), "sema errors: {serrs:?}");

        let (harness_module, n_tests) = super::harness::build_harness(&m);
        assert!(n_tests >= 1, "fixture must contain at least one @test");

        let (_zir, with_errs) = lower_module_opts_with_types(&harness_module, false, &types);
        assert!(
            with_errs.is_empty(),
            "harness must lower WITH sema types (the #1308/#1310 fix): {with_errs:?}"
        );

        let (_zir, without_errs) = lower_module(&harness_module);
        assert!(
            !without_errs.is_empty(),
            "harness lowered WITHOUT sema types unexpectedly — this fixture no \
             longer guards the TypeMap-threading path (the monomorphizer's own \
             inferer now pins the argument); pick a form it cannot infer"
        );
    }

    #[test]
    fn harness_infers_optional_some_from_explicit_int64_cast() {
        // #1308's exact reproduction: `Optional.Some(int64(0))` in a @test body.
        // `int64(0)` is an explicit conversion whose result type the mono inferer
        // does not derive; sema's `TypeMap` pins it as `Optional[int64]`.
        harness_needs_sema_types(
            "@test\ndef explicit_integer_cast():\n\
             \x20   auto value = Optional.Some(int64(0))\n\
             \x20   assert value.is_some()\n",
        );
    }

    #[test]
    fn harness_infers_optional_some_from_explicit_cast_in_kwarg() {
        // #1308 also calls out the typed keyword-argument form: the same explicit
        // conversion flowing through a `Some(value=...)` keyword argument.
        harness_needs_sema_types(
            "@test\ndef explicit_integer_cast_kwarg():\n\
             \x20   auto value = Optional.Some(value=int64(0))\n\
             \x20   assert value.is_some()\n",
        );
    }

    #[test]
    fn harness_infers_generic_arg_from_builtin_len_result() {
        // The #1310 family: a generic argument whose type is a built-in call's
        // result (`len`), which the mono inferer loses but sema records. Covers
        // the "built-in result" axis host-only (the reflected-operator / datetime
        // shape #1310 cites needs an installed package, exercised by the golden
        // `zap test` fixtures in CI).
        harness_needs_sema_types(
            "@test\ndef builtin_result():\n\
             \x20   list[int64] xs = [1, 2, 3]\n\
             \x20   auto value = Optional.Some(len(xs))\n\
             \x20   assert value.is_some()\n",
        );
    }

    #[test]
    fn harness_infers_generic_reflected_operator_over_builtin_result() {
        // #1310's shape, self-contained: a generic reflected operator
        // (`Delta.__radd__[T]`) whose left operand is a built-in call's result
        // (`len(xs)`). The mono inferer loses the built-in result type and bails
        // with the issue's exact "cannot infer type argument 'T' of operator
        // method '__radd__'"; sema's `TypeMap` pins `T = int64`. (The datetime
        // `date + Delta(1)` form the issue cites is the same mechanism over an
        // installed-package seam call, exercised end-to-end in `zap_cli`.)
        harness_needs_sema_types(
            "value struct Delta:\n\
             \x20   int64 n\n\n\
             \x20   def __radd__[T](self, T x) -> T:\n\
             \x20       return x\n\n\
             @test\ndef reflected_over_builtin():\n\
             \x20   list[int64] xs = [1, 2, 3]\n\
             \x20   auto result = len(xs) + Delta(1)\n\
             \x20   assert result == 3\n",
        );
    }

    #[test]
    fn harness_infers_generic_arg_in_parametrized_test_case() {
        // `@parametrize` reuses the case expression nodes directly in synthesized
        // call sites; those nodes keep their original spans, so sema's type for a
        // hard-to-infer argument still keys through the harness transform.
        harness_needs_sema_types(
            "@parametrize(cases=[[1], [2]])\n\
             @test\ndef param_case(int64 k):\n\
             \x20   auto value = Optional.Some(int64(k))\n\
             \x20   assert value.is_some()\n",
        );
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

    // ---- module-level constants in the harness (#1236) -------------------
    //
    // A module-level `const`/mutable binding (D-0112) is persistent module state
    // that lowers to a module global; the harness must carry it over so a `@test`
    // (or a function it calls, or a default argument) can read it. Before the
    // fix, `build_harness` dropped every top-level `Item::Stmt`, so codegen
    // reported the name as an unknown variable even though sema accepted the
    // module. These assert the binding survives into the harness AND lowers
    // (AST→ZIR is host-safe and is exactly where the old `ZAP6001: unknown
    // variable` surfaced — no LLVM needed); the native run is covered in CI.

    /// The bindings a module const lowers to must be present in the harness.
    fn harness_keeps_binding(h: &Module, name: &str) -> bool {
        h.items.iter().any(
            |it| matches!(it, Item::Stmt(zapc_syntax::ast::Stmt::Let { name: n, .. }) if n == name),
        )
    }

    #[test]
    fn harness_keeps_scalar_module_const_and_lowers() {
        let m = module("const int64 ANSWER = 42\n@test\ndef t():\n    assert ANSWER == 42\n");
        let (h, n) = harness::build_harness(&m);
        assert_eq!(n, 1);
        assert!(
            harness_keeps_binding(&h, "ANSWER"),
            "the module const must be carried into the harness"
        );
        assert!(zapc_sema::check(&h).is_empty(), "harness should type-check");
        let (_zir, errs) = zapc_ir::lower_module(&h);
        assert!(
            errs.is_empty(),
            "scalar const harness should lower: {errs:?}"
        );
    }

    #[test]
    fn harness_keeps_mutable_module_binding_and_lowers() {
        // D-0112 Phase 2 (#662): a mutable module-level binding is also persistent
        // state and must survive into the harness, not just a `const`.
        let m = module("int64 counter = 7\n@test\ndef t():\n    assert counter == 7\n");
        let (h, n) = harness::build_harness(&m);
        assert_eq!(n, 1);
        assert!(harness_keeps_binding(&h, "counter"));
        assert!(zapc_sema::check(&h).is_empty());
        let (_zir, errs) = zapc_ir::lower_module(&h);
        assert!(
            errs.is_empty(),
            "mutable binding harness should lower: {errs:?}"
        );
    }

    #[test]
    fn harness_keeps_struct_value_module_const_with_reference_field() {
        // A value struct holding a REFERENCE field (a `str`), the timezone/Easter
        // shape called out in #1236: `const TZ UTC = TZ("UTC")`.
        let m = module(
            "struct TZ:\n    str name\n\
             const TZ UTC = TZ(\"UTC\")\n\
             @test\ndef reads_struct_const():\n    assert UTC.name == \"UTC\"\n",
        );
        let (h, n) = harness::build_harness(&m);
        assert_eq!(n, 1);
        assert!(harness_keeps_binding(&h, "UTC"));
        assert!(zapc_sema::check(&h).is_empty());
        let (_zir, errs) = zapc_ir::lower_module(&h);
        assert!(
            errs.is_empty(),
            "struct const harness should lower: {errs:?}"
        );
    }

    #[test]
    fn harness_function_reading_module_const_called_from_test_lowers() {
        let m = module(
            "const int64 BASE = 10\n\
             def scaled(int64 x) -> int64:\n    return x * BASE\n\
             @test\ndef calls_fn_reading_const():\n    assert scaled(4) == 40\n",
        );
        let (h, _n) = harness::build_harness(&m);
        assert!(harness_keeps_binding(&h, "BASE"));
        assert!(zapc_sema::check(&h).is_empty());
        let (_zir, errs) = zapc_ir::lower_module(&h);
        assert!(
            errs.is_empty(),
            "fn-reading-const harness should lower: {errs:?}"
        );
    }

    #[test]
    fn harness_default_argument_referencing_module_const_lowers() {
        // A default argument that references a module const (#1236 acceptance).
        let m = module(
            "const int64 FALLBACK = 99\n\
             def get(int64 x = FALLBACK) -> int64:\n    return x\n\
             @test\ndef uses_default_from_const():\n    assert get() == 99\n",
        );
        let (h, _n) = harness::build_harness(&m);
        assert!(harness_keeps_binding(&h, "FALLBACK"));
        assert!(zapc_sema::check(&h).is_empty());
        let (_zir, errs) = zapc_ir::lower_module(&h);
        assert!(
            errs.is_empty(),
            "default-arg-const harness should lower: {errs:?}"
        );
    }

    #[test]
    fn harness_preserves_initialization_order_of_dependent_consts() {
        // One const initialized from another: both must be carried over in source
        // order so the dependent initializer sees the earlier value.
        let m = module(
            "const int64 A = 2\nconst int64 B = A + 1\n\
             @test\ndef ordered():\n    assert B == 3\n",
        );
        let (h, _n) = harness::build_harness(&m);
        // The two bindings appear in source order, A before B.
        let let_names: Vec<String> = h
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Stmt(zapc_syntax::ast::Stmt::Let { name, .. }) => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(let_names, vec!["A", "B"], "init order must be preserved");
        assert!(zapc_sema::check(&h).is_empty());
        let (_zir, errs) = zapc_ir::lower_module(&h);
        assert!(
            errs.is_empty(),
            "dependent-const harness should lower: {errs:?}"
        );
    }

    #[test]
    fn harness_still_drops_module_docstring_and_has_no_stray_statements() {
        // Only module-level bindings survive; a bare module docstring (a
        // non-executable top-level statement) is still dropped, and no bare
        // statement leaks into the harness.
        let m = module("\"module doc\"\nconst int64 K = 1\n@test\ndef t():\n    assert K == 1\n");
        let (h, _n) = harness::build_harness(&m);
        assert!(harness_keeps_binding(&h, "K"));
        let non_let_stmts = h
            .items
            .iter()
            .filter(|it| {
                matches!(it, Item::Stmt(s) if !matches!(s, zapc_syntax::ast::Stmt::Let { .. }))
            })
            .count();
        assert_eq!(non_let_stmts, 0, "no non-binding statement should survive");
        assert!(zapc_sema::check(&h).is_empty());
        let (_zir, errs) = zapc_ir::lower_module(&h);
        assert!(errs.is_empty());
    }

    #[test]
    fn harness_unknown_name_without_a_matching_const_still_errors() {
        // Negative: the fix carries real bindings over; it does not invent them.
        // A reference to a name with no declaration must still fail (here at sema,
        // ZAP3001 — before lowering is even reached).
        let m = module("@test\ndef t():\n    assert MISSING == 1\n");
        let errs = zapc_sema::check(&m);
        assert!(
            errs.iter().any(|e| e.code == zapc_diag::ErrorCode::ZAP3001),
            "an undefined name must still report ZAP3001, got: {errs:?}"
        );
    }

    #[test]
    fn strip_test_items_keeps_module_consts_for_ordinary_builds() {
        // Ordinary `zap run`/`build` behavior is unchanged: stripping `@test`
        // items leaves module-level bindings in place (they are not test items).
        let mut m = module(
            "const int64 KEEP = 5\n\
             @test\ndef t():\n    assert KEEP == 5\n\
             def main():\n    print(KEEP)\n",
        );
        strip_test_items(&mut m);
        assert!(
            harness_keeps_binding(&m, "KEEP"),
            "a normal build must retain module consts after @test stripping"
        );
        let test_fns = m
            .items
            .iter()
            .filter(|it| matches!(it, Item::Function(f) if f.name == "t"))
            .count();
        assert_eq!(
            test_fns, 0,
            "the @test function must be stripped from a build"
        );
    }
}

/// `zap test` import-resolution (#1232; D-0054, SPEC §16 — tests use the same
/// module resolution as builds).
///
/// These build a throwaway project tree on disk and drive the exact front-end
/// `zap test`'s `run_file` now uses ([`front_end_test_module`] →
/// [`crate::front_end_reports_with`]), asserting that an imported dependency
/// installed in the project `zenv`, a sibling source module, and qualified
/// exception types raised/caught across a module boundary are all in scope — and
/// that a genuinely unresolved import or unknown name still fails cleanly as a
/// file-level error, with test discovery and reporting unchanged. No LLVM is
/// needed: resolution and type-checking are a front-end concern, so the fix is
/// verified here; the full native harness run is exercised in CI.
#[cfg(test)]
mod import_resolution_tests {
    use super::{front_end_test_module, harness, render_front_end_errors};
    use std::io::Write;
    use std::path::PathBuf;
    use zapc_syntax::ast::{Item, Module};
    use zapc_syntax::cfg::Target;

    /// A throwaway project directory under the OS temp dir, cleaned on drop.
    /// Mirrors the module-linker tests' helper so the two suites build trees the
    /// same way (pid+counter naming, no clock — #917).
    struct Project {
        dir: PathBuf,
    }

    impl Project {
        fn new(tag: &str) -> Self {
            let dir = crate::testutil::unique_temp_dir(&format!("zap-zaptest-{tag}"));
            Project { dir }
        }

        fn write(&self, rel: &str, src: &str) -> PathBuf {
            let path = self.dir.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(src.as_bytes()).unwrap();
            path
        }

        /// The project `zenv`'s package-search root (`<root>/zenv/pkg`), the same
        /// directory `ZAP_ZENV=<root>/zenv` resolves to at runtime.
        fn pkg_root(&self) -> PathBuf {
            self.dir.join("zenv").join("pkg")
        }
    }

    impl Drop for Project {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Resolve + type-check an entry file through the SAME build front-end, with
    /// an explicit `zenv` package root so the process-wide `ZAP_ZENV` is never
    /// touched (parallel-test-safe). Returns the merged, import-qualified module.
    fn resolve(entry: &std::path::Path, pkg_roots: Vec<PathBuf>) -> Result<Module, Vec<String>> {
        crate::front_end_reports_with(&entry.display().to_string(), &Target::host(), pkg_roots)
            .map(|(module, _types, _warns)| module)
            .map_err(|reports| {
                reports
                    .iter()
                    .map(|r| format!("error[{}]: {}", r.code, r.message))
                    .collect()
            })
    }

    /// Resolve an entry file AND lower the merged module to ZIR, returning the
    /// rendered ZIR text. Lowering runs monomorphization (reading sema's inferred
    /// types), so a generic instantiated by a type value — e.g. `option[T]` bound
    /// at `Type[int64]` — appears here as the mangled `option$Type$int64` (it is
    /// still the bare generic in the pre-lowering AST module `resolve` returns).
    fn resolve_and_lower_zir(entry: &std::path::Path, pkg_roots: Vec<PathBuf>) -> String {
        let (module, types, _warns) = match crate::front_end_reports_with(
            &entry.display().to_string(),
            &Target::host(),
            pkg_roots,
        ) {
            Ok(ok) => ok,
            Err(reports) => {
                let msgs: Vec<String> = reports
                    .iter()
                    .map(|r| format!("error[{}]: {}", r.code, r.message))
                    .collect();
                panic!("front end must resolve, got: {msgs:?}");
            }
        };
        let (zir, errs) = zapc_ir::lower_module_opts_with_types(&module, false, &types);
        assert!(errs.is_empty(), "the program must lower, got: {errs:?}");
        format!("{zir}")
    }

    /// Whether any top-level function's name CONTAINS `needle` — used to prove an
    /// imported dependency's symbols were actually merged into the linked module
    /// (they arrive module-qualified/mangled, e.g. `__zm1_sample__answer__`, not
    /// under their bare source name).
    fn has_fn_containing(m: &Module, needle: &str) -> bool {
        m.items.iter().any(|it| match it {
            Item::Function(f) => f.name.contains(needle),
            _ => false,
        })
    }

    /// Assert the merged, import-qualified test module both type-checks as linked
    /// AND survives the harness transform + lowering with `n` tests — i.e. every
    /// cross-module reference resolved all the way through to runnable ZIR.
    fn assert_harness_ok(module: &Module, n: usize) {
        let (h, got) = harness::build_harness(module);
        assert_eq!(got, n, "discovered {got} @test fns, expected {n}");
        let sema = zapc_sema::check(&h);
        assert!(sema.is_empty(), "harness should type-check, got: {sema:?}");
        let (_zir, errs) = zapc_ir::lower_module(&h);
        assert!(errs.is_empty(), "harness should lower, got: {errs:?}");
    }

    // ---- the pre-#1232 bug, pinned ---------------------------------------

    /// Regression guard for the actual defect: the OLD `zap test` front end
    /// parsed the entry file and type-checked it in ISOLATION
    /// (`parse_str` + `zapc_sema::check`), so an imported dependency resolved to
    /// nothing and the body failed at sema with `unknown name`. Pinning the old
    /// behaviour here documents *why* the fix is needed and fails loudly if a
    /// future change reintroduces the single-file path.
    #[test]
    fn old_single_file_check_fails_on_imported_dependency_symbol() {
        let src = "import sample\n\
                   @test\ndef uses_dep():\n    assert sample.answer() == 42\n";
        let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
        assert!(perrs.is_empty() && lerrs.is_empty(), "should parse");
        let errs = zapc_sema::check(&module);
        assert!(
            errs.iter().any(|e| e.code == zapc_diag::ErrorCode::ZAP3001),
            "the single-file check must still report ZAP3001 'unknown name' for an \
             unresolved import (this is the bug the module-resolution fix cures), got: {errs:?}"
        );
    }

    // ---- positive cases --------------------------------------------------

    /// A dependency installed at a LOCAL PATH (`zap add --path`) — present in the
    /// project `zenv` as `<zenv>/pkg/<name>/src/<name>.zap` — is resolvable from a
    /// `@test`, imported symbol and all. This is the exact reproducer in #1232.
    #[test]
    fn local_path_dependency_symbol_resolves_in_test() {
        let p = Project::new("localdep");
        p.write(
            "zenv/pkg/sample/src/sample.zap",
            "def answer() -> int64:\n    return 42\n",
        );
        let entry = p.write(
            "tests/sample_test.zap",
            "import sample\n\
             @test\ndef answer_from_dependency():\n    assert sample.answer() == 42\n",
        );

        let module = resolve(&entry, vec![p.pkg_root()]).expect("dependency import must resolve");
        assert!(
            has_fn_containing(&module, "answer"),
            "the dependency's merged (mangled) `answer` symbol must be present in the linked module"
        );
        assert_harness_ok(&module, 1);
    }

    /// A SIBLING source module in the same project (resolved relative to the test
    /// file's own package source root) is in scope from a `@test`.
    #[test]
    fn sibling_source_module_resolves_in_test() {
        let p = Project::new("sibling");
        p.write(
            "tests/helpers.zap",
            "def double(int64 x) -> int64:\n    return x * 2\n",
        );
        let entry = p.write(
            "tests/sibling_test.zap",
            "import helpers\n\
             @test\ndef sibling_in_scope():\n    assert helpers.double(21) == 42\n",
        );

        // No dependency roots at all: a sibling must resolve locally, not via zenv.
        let module = resolve(&entry, Vec::new()).expect("sibling import must resolve");
        assert_harness_ok(&module, 1);
    }

    /// A QUALIFIED exception type defined in an imported dependency is usable in a
    /// `@test` body: raised and caught across the module boundary. The linker must
    /// rewrite the qualified `raise`/`except` type to the dependency's merged
    /// symbol, or sema rejects it (the "rejects its qualified exception types"
    /// half of #1232).
    #[test]
    fn qualified_exception_from_dependency_raises_and_catches_in_test() {
        let p = Project::new("qualexc");
        p.write(
            "zenv/pkg/sample/src/sample.zap",
            "@exception\nstruct SampleError:\n    str message\n\
             def boom():\n    raise SampleError(\"from dep\")\n",
        );
        let entry = p.write(
            "tests/exc_test.zap",
            "import sample\n\
             @test\ndef catches_dep_exception():\n\
             \x20   int64 caught = 0\n\
             \x20   try:\n\
             \x20       raise sample.SampleError(\"boom\")\n\
             \x20   except sample.SampleError as e:\n\
             \x20       caught = 1\n\
             \x20   assert caught == 1\n\
             @test\ndef catches_exception_raised_inside_dependency():\n\
             \x20   int64 caught = 0\n\
             \x20   try:\n\
             \x20       sample.boom()\n\
             \x20   except sample.SampleError as e:\n\
             \x20       caught = 1\n\
             \x20   assert caught == 1\n",
        );

        let module =
            resolve(&entry, vec![p.pkg_root()]).expect("qualified exception types must resolve");
        assert_harness_ok(&module, 2);
    }

    /// Several `@test`s across dependency + sibling + local in one file all
    /// resolve together, and discovery counts every one (reporting is unchanged):
    /// a passing test and a deliberately-failing test (whose PASS/FAIL outcome is
    /// decided at run time in CI) are BOTH discovered and lower cleanly.
    #[test]
    fn discovery_counts_passing_and_failing_tests_with_mixed_imports() {
        let p = Project::new("mixed");
        p.write(
            "zenv/pkg/sample/src/sample.zap",
            "def answer() -> int64:\n    return 42\n",
        );
        p.write("tests/helpers.zap", "def ten() -> int64:\n    return 10\n");
        let entry = p.write(
            "tests/mixed_test.zap",
            "import sample\nimport helpers\n\
             @test\ndef passes():\n    assert sample.answer() == helpers.ten() + 32\n\
             @test\ndef fails_at_runtime():\n    assert sample.answer() == 0\n\
             @test\n@skip(\"later\")\ndef skipped():\n    assert False\n",
        );

        let module = resolve(&entry, vec![p.pkg_root()]).expect("mixed imports must resolve");
        // All three @tests are discovered — the failing one is not dropped, so the
        // run will still report it (as FAIL at run time).
        assert_harness_ok(&module, 3);
    }

    /// A module-level `const` defined in an imported DEPENDENCY is readable from
    /// a `@test` (the #1232 import fix + the #1236 harness-const fix together):
    /// the linker merges the dependency's global, the harness carries it over,
    /// and it lowers. Covers a scalar and a value-struct const with a reference
    /// field (the timezone/Easter shape).
    #[test]
    fn module_const_from_dependency_is_readable_in_test() {
        let p = Project::new("depconst");
        p.write(
            "zenv/pkg/cfg/src/cfg.zap",
            "struct Zone:\n    str name\n\
             const int64 LIMIT = 100\n\
             const Zone UTC = Zone(\"UTC\")\n",
        );
        let entry = p.write(
            "tests/cfg_test.zap",
            "import cfg\n\
             @test\ndef reads_scalar_dep_const():\n    assert cfg.LIMIT == 100\n\
             @test\ndef reads_struct_dep_const():\n    assert cfg.UTC.name == \"UTC\"\n",
        );

        let module = resolve(&entry, vec![p.pkg_root()]).expect("dependency consts must resolve");
        // The dependency's merged (mangled) globals must be present as bindings.
        let dep_bindings = module
            .items
            .iter()
            .filter(|it| matches!(it, Item::Stmt(zapc_syntax::ast::Stmt::Let { .. })))
            .count();
        assert!(
            dep_bindings >= 2,
            "both dependency consts must be merged in as module bindings, found {dep_bindings}"
        );
        assert_harness_ok(&module, 2);
    }

    // ---- negative cases --------------------------------------------------

    /// A genuinely unknown MODULE import fails cleanly (a module-not-found error,
    /// ZAP3010 — exactly what `zap run`/`zap build` report for the same import),
    /// surfaced as a non-empty, rendered file-level diagnostic rather than a crash
    /// or a silent pass.
    #[test]
    fn unknown_module_import_fails_cleanly() {
        let p = Project::new("unknownmod");
        let entry = p.write(
            "tests/bad_test.zap",
            "import does_not_exist\n\
             @test\ndef t():\n    assert does_not_exist.thing() == 1\n",
        );

        let errs =
            resolve(&entry, vec![p.pkg_root()]).expect_err("an unresolved module import must fail");
        assert!(
            errs.iter().any(|e| e.contains("ZAP3010")),
            "an unknown module must report ZAP3010 (module not found), got: {errs:?}"
        );
        assert!(
            errs.iter().any(|e| e.contains("does_not_exist")),
            "the diagnostic must name the missing module, got: {errs:?}"
        );
    }

    /// A genuinely unknown NAME in a test body still reports ZAP3001 cleanly —
    /// the fix resolves real imports, it does not paper over real errors.
    #[test]
    fn unknown_name_in_test_body_still_reports_zap3001() {
        let p = Project::new("unknownname");
        let entry = p.write(
            "tests/bad_test.zap",
            "@test\ndef t():\n    assert frobnicate == 1\n",
        );

        let errs =
            resolve(&entry, Vec::new()).expect_err("an unknown name must fail type-checking");
        assert!(
            errs.iter().any(|e| e.contains("ZAP3001")),
            "an unknown name must report ZAP3001, got: {errs:?}"
        );
    }

    /// The front-end error renderer used by the file-level test result produces
    /// the one-error-per-line `error[CODE]: message` form the report already
    /// expects — so a resolution failure reads as a failing file, unchanged.
    #[test]
    fn front_end_errors_render_as_file_level_result() {
        let p = Project::new("render");
        let entry = p.write(
            "tests/bad_test.zap",
            "import missing_dep\n@test\ndef t():\n    assert True\n",
        );
        // Drive the exact helper `run_file` uses (env-based), then confirm its
        // rendered message shape. `front_end_test_module` resolves against the
        // ambient environment; with no matching package on any search root the
        // import is unresolved, so it returns the rendered error string.
        let rendered = front_end_test_module(&entry).expect_err("must not resolve");
        assert!(
            rendered.lines().all(|l| l.starts_with("error[")),
            "each rendered line must be an `error[CODE]: ...` entry, got: {rendered:?}"
        );
        assert!(
            rendered.contains("missing_dep"),
            "the rendered error must name the missing module, got: {rendered:?}"
        );

        // And the shared renderer itself maps reports to that shape. (`Report`
        // is not `Debug`, so match rather than `expect_err`.)
        let reports = match crate::front_end_reports_with(
            &entry.display().to_string(),
            &Target::host(),
            vec![p.pkg_root()],
        ) {
            Ok(_) => panic!("must not resolve"),
            Err(reports) => reports,
        };
        let rendered = render_front_end_errors(&reports);
        assert!(rendered.starts_with("error["), "got: {rendered:?}");
    }

    // ---- #1519: imported-package dotted-decorator receivers --------------
    //
    // The #1509/#1515 object-binding decorator (mechanism A, return-type-driven)
    // only resolved a LOCAL struct receiver. #1519 extends it across a package
    // boundary: a dotted decorator whose root is an IMPORTED module resolves the
    // linked package's exported free function, reads its declared return type,
    // and — when non-void — rebinds the decorated name to the returned object
    // (the cli#4 consumer surface). These build a real `zenv/pkg/<name>/src`
    // path dependency and drive the full front-end (`resolve` = modlink +
    // decorator desugar + sema), the same flow `ZAP_ZENV=… zap ir repro.zap`
    // runs.

    /// Find `main`'s body as a debug string, for asserting on the spliced
    /// rebind/registration statements (the merged AST carries mangled symbols,
    /// so matching on the debug render mirrors the sibling modlink suite).
    fn main_body_debug(m: &Module) -> String {
        m.items
            .iter()
            .find_map(|it| match it {
                Item::Function(f) if f.name == "main" => Some(format!("{:?}", f.body.stmts)),
                _ => None,
            })
            .expect("a main function")
    }

    /// THE #1519 repro (from the issue): `@factory.command`, where
    /// `factory.command(fn(str) -> str) -> Command` is an imported free function,
    /// rebinds `greet` to the returned `Command`, so `greet.run()` type-checks
    /// and the `assert greet.run() == "Hello Ada"` body resolves. Before the fix
    /// this reported `ZAP3001: unknown name 'factory'` at the decorator.
    #[test]
    fn imported_decorator_binds_decorated_name_to_returned_object() {
        let p = Project::new("impdeco");
        p.write(
            "zenv/pkg/factory/src/factory.zap",
            "struct Command:\n\
             \x20   fn(str) -> str handler\n\n\
             \x20   def run(self) -> str:\n\
             \x20       auto handler = self.handler\n\
             \x20       return handler(\"Ada\")\n\n\
             def command(fn(str) -> str handler) -> Command:\n\
             \x20   return Command(handler=handler)\n",
        );
        let entry = p.write(
            "src/repro.zap",
            "import factory\n\n\
             @factory.command\n\
             def greet(str name) -> str:\n\
             \x20   return \"Hello \" + name\n\n\
             def main():\n\
             \x20   assert greet.run() == \"Hello Ada\"\n",
        );

        let module = resolve(&entry, vec![p.pkg_root()])
            .expect("an imported dotted decorator must resolve and type-check (#1519)");
        // The original function is kept, renamed `greet$impl` — the typed callable
        // handed to the factory.
        assert!(
            has_fn_containing(&module, "greet$impl"),
            "the decorated function must be renamed greet$impl"
        );
        // The raw `greet` free function no longer exists under its own name (it
        // is now a local rebound in `main`).
        assert!(
            !module
                .items
                .iter()
                .any(|it| matches!(it, Item::Function(f) if f.name == "greet")),
            "the bare `greet` function must be consumed into the rebind"
        );
        // `greet` is rebound in main to the mangled imported free-function call.
        let main = main_body_debug(&module);
        assert!(
            main.contains("__zm") && main.contains("command") && main.contains("greet$impl"),
            "main must rebind greet to the mangled factory.command(greet$impl), got: {main}"
        );
    }

    /// The import ALIAS form: `import factory as f` + `@f.command` binds the same
    /// way — the alias is the receiver root the decorator resolves through.
    #[test]
    fn imported_decorator_alias_form_binds_object() {
        let p = Project::new("impdeco_alias");
        p.write(
            "zenv/pkg/factory/src/factory.zap",
            "struct Command:\n\
             \x20   fn(str) -> str handler\n\n\
             \x20   def run(self) -> str:\n\
             \x20       auto handler = self.handler\n\
             \x20       return handler(\"Ada\")\n\n\
             def command(fn(str) -> str handler) -> Command:\n\
             \x20   return Command(handler=handler)\n",
        );
        let entry = p.write(
            "src/repro.zap",
            "import factory as f\n\n\
             @f.command\n\
             def greet(str name) -> str:\n\
             \x20   return \"Hello \" + name\n\n\
             def main():\n\
             \x20   assert greet.run() == \"Hello Ada\"\n",
        );

        let module = resolve(&entry, vec![p.pkg_root()])
            .expect("the import-alias dotted decorator must resolve (#1519)");
        assert!(
            has_fn_containing(&module, "greet$impl"),
            "alias form must still rename to greet$impl"
        );
        let main = main_body_debug(&module);
        assert!(
            main.contains("__zm") && main.contains("command") && main.contains("greet$impl"),
            "alias form must rebind greet to the mangled command call, got: {main}"
        );
    }

    /// A STACKED imported-module chain `@cli.command` / `@cli.option(...)` /
    /// `@cli.argument(...)` composes inner-to-outer into one nested call, with the
    /// decorators' typed metadata arguments (`"--count"`, `"World"`) retained as
    /// the library functions' leading arguments. The name binds to the OUTERMOST
    /// result; `greet.run()` dispatches through the whole chain.
    #[test]
    fn imported_decorator_stacked_chain_composes_with_metadata() {
        let p = Project::new("impdeco_stacked");
        p.write(
            "zenv/pkg/cli/src/cli.zap",
            "struct Command:\n\
             \x20   str opt\n\
             \x20   str arg\n\
             \x20   fn(str) -> str handler\n\n\
             \x20   def run(self) -> str:\n\
             \x20       auto handler = self.handler\n\
             \x20       return handler(self.arg)\n\n\
             def argument(str name, fn(str) -> str handler) -> Command:\n\
             \x20   return Command(opt=\"\", arg=name, handler=handler)\n\n\
             def option(str flag, Command c) -> Command:\n\
             \x20   return Command(opt=flag, arg=c.arg, handler=c.handler)\n\n\
             def command(Command c) -> Command:\n\
             \x20   return Command(opt=c.opt, arg=c.arg, handler=c.handler)\n",
        );
        let entry = p.write(
            "src/repro.zap",
            "import cli\n\n\
             @cli.command\n\
             @cli.option(\"--count\")\n\
             @cli.argument(\"World\")\n\
             def greet(str name) -> str:\n\
             \x20   return \"Hello \" + name\n\n\
             def main():\n\
             \x20   assert greet.run() == \"Hello World\"\n",
        );

        let module = resolve(&entry, vec![p.pkg_root()])
            .expect("a stacked imported-module decorator chain must resolve (#1519)");
        assert!(has_fn_containing(&module, "greet$impl"), "stacked: rename");
        let main = main_body_debug(&module);
        // One nested call binding greet: command(option("--count", argument("World", greet$impl))).
        assert!(
            main.contains("command") && main.contains("option") && main.contains("argument"),
            "the stacked chain must compose all three imported functions, got: {main}"
        );
        assert!(
            main.contains("--count") && main.contains("World"),
            "stacked metadata arguments must be retained, got: {main}"
        );
        // Composition order: the argument call (innermost) sits inside the option
        // call, which sits inside the command call — the metadata string for the
        // inner decorator appears later in the debug render than the outer method.
        let i_cmd = main.find("command").unwrap();
        let i_opt = main.find("option").unwrap();
        let i_arg = main.find("argument").unwrap();
        assert!(
            i_cmd < i_opt && i_opt < i_arg,
            "chain must nest command(option(argument(...))) outer-to-inner, got: {main}"
        );
    }

    /// THE #1527 repro (from the issue): a library-defined returning decorator
    /// `@factory.option("--count", int64)` accepts the concrete type `int64` as a
    /// first-class **type value** argument. Before #1527 this was
    /// `ZAP3001: unknown name 'int64'` at the decorator argument. It must now
    /// resolve (sema) AND lower end-to-end: the type value flows through the
    /// imported returning-decorator desugar to the generic `option[T]`, binding
    /// `T = Type[int64]`, and `greet` is rebound to the returned `Command`.
    #[test]
    fn imported_decorator_accepts_type_argument() {
        let p = Project::new("typearg_repro");
        p.write(
            "zenv/pkg/factory/src/factory.zap",
            "struct Command:\n\
             \x20   fn(str) -> str handler\n\n\
             \x20   def run(self) -> str:\n\
             \x20       auto handler = self.handler\n\
             \x20       return handler(\"Ada\")\n\n\
             def option[T](str name, T kind, fn(str) -> str handler) -> Command:\n\
             \x20   return Command(handler=handler)\n",
        );
        let entry = p.write(
            "src/main.zap",
            "import factory\n\n\
             @factory.option(\"--count\", int64)\n\
             def greet(str name) -> str:\n\
             \x20   return \"Hello \" + name\n\n\
             def main():\n\
             \x20   assert greet.run() == \"Hello Ada\"\n",
        );

        let module = resolve(&entry, vec![p.pkg_root()]).expect(
            "a concrete type argument to an imported returning decorator must resolve (#1527)",
        );
        assert!(
            has_fn_containing(&module, "greet$impl"),
            "the decorated name must be rebound (greet$impl), the #1509/#1519 path"
        );
        // End-to-end: the whole program lowers to ZIR, and the type value binds
        // the generic parameter `T = Type[int64]`, monomorphizing the callee (the
        // mangled name appears only after lowering runs monomorphization). The
        // `int64` token lowers to its `int64` type-id; no residual `unknown name`.
        let zir = resolve_and_lower_zir(&entry, vec![p.pkg_root()]);
        assert!(
            zir.contains("option$Type$int64"),
            "the type value must bind `T = Type[int64]`, monomorphizing \
             option$Type$int64, got:\n{zir}"
        );
    }

    /// The #1527 golden surface: a primitive type reaches a STACKED imported
    /// decorator carrying the named metadata `default=`, and a second type reaches
    /// the innermost decorator. Two type values (`int64`, `str`) flow through the
    /// nested-call desugar alongside `default=7`; the chain composes and lowers.
    #[test]
    fn imported_type_decorator_stacked_with_default_metadata() {
        let p = Project::new("typearg_stacked");
        p.write(
            "zenv/pkg/cli/src/cli.zap",
            "struct Command:\n\
             \x20   str flag\n\
             \x20   int64 deflt\n\
             \x20   fn(str) -> str handler\n\n\
             \x20   def run(self) -> str:\n\
             \x20       auto handler = self.handler\n\
             \x20       return handler(self.flag)\n\n\
             def argument[T](str name, T kind, fn(str) -> str handler) -> Command:\n\
             \x20   return Command(flag=name, deflt=0, handler=handler)\n\n\
             def option[T](str flag, T kind, Command c, int64 default) -> Command:\n\
             \x20   return Command(flag=flag, deflt=default, handler=c.handler)\n\n\
             def command(Command c) -> Command:\n\
             \x20   return Command(flag=c.flag, deflt=c.deflt, handler=c.handler)\n",
        );
        let entry = p.write(
            "src/main.zap",
            "import cli\n\n\
             @cli.command\n\
             @cli.option(\"--count\", int64, default=7)\n\
             @cli.argument(\"name\", str)\n\
             def greet(str flag) -> str:\n\
             \x20   return \"greet \" + flag\n\n\
             def main():\n\
             \x20   assert greet.run() == \"greet --count\"\n\
             \x20   assert greet.deflt == 7\n",
        );

        let module = resolve(&entry, vec![p.pkg_root()]).expect(
            "stacked type-carrying imported decorators with default= metadata must resolve (#1527)",
        );
        let main = main_body_debug(&module);
        assert!(
            main.contains("command") && main.contains("option") && main.contains("argument"),
            "the stacked chain must compose all three imported functions, got: {main}"
        );
        assert!(
            main.contains("--count") && main.contains("name"),
            "stacked metadata arguments must be retained, got: {main}"
        );
        // Lower end-to-end: both type values are monomorphized — int64 at option,
        // str at argument — proving each reached its decorator through the stacked
        // nested-call desugar alongside the `default=` metadata.
        let zir = resolve_and_lower_zir(&entry, vec![p.pkg_root()]);
        assert!(
            zir.contains("option$Type$int64"),
            "int64 must reach option as Type[int64], got:\n{zir}"
        );
        assert!(
            zir.contains("argument$Type$str"),
            "str must reach argument as Type[str], got:\n{zir}"
        );
    }

    /// A VOID imported receiver still REGISTERS (the fastzap `@app.get` surface
    /// across a package boundary): the decorated name stays the raw function and
    /// the synthesized `factory.register(greet)` call is spliced as a bare
    /// statement — it is NOT rebound. Preserves the #1509 void case for imports.
    #[test]
    fn imported_void_decorator_still_registers() {
        let p = Project::new("impdeco_void");
        p.write(
            "zenv/pkg/factory/src/factory.zap",
            "def register(fn(str) -> str handler):\n\
             \x20   auto h = handler\n\
             \x20   print(h(\"x\"))\n",
        );
        let entry = p.write(
            "src/repro.zap",
            "import factory\n\n\
             @factory.register\n\
             def greet(str name) -> str:\n\
             \x20   return \"Hello \" + name\n\n\
             def main():\n\
             \x20   assert greet(\"World\") == \"Hello World\"\n",
        );

        let module = resolve(&entry, vec![p.pkg_root()])
            .expect("a void imported receiver must still register and resolve (#1519)");
        // Not rebound: no `$impl` rename, and `greet` survives as a free function.
        assert!(
            !has_fn_containing(&module, "greet$impl"),
            "a void imported receiver must NOT rebind (no greet$impl)"
        );
        assert!(
            module
                .items
                .iter()
                .any(|it| matches!(it, Item::Function(f) if f.name == "greet")),
            "the raw `greet` function must survive the registering path"
        );
        // The registration call to the mangled imported function is spliced in.
        let main = main_body_debug(&module);
        assert!(
            main.contains("__zm") && main.contains("register") && main.contains("greet"),
            "the mangled factory.register(greet) call must be spliced, got: {main}"
        );
    }

    /// Regression: a LOCAL struct-receiver object-binding decorator (#1515) still
    /// binds correctly when driven through the full modlink front-end (not just
    /// the standalone desugar unit tests) — the imported path must not disturb it.
    #[test]
    fn local_struct_receiver_decorator_still_binds_through_front_end() {
        let p = Project::new("localdeco");
        let entry = p.write(
            "src/repro.zap",
            "struct Cli:\n\
             \x20   def command(self, fn(str) -> str handler) -> Command:\n\
             \x20       return Command(handler=handler)\n\n\
             struct Command:\n\
             \x20   fn(str) -> str handler\n\n\
             \x20   def run(self) -> str:\n\
             \x20       auto handler = self.handler\n\
             \x20       return handler(\"Ada\")\n\n\
             @cli.command\n\
             def greet(str name) -> str:\n\
             \x20   return \"Hello \" + name\n\n\
             def main():\n\
             \x20   auto cli = Cli()\n\
             \x20   assert greet.run() == \"Hello Ada\"\n",
        );

        let module =
            resolve(&entry, Vec::new()).expect("a local struct-receiver decorator must still bind");
        assert!(
            has_fn_containing(&module, "greet$impl"),
            "the local receiver path must still rename to greet$impl"
        );
        let main = main_body_debug(&module);
        assert!(
            main.contains("command") && main.contains("greet$impl"),
            "local receiver must still rebind greet to cli.command(greet$impl), got: {main}"
        );
    }
}
