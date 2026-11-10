//! Performance benchmark harness (#568, D-0144; the baseline for GC Immix++ epic
//! #714 and the benchmark hook called out in #422).
//!
//! WHY THIS EXISTS. An external language review (#568) measured that the default
//! calling convention paid a non-inlinable `zap_safepoint` call three times per
//! call plus a shadow-frame setup even in GC-free integer code, so recursive
//! `fib` ran ~10× slower than C. The fix (inlined back-edge safepoint polls +
//! frame skip for provably-GC-free functions) is guarded *deterministically* by
//! the codegen IR tests in `crates/codegen` (no frame / one entry poll for a
//! GC-free function) and the runtime safety/liveness tests in `gc.rs` and the
//! `*preempt*` goldens. THIS harness is the standing wall-clock TREND record: it
//! runs a fixed set of benches, checks each produces the correct result, prints a
//! report (also appended to `$GITHUB_STEP_SUMMARY` so every CI run surfaces the
//! numbers), and asserts only lenient, machine-independent floors that catch a
//! hang or a catastrophic structural regression — never an exact wall-clock or
//! throughput target, which would flake across CI machines (the GC-golden
//! determinism rule, extended to perf).
//!
//! Needs the full LLVM toolchain + the precise collector, so it is gated to the
//! same build as `gc.rs`.

#![cfg(all(feature = "codegen", feature = "gc-immix"))]

use std::io::Write;
use std::process::Command;
use std::time::{Duration, Instant};

/// Hard ceiling per bench: a structural regression that deadlocks (e.g. a loop
/// that no longer reaches a safepoint while the collector waits) would otherwise
/// hang until the CI job itself times out. Kill and fail instead. Generous so a
/// slow/loaded runner never trips it on a healthy build.
const BENCH_TIMEOUT: Duration = Duration::from_secs(120);

/// Compile and run `src`, returning `(stdout_trimmed, wall_time)`. Panics (fails
/// the test) on a compile/run error or on exceeding [`BENCH_TIMEOUT`].
fn run_timed(name: &str, src: &str) -> (String, Duration) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zapperf-{}-{}", std::process::id(), seq));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}_{}_{}.zap", std::process::id(), seq));
    std::fs::write(&path, src).unwrap();

    let start = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_zapc"))
        .arg("run")
        .arg(&path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn zapc run");

    // Poll for completion so a hung bench is killed rather than wedging the job.
    let output = loop {
        if let Some(_status) = child.try_wait().expect("try_wait") {
            break child.wait_with_output().expect("wait_with_output");
        }
        if start.elapsed() > BENCH_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&dir);
            panic!(
                "bench `{name}` exceeded {BENCH_TIMEOUT:?} — likely a safepoint/STW \
                 liveness regression (a loop that no longer polls)"
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let elapsed = start.elapsed();
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string();
    assert!(
        output.status.success(),
        "bench `{name}` must compile and run cleanly; stdout={stdout:?} stderr={:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    (stdout, elapsed)
}

/// One benchmark: source, the exact stdout it must print, the logical work count
/// (calls or iterations) for the throughput figure, and its unit label.
struct Bench {
    name: &'static str,
    src: &'static str,
    expect: &'static str,
    work: u64,
    unit: &'static str,
    /// Lenient throughput floor (work-units per second). Only a hang or a
    /// catastrophic (orders-of-magnitude) regression falls below it.
    floor_per_sec: f64,
}

fn benches() -> Vec<Bench> {
    vec![
        // Call-heavy, GC-FREE: the headline #568 case. fib(30) = 832040, with
        // ~2.69M calls (2*fib(31)-1). Each call now pays one inlined entry poll,
        // not three non-inlinable safepoint calls + a frame.
        Bench {
            name: "fib_recursive",
            src: "def fib(int64 n) -> int64:\n    if n < 2:\n        return n\n    \
                  return fib(n - 1) + fib(n - 2)\n\
                  def main():\n    print(fib(30))\n",
            expect: "832040",
            work: 2_692_537, // 2*fib(31)-1
            unit: "calls",
            floor_per_sec: 50_000.0,
        },
        // Call-heavy with a loop: a tiny leaf called 10M times — stresses the
        // per-call entry poll in isolation (no recursion depth, no allocation).
        Bench {
            name: "leaf_call_loop",
            src: "def inc(int64 x) -> int64:\n    return x + 1\n\
                  def main():\n    int64 s = 0\n    int64 i = 0\n    \
                  while i < 10000000:\n        s = inc(s)\n        i = i + 1\n    \
                  print(s)\n",
            expect: "10000000",
            work: 10_000_000,
            unit: "calls",
            floor_per_sec: 50_000.0,
        },
        // GC-FREE, call-FREE loop: the baseline for per-iteration cost. Its only
        // safepoint is the inlined loop back-edge poll. s = n*(n-1)/2.
        Bench {
            name: "tight_loop",
            src: "def main():\n    int64 s = 0\n    int64 i = 0\n    \
                  while i < 30000000:\n        s = s + i\n        i = i + 1\n    \
                  print(s)\n",
            expect: "449999985000000",
            work: 30_000_000,
            unit: "iters",
            floor_per_sec: 100_000.0,
        },
        // GC-HEAVY mixed workload (the review's dict+string case): 100k string
        // keys inserted then looked up. Exercises the frame/root path that is
        // intentionally UNCHANGED, so it is a regression guard on the other side.
        // sum of lookups = 0+1+...+99999 = 4999950000.
        Bench {
            name: "dict_string",
            src: "def main():\n    dict[str, int64] d = {}\n    int64 i = 0\n    \
                  while i < 100000:\n        d[f\"k{i}\"] = i\n        i = i + 1\n    \
                  int64 s = 0\n    i = 0\n    \
                  while i < 100000:\n        s = s + d[f\"k{i}\"]\n        i = i + 1\n    \
                  print(s)\n",
            expect: "4999950000",
            work: 200_000, // 100k inserts + 100k lookups
            unit: "ops",
            floor_per_sec: 1_000.0,
        },
    ]
}

#[test]
fn perf_bench_suite() {
    let mut report = String::new();
    report.push_str("### Zap perf benchmarks (#568 / #714 baseline)\n\n");
    report.push_str("Wall-clock is a TREND signal, not a gate (CI machines vary); ");
    report.push_str("the deterministic win/regression guards are the codegen IR tests.\n\n");
    report.push_str("| bench | work | wall (ms) | throughput |\n");
    report.push_str("|---|---:|---:|---:|\n");

    let mut fib_ns_per_call = None;
    let mut loop_ns_per_iter = None;

    for b in benches() {
        let (out, dur) = run_timed(b.name, b.src);
        assert_eq!(
            out, b.expect,
            "bench `{}` produced the wrong result (correctness gate)",
            b.name
        );
        let secs = dur.as_secs_f64();
        let per_sec = b.work as f64 / secs.max(1e-9);
        let ns_per = secs * 1e9 / b.work as f64;
        report.push_str(&format!(
            "| {} | {} {} | {:.1} | {:.2}M {}/s ({:.1} ns/{}) |\n",
            b.name,
            b.work,
            b.unit,
            secs * 1e3,
            per_sec / 1e6,
            b.unit,
            ns_per,
            b.unit.trim_end_matches('s'),
        ));
        assert!(
            per_sec > b.floor_per_sec,
            "bench `{}` throughput {:.0} {}/s fell below the catastrophic-regression \
             floor {:.0} {}/s — a structural per-call/safepoint regression, not CI noise",
            b.name,
            per_sec,
            b.unit,
            b.floor_per_sec,
            b.unit,
        );
        if b.name == "fib_recursive" {
            fib_ns_per_call = Some(ns_per);
        }
        if b.name == "tight_loop" {
            loop_ns_per_iter = Some(ns_per);
        }
    }

    // The #568 headline trend metric: cost of a GC-free CALL relative to a
    // GC-free loop ITERATION on the same machine/build. Before the fix a call
    // carried the extra non-inlinable safepoint calls + frame, so this ratio was
    // large; after it, a call is close to a loop iteration. Printed for trend; not
    // asserted (the ratio's absolute value is still machine-sensitive).
    if let (Some(call), Some(iter)) = (fib_ns_per_call, loop_ns_per_iter) {
        report.push_str(&format!(
            "\n**GC-free call / loop-iter cost ratio:** {:.2}× ({:.1} ns/call vs {:.1} ns/iter)\n",
            call / iter.max(1e-9),
            call,
            iter,
        ));
    }

    // Always visible on the console (failures) and in the GitHub Actions run
    // summary (every run) so wins and regressions are tracked over time (#422).
    eprintln!("\n{report}");
    if let Ok(path) = std::env::var("GITHUB_STEP_SUMMARY") {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
        {
            let _ = f.write_all(report.as_bytes());
        }
    }
}
