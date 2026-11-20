//! Golden-test harness.
//!
//! Golden programs live in `<repo>/tests/golden/*.zap`, each paired with a
//! `*.expected` file holding the program's expected stdout. Two optional
//! sibling files cover programs that are meant to fail (e.g. an uncaught
//! exception, ticket #22):
//!
//! - `*.exit` — the expected process exit code (default `0`, i.e. success).
//! - `*.stderr` — a substring that must appear in the program's stderr.
//!
//! The end-to-end run (`golden_programs_match`) requires the `llvm` feature
//! (and an LLVM toolchain); run it in the dev container with
//! `--features zapc/llvm`. The `fixtures_are_paired` check runs everywhere.
//!
//! The end-to-end runs compile+link+run independent fixtures concurrently on a
//! bounded worker pool (see `run_goldens_parallel`). The pool width defaults to
//! 2 and is tunable without recompiling via the `ZAP_GOLDEN_JOBS` env var; set
//! it to `1` to restore fully serial execution. Per-fixture outcomes are
//! collected and reported in sorted fixture order, so a failure prints the same
//! regardless of scheduling.

use std::path::PathBuf;

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

/// `<repo>/tests/golden_projects` — multi-file project fixtures (I#283/D-0081).
/// Each subdirectory is a project with an entry `main.zap`, its sibling module
/// files, and an `expected` file (plus optional `exit`/`stderr` sidecars).
fn golden_projects_dir() -> PathBuf {
    golden_dir()
        .parent()
        .expect("golden dir has a parent")
        .join("golden_projects")
}

/// The entry `main.zap` of each project fixture.
fn project_fixtures() -> Vec<PathBuf> {
    let dir = golden_projects_dir();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for entry in rd {
            let p = entry.expect("dir entry").path();
            let main = p.join("main.zap");
            if p.is_dir() && main.exists() {
                out.push(main);
            }
        }
    }
    out.sort();
    out
}

/// Every project fixture has an `expected` file. Runs everywhere.
#[test]
fn project_fixtures_are_paired() {
    for main in project_fixtures() {
        let expected = main.parent().expect("project dir").join("expected");
        assert!(
            expected.exists(),
            "missing `expected` for project {}",
            main.display()
        );
    }
}

/// Every `.zap` fixture has a matching `.expected` file. Runs everywhere.
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

/// Run `cmd`, capturing stdout/stderr, but FAIL FAST if it does not finish
/// within `timeout` — a hung golden (e.g. a wait on a signal/channel that never
/// fires) then fails this test deterministically instead of stalling the whole
/// CI run until the job's global timeout. stdout/stderr are drained on threads
/// so the child never blocks on a full pipe buffer.
#[cfg(feature = "codegen")]
fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
    label: &str,
) -> std::process::Output {
    use std::io::Read;
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn zapc");
    let mut out = child.stdout.take().expect("stdout pipe");
    let mut err = child.stderr.take().expect("stderr pipe");
    let out_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let err_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(s) => break s,
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "golden {label} did not finish within {timeout:?} — timed out and \
                         was killed. A `[run]` timeout means the compiled program hung \
                         (deadlock/livelock); a `[build]` timeout means the compiler/linker \
                         stalled — usually CPU starvation on an overloaded runner, not a \
                         program bug (see zaplang-io/zap#1113)."
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    };
    let stdout = out_t.join().unwrap_or_default();
    let stderr = err_t.join().unwrap_or_default();
    std::process::Output {
        status,
        stdout,
        stderr,
    }
}

/// A unique, per-invocation output path for a fixture's compiled binary, so the
/// parallel pool never collides on `-o`. Derived from the fixture stem, this
/// process id, and a monotonic counter.
#[cfg(feature = "codegen")]
fn unique_out_path(source: &std::path::Path) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CTR: AtomicUsize = AtomicUsize::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("fixture");
    std::env::temp_dir().join(format!("zap-golden-{stem}-{}-{n}", std::process::id()))
}

/// Compile a fixture with `zapc build -o <tmp>` under a GENEROUS ceiling, then
/// run the produced binary under the TIGHT `run_timeout`. This is the whole
/// point of the split (zaplang-io/zap#1113): the tight per-fixture deadline must
/// cover only the compiled program's execution, never `cc`/`ld` — otherwise a
/// linker starved of CPU on an overloaded runner trips the deadline and is
/// misreported as a "hang in the compiled program". The build still has its own
/// (large) ceiling so a genuine compiler/linker *deadlock* still fails, just not
/// on mere slowness. `envs` are applied to BOTH phases: some are build-time
/// (e.g. `ZAP_ZENV` for project dependency resolution), others run-time (e.g.
/// scheduler knobs like `MAXPROCS`).
///
/// Returns `(status, stdout, stderr)` matching what `zapc run` used to produce:
/// on a build failure, the build's own output (so a compile-diagnostic fixture
/// still works); on success, the run's exit status and stdout, with build and
/// run stderr concatenated (so a compile *warning* a fixture asserts on is still
/// present).
#[cfg(feature = "codegen")]
fn compile_then_run(
    bin: &str,
    source: &std::path::Path,
    envs: &[(String, String)],
    run_timeout: std::time::Duration,
) -> (std::process::ExitStatus, Vec<u8>, Vec<u8>) {
    use std::process::Command;
    // Large enough that normal compile+link, even on a saturated runner, never
    // reaches it, but a true compiler/linker deadlock still fails deterministically.
    let build_ceiling = std::time::Duration::from_secs(300);
    let out = unique_out_path(source);

    let mut build = Command::new(bin);
    build.arg("build").arg(source).arg("-o").arg(&out);
    for (k, v) in envs {
        build.env(k, v);
    }
    let build_out = run_with_timeout(
        build,
        build_ceiling,
        &format!("{} [build]", source.display()),
    );
    if !build_out.status.success() {
        // Compile/link failed (or a diagnostic fixture that expects a nonzero
        // exit): surface the build output verbatim, exactly as `zapc run` would
        // have on a compile failure.
        return (build_out.status, build_out.stdout, build_out.stderr);
    }

    let mut run = Command::new(&out);
    for (k, v) in envs {
        run.env(k, v);
    }
    let run_out = run_with_timeout(run, run_timeout, &format!("{} [run]", source.display()));
    let _ = std::fs::remove_file(&out);

    let mut stderr = build_out.stderr;
    stderr.extend_from_slice(&run_out.stderr);
    (run_out.status, run_out.stdout, stderr)
}

/// Parse an optional `KEY=VALUE` env sidecar into pairs, skipping blanks and
/// `#` comments. `$PROJDIR` in a value is replaced with `projdir` when given
/// (used by project fixtures to point `ZAP_ZENV` at a prebuilt `zenv/`).
#[cfg(feature = "codegen")]
fn parse_env_sidecar(path: &std::path::Path, projdir: Option<&str>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(contents) = std::fs::read_to_string(path) {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let v = match projdir {
                    Some(p) => v.trim().replace("$PROJDIR", p),
                    None => v.trim().to_string(),
                };
                out.push((k.trim().to_string(), v));
            }
        }
    }
    out
}

/// Optional `*.skip_arch` sidecar: whitespace/newline-separated architecture
/// names (matched against `std::env::consts::ARCH`; `#` starts a comment). A
/// fixture whose sidecar names the architecture this test binary runs on is
/// SKIPPED by the end-to-end runner below, with a loud notice. Used to
/// quarantine a golden that hits a known, tracked, architecture-specific bug so
/// CI stays deterministically green while the ticket stays open. No fixture is
/// quarantined at present: the three concurrent stress goldens that were skipped
/// on aarch64 for the parked #223 GC deadlock (immix_evac_coro_stress,
/// concurrent_queue_mpsc_coro_stress, concurrent_queue_mpsc_thread_stress) run on
/// every arch again now that #223 is fixed (PR #694, D-0117). Add a sidecar only
/// to quarantine a fresh arch-specific failure against an open ticket, and delete
/// it as soon as that ticket is fixed.
#[cfg(feature = "codegen")]
fn skipped_on_this_arch(zap: &std::path::Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(zap.with_extension("skip_arch")) else {
        return false;
    };
    let arch = std::env::consts::ARCH;
    contents.lines().any(|line| {
        line.split('#')
            .next()
            .unwrap_or("")
            .split_whitespace()
            .any(|a| a == arch)
    })
}

/// Outcome of running a single golden fixture. `Skipped` covers an
/// architecture-quarantined fixture (see `skipped_on_this_arch`); `Failed`
/// carries the human-readable mismatch/exit/stderr message to surface.
#[cfg(feature = "codegen")]
enum Outcome {
    Pass,
    Skipped,
    Failed(String),
}

/// Number of fixtures to run concurrently. Defaults to 2 (the conservative
/// starting width for the concurrency/GC-sensitive suite); `ZAP_GOLDEN_JOBS=1`
/// restores serial execution, and a higher value widens the pool without a
/// recompile. A missing/garbage/zero value falls back to the default.
#[cfg(feature = "codegen")]
fn golden_jobs() -> usize {
    std::env::var("ZAP_GOLDEN_JOBS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(2)
}

/// Run `work` over `items` on a bounded pool of `golden_jobs()` worker threads,
/// then assert the aggregated outcome. Workers pull fixtures off a shared index
/// so the pool stays bounded (no thread-per-fixture fan-out); each fixture's
/// work is independent (spawn `zapc`, link, run, diff), so this parallelizes
/// cleanly. Outcomes are collected and, after the pool drains, reported in the
/// original (sorted) `items` order, so a failure prints identically regardless
/// of scheduling and concurrent fixtures never interleave into unreadable
/// output. A hang still panics loudly from inside `run_with_timeout`, failing
/// the test fast rather than being silenced.
#[cfg(feature = "codegen")]
fn run_goldens_parallel<F>(items: Vec<PathBuf>, work: F)
where
    F: Fn(&std::path::Path) -> Outcome + Sync,
{
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    // Optional `ZAP_GOLDEN_ONLY` allowlist: a comma-separated list of
    // substrings matched against each fixture's path. When set (non-empty),
    // only the matching fixtures run; when unset/blank the whole corpus runs,
    // as always. CI's "Test under LTO" step sets this to a small,
    // representative slice so the LTO build is exercised end-to-end without
    // re-linking the entire program corpus under `-O2 -flto` on every push
    // (the full corpus still runs under the default, fast build in the step
    // above). Patterns that match nothing simply contribute nothing — this
    // runner is shared by the golden/project/race corpora, so an allowlist of
    // golden stems is expected to filter the project and race corpora to zero.
    let items: Vec<PathBuf> = match std::env::var("ZAP_GOLDEN_ONLY") {
        Ok(list) if !list.trim().is_empty() => {
            let pats: Vec<String> = list
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            items
                .into_iter()
                .filter(|p| {
                    let s = p.to_string_lossy();
                    pats.iter().any(|pat| s.contains(pat.as_str()))
                })
                .collect()
        }
        _ => items,
    };

    if items.is_empty() {
        return;
    }
    let jobs = golden_jobs().min(items.len());
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Outcome)>> = Mutex::new(Vec::with_capacity(items.len()));

    std::thread::scope(|scope| {
        for _ in 0..jobs {
            let (next, results, items, work) = (&next, &results, &items, &work);
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= items.len() {
                    break;
                }
                let outcome = work(&items[i]);
                results.lock().expect("results lock").push((i, outcome));
            });
        }
    });

    let mut results = results.into_inner().expect("results lock");
    results.sort_by_key(|(i, _)| *i);

    let mut failures = Vec::new();
    for (i, outcome) in results {
        match outcome {
            Outcome::Pass | Outcome::Skipped => {}
            Outcome::Failed(msg) => failures.push(format!("{}:\n{msg}", items[i].display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} golden fixture(s) failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Compile + run each fixture and compare stdout. Requires the `llvm` feature.
#[cfg(feature = "codegen")]
#[test]
fn golden_programs_match() {
    // Tight per-fixture ceiling for the RUN phase only: the compiled program runs
    // in a second or two, so 60s trips only on a true program hang. Compilation
    // and linking are timed separately and generously (see `compile_then_run`),
    // so a `cc`/`ld` slowdown on a loaded runner no longer masquerades as a hang
    // (zaplang-io/zap#1113).
    let per_test = std::time::Duration::from_secs(60);
    let bin = env!("CARGO_BIN_EXE_zapc");
    run_goldens_parallel(zap_fixtures(), |zap| {
        // A fixture quarantined on this architecture (see `skipped_on_this_arch`)
        // is skipped loudly rather than run — e.g. the concurrent stress goldens
        // on aarch64, pending the parked #223 GC race.
        if skipped_on_this_arch(zap) {
            eprintln!(
                "golden: SKIP {} on arch {} — quarantined via .skip_arch sidecar (see #223)",
                zap.display(),
                std::env::consts::ARCH
            );
            return Outcome::Skipped;
        }
        let expected = std::fs::read_to_string(zap.with_extension("expected"))
            .expect("expected output present");
        // Optional expected exit code (default 0) and expected-stderr substring.
        let expected_exit: i32 = std::fs::read_to_string(zap.with_extension("exit"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let expected_stderr = std::fs::read_to_string(zap.with_extension("stderr")).ok();

        // Compile with a generous ceiling, then time ONLY the program's run
        // (zaplang-io/zap#1113): `per_test` must not cover `cc`/`ld`, or a linker
        // starved of CPU on an overloaded runner trips it and is misreported as a
        // program hang. The `*.env` sidecar — e.g. `MAXPROCS=1` to pin the
        // preemption golden to one worker M (ticket #172) — applies to both phases.
        let envs = parse_env_sidecar(&zap.with_extension("env"), None);
        let (status, stdout, stderr) = compile_then_run(bin, zap, &envs, per_test);

        let got = String::from_utf8_lossy(&stdout);
        let got_err = String::from_utf8_lossy(&stderr);
        if status.code() != Some(expected_exit) {
            return Outcome::Failed(format!(
                "exit code mismatch: got {:?}, want {expected_exit} (stderr:\n{got_err})",
                status.code()
            ));
        }
        if got.trim_end() != expected.trim_end() {
            return Outcome::Failed(format!(
                "stdout mismatch:\n--- got ---\n{}\n--- want ---\n{}\n(stderr:\n{got_err})",
                got.trim_end(),
                expected.trim_end()
            ));
        }
        if let Some(want_err) = expected_stderr {
            let want_err = want_err.trim_end();
            if !got_err.contains(want_err) {
                return Outcome::Failed(format!(
                    "stderr should contain {want_err:?}, got:\n{got_err}"
                ));
            }
        }
        Outcome::Pass
    });
}

/// Compile + run each multi-file project fixture (I#283/D-0081) by its entry
/// `main.zap` and compare stdout. Requires the `llvm` feature.
#[cfg(feature = "codegen")]
#[test]
fn golden_projects_match() {
    // Tight RUN-phase-only ceiling; compile/link is timed separately and
    // generously in `compile_then_run` (zaplang-io/zap#1113).
    let per_test = std::time::Duration::from_secs(60);
    let bin = env!("CARGO_BIN_EXE_zapc");
    run_goldens_parallel(project_fixtures(), |main| {
        let proj = main.parent().expect("project dir");
        let expected =
            std::fs::read_to_string(proj.join("expected")).expect("expected output present");
        let expected_exit: i32 = std::fs::read_to_string(proj.join("exit"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let expected_stderr = std::fs::read_to_string(proj.join("stderr")).ok();

        // Optional `env` sidecar: `KEY=VALUE` lines, with `$PROJDIR` expanded to
        // the project's absolute path. Used by the cross-package fixture to point
        // `ZAP_ZENV` at a prebuilt `zenv/` of materialized dependency packages
        // (I#283/D-0081 Stage B) — a BUILD-time concern, so it must reach the
        // compile phase too; `compile_then_run` applies it to both.
        let projdir = proj.display().to_string();
        let envs = parse_env_sidecar(&proj.join("env"), Some(&projdir));
        let (status, stdout, stderr) = compile_then_run(bin, main, &envs, per_test);

        let got = String::from_utf8_lossy(&stdout);
        let got_err = String::from_utf8_lossy(&stderr);
        if status.code() != Some(expected_exit) {
            return Outcome::Failed(format!(
                "exit code mismatch: got {:?}, want {expected_exit} (stderr:\n{got_err})",
                status.code()
            ));
        }
        if got.trim_end() != expected.trim_end() {
            return Outcome::Failed(format!(
                "stdout mismatch:\n--- got ---\n{}\n--- want ---\n{}\n(stderr:\n{got_err})",
                got.trim_end(),
                expected.trim_end()
            ));
        }
        if let Some(want_err) = expected_stderr {
            let want_err = want_err.trim_end();
            if !got_err.contains(want_err) {
                return Outcome::Failed(format!(
                    "stderr should contain {want_err:?}, got:\n{got_err}"
                ));
            }
        }
        Outcome::Pass
    });
}

// ---- `--race` detector goldens (#665, D-0114; slice 1 #677, slice 2 #678) --
//
// Slice 1 (#677) instruments every ZAP5125 shared-global access with a
// `__zap_race_*` hook; slice 2 (#678) fills those hooks in with a real
// happens-before / vector-clock engine (`zaprace.inc.c`), wiring spawn and
// `Mutex` release/acquire edges. So a `--race` run of the `tests/race/*.zap`
// fixtures now produces real verdicts: the `racy_*` programs are REPORTED with a
// source location and exit non-zero, and the `clean_*` programs run silently and
// exit 0 with their deterministic total. These goldens assert that behaviour
// end-to-end, under both collectors (CI matrix).

/// `<repo>/tests/race` — the runtime-data-race fixtures (#665, D-0114).
fn race_dir() -> PathBuf {
    golden_dir()
        .parent()
        .expect("golden dir has a parent")
        .join("race")
}

fn race_fixtures() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(race_dir()) {
        for entry in rd {
            let p = entry.expect("dir entry").path();
            if p.extension().is_some_and(|e| e == "zap") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// The four spec fixtures are present. Runs everywhere (no LLVM needed).
#[test]
fn race_fixtures_present() {
    let names: Vec<String> = race_fixtures()
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    for want in [
        "racy_counter.zap",
        "racy_read_write.zap",
        "racy_write_write.zap",
        "racy_chan.zap",
        "clean_mutex.zap",
        "clean_atomic.zap",
        "clean_single.zap",
        "clean_chan_rendezvous.zap",
        "clean_chan_buffered.zap",
        "clean_waitgroup.zap",
        "clean_once.zap",
        "racy_heap_list.zap",
        "racy_heap_struct.zap",
        "clean_heap_mutex.zap",
        "clean_heap_concurrent.zap",
        "clean_heap_evac.zap",
    ] {
        assert!(
            names.iter().any(|n| n == want),
            "missing race fixture {want}; found {names:?}"
        );
    }
}

/// Flag-plumbing regression guard (#678/#679): the `zapc` binary MUST honor
/// `--race` end-to-end. The #678 saga was a SILENT no-op — the thin `zapc` arg
/// parser dropped `--race`, so instrumentation was never emitted and racy
/// programs ran clean, yet every golden still "passed". This asserts, at the
/// binary's own arg parser and WITHOUT LLVM (so it runs in the host gate, not
/// only the codegen matrix), that `zapc ir --race` emits the `__zap_race_write`
/// hook on a known-racy fixture and that a plain `zapc ir` emits none. A future
/// regression that drops or mis-parses the flag fails loudly here rather than
/// silently disabling the detector.
#[test]
fn race_flag_is_honored_by_zapc_binary() {
    use std::process::Command;
    // `zapc ir` is the front-end + lowering only (no LLVM, no linking, cannot
    // hang), so a plain `.output()` is enough — no timeout harness, which keeps
    // this guard host-runnable without the `codegen` feature.
    let bin = env!("CARGO_BIN_EXE_zapc");
    let fixture = race_dir().join("racy_counter.zap");

    let with_out = Command::new(bin)
        .arg("ir")
        .arg("--race")
        .arg(&fixture)
        .output()
        .expect("spawn zapc ir --race");
    assert_eq!(
        with_out.status.code(),
        Some(0),
        "`zapc ir --race` should succeed; stderr:\n{}",
        String::from_utf8_lossy(&with_out.stderr)
    );
    let with_ir = String::from_utf8_lossy(&with_out.stdout);
    assert!(
        with_ir.contains("__zap_race_write"),
        "`zapc ir --race` must emit a `__zap_race_write` hook on a racy fixture \
         (flag dropped/mis-parsed? see #678); got IR:\n{with_ir}"
    );

    let without_out = Command::new(bin)
        .arg("ir")
        .arg(&fixture)
        .output()
        .expect("spawn zapc ir");
    let without_ir = String::from_utf8_lossy(&without_out.stdout);
    assert!(
        !without_ir.contains("__zap_race_"),
        "a plain `zapc ir` (no --race) must emit NO race hook (byte-identical-off \
         invariant); got IR:\n{without_ir}"
    );
}

/// The exact, deterministic total each `clean_*` fixture must print. These are
/// walled-garden invariants: the serialized / atomic / single-writer programs
/// produce the same count on every run and under either collector, with or
/// without `--race`.
#[cfg(feature = "codegen")]
fn clean_expected_total(name: &str) -> Option<&'static str> {
    match name {
        "clean_mutex.zap" => Some("100000"),
        "clean_atomic.zap" => Some("100000"),
        "clean_single.zap" => Some("50000"),
        // Slice 3 (#679) breadth fixtures — channel / WaitGroup / Once edges.
        "clean_chan_rendezvous.zap" => Some("43"),
        "clean_chan_buffered.zap" => Some("1098"),
        "clean_waitgroup.zap" => Some("77"),
        "clean_once.zap" => Some("246"),
        // Slice 4 (#684) shared-heap fixtures.
        "clean_heap_mutex.zap" => Some("100000"),
        "clean_heap_concurrent.zap" => Some("100000"),
        "clean_heap_evac.zap" => Some("40000"),
        _ => None,
    }
}

/// The substring a `racy_*` fixture's `--race` report must contain — the KIND of
/// location it races on (a module global names `counter`; a slice-4 shared heap
/// object is reported as a `shared heap location`, i.e. `heap`) — so each fixture
/// stays pinned to its own location while tolerating the exact source span.
#[cfg(feature = "codegen")]
fn racy_expected_token(name: &str) -> &'static str {
    if name.starts_with("racy_heap") {
        "heap"
    } else {
        "counter"
    }
}

/// Run one race fixture and judge the detector's end-to-end verdict (#678 engine,
/// #679 breadth). A plain `zap run` still exits 0 (the program is legal — the
/// light model admits unsynchronized sharing); `zap run --race` applies the
/// detector:
///
/// - `racy_*` — the detector REPORTS a data race on `counter` and exits
///   non-zero. The match is deliberately LOCATION-TOLERANT (it asserts the words
///   `data race` and the global name `counter`, not a line/column) and asserts
///   NO nondeterministic value (heap bytes, addresses, timings) — the verdict is
///   schedule-independent, so the report is a stable golden; the racy total is
///   never asserted (the run exits before it is printed, and it is a lost-update
///   nondeterministic value anyway).
/// - `clean_*` — the detector stays SILENT: exit 0 and the exact expected total
///   (`clean_expected_total`), identical to the non-`--race` baseline.
///
/// `maxprocs`, when `Some`, is exported as `ZAP_MAXPROCS` to the compiled program
/// (true OS-thread parallelism across the scheduler's Ms); `None` leaves the
/// default. The verdict must be identical either way — the detector is
/// schedule-independent, so parallelism never changes whether a program is
/// reported racy or clean.
///
/// Requires the `codegen` feature (compile + link + run); runs in CI under both
/// collectors (the CI matrix builds the test binary with the precise Immix GC
/// and with the conservative Boehm GC). Docker/LLVM is unavailable locally, so
/// this leg is CI-only.
#[cfg(feature = "codegen")]
fn check_race_fixture(bin: &str, zap: &std::path::Path, maxprocs: Option<&str>) -> Outcome {
    use std::process::Command;
    let per_test = std::time::Duration::from_secs(60);
    let label = zap.display().to_string();
    let suffix = maxprocs
        .map(|m| format!(" ZAP_MAXPROCS={m}"))
        .unwrap_or_default();
    let name = zap
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Baseline: a plain `zap run` (no instrumentation) — legal, exits 0.
    let mut base = Command::new(bin);
    base.arg("run").arg(zap);
    if let Some(m) = maxprocs {
        base.env("ZAP_MAXPROCS", m);
    }
    let base_out = run_with_timeout(base, per_test, &format!("{label} (no --race{suffix})"));
    if base_out.status.code() != Some(0) {
        return Outcome::Failed(format!(
            "non-race run of {name}{suffix} should exit 0, got {:?} (stderr:\n{})",
            base_out.status.code(),
            String::from_utf8_lossy(&base_out.stderr)
        ));
    }

    // Instrumented: `zap run --race` — the detector's verdict.
    let mut race = Command::new(bin);
    race.arg("run").arg("--race").arg(zap);
    if let Some(m) = maxprocs {
        race.env("ZAP_MAXPROCS", m);
    }
    let race_out = run_with_timeout(race, per_test, &format!("{label} (--race{suffix})"));
    let race_code = race_out.status.code();
    let race_err = String::from_utf8_lossy(&race_out.stderr);
    let race_out_s = String::from_utf8_lossy(&race_out.stdout);

    if name.starts_with("racy_") {
        // A race must be reported and the process must exit non-zero.
        if race_code == Some(0) || race_code.is_none() {
            return Outcome::Failed(format!(
                "--race run of {name}{suffix} should REPORT a race and exit non-zero, got \
                 {race_code:?}\n--- stderr ---\n{race_err}\n--- stdout ---\n{race_out_s}"
            ));
        }
        let lower = race_err.to_lowercase();
        // Each racy fixture pins the KIND of location it must name (a module
        // global -> `counter`; a shared heap object -> `heap`), while staying
        // location-tolerant about the exact source span.
        let token = racy_expected_token(&name);
        if !lower.contains("data race") || !lower.contains(token) {
            return Outcome::Failed(format!(
                "--race report of {name}{suffix} must name a `data race` mentioning \
                 `{token}` (location-tolerant); got stderr:\n{race_err}"
            ));
        }
        return Outcome::Pass;
    }

    if name.starts_with("clean_") {
        // No race: exit 0 and the exact deterministic total.
        if race_code != Some(0) {
            return Outcome::Failed(format!(
                "--race run of clean fixture {name}{suffix} must exit 0 (no false positive), \
                 got {race_code:?}\n--- stderr ---\n{race_err}"
            ));
        }
        // The deterministic total is the LAST line of stdout (clean_single
        // also prints a line from its idle coroutine first), so assert the
        // final line rather than the whole buffer.
        let want = clean_expected_total(&name)
            .expect("every clean_* fixture has a known total; add it to clean_expected_total");
        let last = race_out_s.trim_end().lines().last().unwrap_or("");
        if last != want {
            return Outcome::Failed(format!(
                "--race run of {name}{suffix} printed final line {last:?}, want {want:?}\n\
                 --- full stdout ---\n{}\n--- stderr ---\n{race_err}",
                race_out_s.trim_end()
            ));
        }
        // And identical to the no-`--race` baseline (deterministic program).
        let base_so = String::from_utf8_lossy(&base_out.stdout);
        if base_so.trim_end() != race_out_s.trim_end() {
            return Outcome::Failed(format!(
                "--race changed stdout of {name}{suffix}:\n--- no --race ---\n{}\n\
                 --- --race ---\n{}",
                base_so.trim_end(),
                race_out_s.trim_end()
            ));
        }
        return Outcome::Pass;
    }

    Outcome::Failed(format!(
        "race fixture {name} is neither `racy_*` nor `clean_*`; name it so the \
         detector golden knows the expected verdict"
    ))
}

#[cfg(feature = "codegen")]
#[test]
fn race_detector_reports_races_and_clears_clean_programs() {
    let bin = env!("CARGO_BIN_EXE_zapc");
    run_goldens_parallel(race_fixtures(), |zap| check_race_fixture(bin, zap, None));
}

/// Hardening (#679): the same fixture set under `ZAP_MAXPROCS>1` — true OS-thread
/// parallelism, so the scheduler runs coroutines on several Ms at once and the
/// detector's single global `race_mu` is exercised under real contention. Every
/// verdict must match the serial run (racy reported, clean silent with the same
/// total): the single-lock engine is correct if and only if parallelism changes
/// nothing. A regression here (a torn clock, a missed edge under contention)
/// surfaces as a false positive on a clean fixture or a lost report on a racy
/// one. CI-only (needs codegen); runs under both collectors via the matrix.
#[cfg(feature = "codegen")]
#[test]
fn race_detector_correct_under_parallelism() {
    let bin = env!("CARGO_BIN_EXE_zapc");
    run_goldens_parallel(race_fixtures(), |zap| {
        check_race_fixture(bin, zap, Some("4"))
    });
}
