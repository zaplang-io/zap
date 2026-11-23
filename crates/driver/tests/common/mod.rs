//! Shared end-to-end test harness for the `.zap` corpora.
//!
//! The golden ([`golden.rs`]), conformance ([`conformance.rs`]) and
//! CPython-differential ([`diff_cpython.rs`]) integration tests all compile
//! `.zap` programs through `zapc` → LLVM → clang → link → execute, one program
//! per fixture. That work is embarrassingly parallel and dominates the wall-clock
//! of every heavy CI leg, so this module owns the one place all three corpora use
//! to run it: a bounded worker pool with a timeout guard, the compile/run split,
//! and the width-selection policy.
//!
//! ## Two-pass width policy (zaplang-io/zap#1305)
//!
//! The pool runs each corpus in **two passes**:
//!
//! * a **wide** pass for ordinary fixtures, at `ZAP_GOLDEN_JOBS` workers
//!   (default **2**); and
//! * a **conservative** pass for the sensitivity-classified subset, at
//!   `ZAP_GOLDEN_SENSITIVE_JOBS` workers (default `min(2, wide)`, and never more
//!   than the wide width).
//!
//! The wide default stays at 2 so a local `cargo test` never suddenly saturates a
//! contributor's machine; CI drives the widening by exporting
//! `ZAP_GOLDEN_JOBS=$(nproc)` on the heavy legs. Because the two passes run back
//! to back (ordinary drains before sensitive starts), peak concurrency is bounded
//! by the wide width.
//!
//! A fixture is **sensitive** ([`is_sensitive`]) when it is one of the
//! concurrency / GC / scheduler timing-sensitive programs with a flake history
//! under load (the `#223` GC-deadlock family, the boehm-flaky `#295`, the
//! quarantined aarch64 stress goldens) OR when it still runs under a single
//! compile+link+run deadline where linker starvation on a loaded runner could be
//! misreported as a hang (`#1113`) — namely the whole `tests/race/*` corpus. See
//! [`is_sensitive_name`] for the exact rule.
//!
//! Local/non-CI callers are unaffected by default: with no env set, both passes
//! run at width 2, exactly as before.

#![allow(dead_code)]

use std::path::Path;
// Only the codegen-gated pool functions below handle owned paths; gate the
// import so the host (no-LLVM) build doesn't warn it unused under `-D warnings`.
#[cfg(feature = "codegen")]
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Width selection — pure, host-testable (no `codegen` feature needed).
// ---------------------------------------------------------------------------

/// Parse a worker-count env value: a trimmed, positive integer, else `default`.
/// A missing / blank / non-numeric / zero value falls back to `default`, so a
/// typo never silently serializes (or over-widens) the suite.
pub fn parse_jobs(value: Option<&str>, default: usize) -> usize {
    value
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(default)
}

/// Wide pool width for ordinary fixtures: `ZAP_GOLDEN_JOBS`, default **2**.
/// `ZAP_GOLDEN_JOBS=1` restores fully serial execution; CI sets it to the runner
/// core count (`$(nproc)`).
pub fn golden_jobs() -> usize {
    parse_jobs(std::env::var("ZAP_GOLDEN_JOBS").ok().as_deref(), 2)
}

/// Conservative pool width for the sensitive subset: `ZAP_GOLDEN_SENSITIVE_JOBS`,
/// default `min(2, golden_jobs())`. Clamped to never exceed the wide width, so
/// `ZAP_GOLDEN_JOBS=1` (serial) also serializes the sensitive pass.
pub fn sensitive_jobs() -> usize {
    let wide = golden_jobs();
    let default = wide.min(2);
    parse_jobs(
        std::env::var("ZAP_GOLDEN_SENSITIVE_JOBS").ok().as_deref(),
        default,
    )
    .min(wide)
}

// ---------------------------------------------------------------------------
// Sensitivity classification.
// ---------------------------------------------------------------------------

/// Classify a fixture path string as sensitive (must run in the conservative
/// lane) by name alone — the pure, host-testable core of [`is_sensitive`].
///
/// Several independent signals, any of which qualifies:
///
/// * the fixture lives under `tests/race/` — that corpus still runs a single
///   `zap run` (compile+link+run) under one deadline, so at a wide width a
///   linker starved of CPU could be misreported as a program hang (`#1113`);
///   its programs are also concurrency-heavy; and
/// * the fixture name contains `stress` or `preempt` — the scheduler / GC /
///   concurrency stress + preemption programs that are timing-sensitive under
///   load and have a flake history (`#223` family, `#295`); and
/// * the fixture name contains `timeout`, `deadline`, or `cancel` — the
///   scope-deadline / cancellation / spawn-timeout programs assert on a *wall-
///   clock* deadline enforced by the scheduler, so at the wide `$(nproc)` width
///   CPU starvation makes the program itself miss its deadline and trip the 60s
///   `[run]` ceiling (observed after the `#1305` widening: the `scope_timeout*`
///   and `spawn_forms_timeout` conformance fixtures, and the net/TLS
///   `*_deadline` / `*_timeout` / `*_cancel` goldens). These are genuinely
///   timing-sensitive at *run* time, independent of the `#1113` build issue.
///
/// A sensitive fixture whose name matches none of these still has the explicit
/// `<fixture>.serial` sidecar escape hatch (see [`is_sensitive`]) — used, e.g.,
/// for `scope_cleanup`, which is scheduler-sensitive but carries no marker word.
///
/// Erring toward classifying a borderline fixture as sensitive only costs it the
/// wide lane's extra parallelism; it never risks correctness, so the rule is
/// deliberately conservative. (`scope`/`scoped` is deliberately NOT a marker: it
/// would pull in deterministic `scoped_*` drop-glue/`with` fixtures that are not
/// timing-sensitive.)
pub fn is_sensitive_name(path: &str) -> bool {
    let in_race_corpus = path.contains("/race/") || path.contains("\\race\\");
    let stress_or_preempt = path.contains("stress") || path.contains("preempt");
    // Wall-clock-deadline / cancellation programs: the scheduler enforces the
    // deadline, so core starvation at the wide width makes them miss it (#1305).
    let timing = path.contains("timeout") || path.contains("deadline") || path.contains("cancel");
    in_race_corpus || stress_or_preempt || timing
}

/// Whether a fixture must run in the conservative (low-width) lane. Combines the
/// name-based rule ([`is_sensitive_name`]) with an explicit `<fixture>.serial`
/// sidecar escape hatch — the same sidecar convention as `.skip_arch`/`.env` —
/// so a timing-sensitive fixture whose name doesn't match can still be pinned to
/// the conservative lane without code changes.
pub fn is_sensitive(path: &Path) -> bool {
    if is_sensitive_name(&path.to_string_lossy()) {
        return true;
    }
    path.with_extension("serial").exists()
}

// ---------------------------------------------------------------------------
// Per-fixture outcome.
// ---------------------------------------------------------------------------

/// Outcome of running a single fixture. `Skipped` covers an architecture-
/// quarantined fixture (see golden.rs's `skipped_on_this_arch`); `Failed`
/// carries the human-readable mismatch/exit/stderr message to surface.
pub enum Outcome {
    Pass,
    Skipped,
    Failed(String),
}

// ---------------------------------------------------------------------------
// Process execution with a hang guard + the compile/run deadline split.
// (Codegen-only: these spawn `zapc` and the compiled program.)
// ---------------------------------------------------------------------------

/// Run `cmd`, capturing stdout/stderr, but FAIL FAST if it does not finish
/// within `timeout` — a hung fixture (e.g. a wait on a signal/channel that never
/// fires) then fails deterministically instead of stalling the whole CI run until
/// the job's global timeout. stdout/stderr are drained on threads so the child
/// never blocks on a full pipe buffer.
#[cfg(feature = "codegen")]
pub fn run_with_timeout(
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
                        "{label} did not finish within {timeout:?} — timed out and \
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
pub fn compile_then_run(
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
pub fn parse_env_sidecar(path: &std::path::Path, projdir: Option<&str>) -> Vec<(String, String)> {
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

// ---------------------------------------------------------------------------
// The two-pass bounded worker pool.
// ---------------------------------------------------------------------------

/// Run `work` over `items` on a bounded pool of `jobs` worker threads. Workers
/// pull fixtures off a shared index so the pool stays bounded (no
/// thread-per-fixture fan-out). Outcomes are collected and, after the pool
/// drains, failures are returned in the original (sorted) `items` order, so a
/// failure prints identically regardless of scheduling and concurrent fixtures
/// never interleave into unreadable output. A hang still panics loudly from
/// inside [`run_with_timeout`], failing the test fast rather than being silenced.
#[cfg(feature = "codegen")]
fn run_pool<F>(items: &[PathBuf], jobs: usize, work: &F) -> Vec<String>
where
    F: Fn(&std::path::Path) -> Outcome + Sync,
{
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    if items.is_empty() {
        return Vec::new();
    }
    let jobs = jobs.min(items.len()).max(1);
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Outcome)>> = Mutex::new(Vec::with_capacity(items.len()));

    std::thread::scope(|scope| {
        for _ in 0..jobs {
            let next = &next;
            let results = &results;
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
        if let Outcome::Failed(msg) = outcome {
            failures.push(format!("{}:\n{msg}", items[i].display()));
        }
    }
    failures
}

/// Run an end-to-end corpus in two passes (zaplang-io/zap#1305): a **wide** pass
/// ([`golden_jobs`]) over the ordinary fixtures and a **conservative** pass
/// ([`sensitive_jobs`]) over the sensitivity-classified subset ([`is_sensitive`]),
/// then assert the aggregated outcome. The passes run back to back so peak
/// concurrency never exceeds the wide width.
///
/// An optional `ZAP_GOLDEN_ONLY` allowlist (comma-separated path substrings)
/// filters the corpus first — CI's "Test under LTO" step sets it to a small,
/// representative slice so the LTO build is exercised end-to-end without
/// re-linking the entire corpus under `-O2 -flto`. Patterns that match nothing
/// contribute nothing; this runner is shared across the golden / project / race /
/// conformance / diff corpora, so an allowlist of golden stems is expected to
/// filter the other corpora to zero.
///
/// Failures from both passes are merged and sorted by fixture path, so the report
/// is deterministic and readable regardless of scheduling or which lane a fixture
/// ran in.
#[cfg(feature = "codegen")]
pub fn run_e2e_parallel<F>(items: Vec<PathBuf>, work: F)
where
    F: Fn(&std::path::Path) -> Outcome + Sync,
{
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

    // Partition into the conservative (sensitive) and wide (ordinary) lanes. The
    // input is already sorted, and `partition` preserves relative order, so each
    // lane stays sorted.
    let (sensitive, ordinary): (Vec<PathBuf>, Vec<PathBuf>) =
        items.into_iter().partition(|p| is_sensitive(p));

    let mut failures = run_pool(&ordinary, golden_jobs(), &work);
    failures.extend(run_pool(&sensitive, sensitive_jobs(), &work));
    // Both lanes are individually sorted; sort the merged failures by their
    // leading path so the report order is deterministic across lanes.
    failures.sort();

    assert!(
        failures.is_empty(),
        "{} end-to-end fixture(s) failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

// ---------------------------------------------------------------------------
// Host-side unit tests for the pure width/classification policy (no LLVM).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_jobs_rules() {
        assert_eq!(parse_jobs(None, 2), 2, "missing -> default");
        assert_eq!(parse_jobs(Some(""), 2), 2, "blank -> default");
        assert_eq!(parse_jobs(Some("   "), 2), 2, "whitespace -> default");
        assert_eq!(
            parse_jobs(Some("0"), 2),
            2,
            "zero -> default (never serialize to 0)"
        );
        assert_eq!(parse_jobs(Some("nope"), 2), 2, "garbage -> default");
        assert_eq!(parse_jobs(Some("1"), 2), 1, "explicit serial honored");
        assert_eq!(parse_jobs(Some("  16 "), 2), 16, "trimmed integer honored");
        assert_eq!(parse_jobs(None, 8), 8, "default is configurable");
    }

    #[test]
    fn sensitive_names_classified() {
        // Stress / preemption fixtures (flake history under load).
        for s in [
            "tests/golden/gc_stress.zap",
            "tests/golden/concurrent_queue_mpsc_coro_stress.zap",
            "tests/golden/immix_evac_coro_stress.zap",
            "tests/golden/scheduler_stress.zap",
            "tests/golden/coroutine_preempt_window_stress.zap",
            "tests/golden/preemption.zap",
            "tests/golden/gc_collect_preempt_stress.zap",
        ] {
            assert!(is_sensitive_name(s), "{s} should be sensitive");
        }
        // The whole race corpus (single-deadline `zap run`, #1113 exposure).
        for s in [
            "tests/race/racy_counter.zap",
            "tests/race/clean_mutex.zap",
            "tests/race/clean_heap_evac.zap",
        ] {
            assert!(
                is_sensitive_name(s),
                "{s} (race corpus) should be sensitive"
            );
        }
        // Wall-clock-deadline / cancellation programs (zaplang-io/zap#1305): the
        // scheduler enforces the deadline, so they starve + miss it at the wide
        // width. Regression source: these starved after the pool widened.
        for s in [
            "tests/conformance/scope_timeout.zap",
            "tests/conformance/scope_timeout_sever.zap",
            "tests/conformance/spawn_forms_timeout.zap",
            "tests/golden/net_connect_deadline.zap",
            "tests/golden/tls_scope_read_cancel.zap",
            "tests/golden/http_client_timeout.zap",
            "tests/golden/process_check_timeout.zap",
        ] {
            assert!(
                is_sensitive_name(s),
                "{s} (timing/deadline) should be sensitive"
            );
        }
    }

    #[test]
    fn ordinary_names_not_classified() {
        // Ordinary functional fixtures — including ones that touch concurrency
        // primitives but are fast, deterministic and not load-sensitive — stay in
        // the wide lane so the suite actually speeds up.
        for s in [
            "tests/golden/hello.zap",
            "tests/golden/fib.zap",
            "tests/golden/dict_word_count.zap",
            "tests/golden/channel_float.zap",
            "tests/golden/atomic_basics.zap",
            "tests/golden/threading_basic.zap",
            "tests/golden/select_stmt.zap",
            "tests/golden/coroutines.zap",
            "tests/conformance/truthiness.zap",
            "tests/diff/arithmetic.zap",
            // `scope`/`scoped` is intentionally NOT a marker: these are
            // deterministic structured-concurrency / drop-glue / `with` fixtures,
            // not wall-clock-deadline tests, so they must stay in the wide lane.
            "tests/golden/scope_join.zap",
            "tests/golden/scope_group.zap",
            "tests/golden/scoped_with_basic.zap",
            "tests/golden/scoped_field_drop_glue.zap",
        ] {
            assert!(!is_sensitive_name(s), "{s} should NOT be sensitive");
        }
    }

    #[test]
    fn serial_sidecar_routes_scope_cleanup_to_conservative_lane() {
        // scope_cleanup carries no marker word, so is_sensitive_name is false …
        assert!(
            !is_sensitive_name("tests/conformance/scope_cleanup.zap"),
            "scope_cleanup should not match a name marker (it uses the sidecar)"
        );
        // … but the committed `scope_cleanup.serial` sidecar must pin it sensitive
        // through the full is_sensitive() path (the #1305 regression fix).
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("crates/driver has a two-level parent")
            .join("tests/conformance/scope_cleanup.zap");
        assert!(
            is_sensitive(&fixture),
            "scope_cleanup.serial sidecar must route {} to the conservative lane",
            fixture.display()
        );
    }

    #[test]
    fn sensitive_jobs_never_exceeds_wide() {
        // The clamp is enforced in `sensitive_jobs()` against `golden_jobs()`;
        // verify the clamp arithmetic directly on representative widths.
        for wide in [1usize, 2, 4, 8, 32] {
            let default = wide.min(2);
            // Explicit request above the wide width is clamped down.
            assert_eq!(parse_jobs(Some("1000"), default).min(wide), wide);
            // Default never exceeds 2 and never exceeds the wide width.
            assert!(default <= 2 && default <= wide);
        }
    }
}
