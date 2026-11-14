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
    run_timed_opts(name, src, None)
}

/// Like [`run_timed`], but when `lto` is `Some((clang, llvm_link))` the compile
/// runs under the `ZAP_LTO` build flag (I#841, D-0149) with those tools, so the
/// runtime fast paths are inlined into the program. Used by the LTO trend test
/// to compare the same workload with and without runtime inlining.
fn run_timed_opts(name: &str, src: &str, lto: Option<(&str, &str, &str)>) -> (String, Duration) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zapperf-{}-{}", std::process::id(), seq));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}_{}_{}.zap", std::process::id(), seq));
    std::fs::write(&path, src).unwrap();

    let start = Instant::now();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zapc"));
    cmd.arg("run").arg(&path);
    if let Some((clang, llvm_link, opt)) = lto {
        cmd.env("ZAP_LTO", "1")
            .env("ZAP_LTO_CC", clang)
            .env("ZAP_LLVM_LINK", llvm_link)
            .env("ZAP_OPT", opt);
    }
    let mut child = cmd
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
        // keys inserted then looked up. Exercises the cached-key-hash probe/resize
        // path (#837 lever 1) and the one-object string keys (lever 2).
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
        // The dict-core cost in isolation (#837 split): the SAME 100k insert +
        // lookup workload with INT keys — no string objects, so no key-hash,
        // byte-compare or string-tracing overhead. Reported next to `dict_string`
        // so each CI run shows the string-key remainder (`dict_string -
        // fstring_only - dict_int`) the issue reads, not absolute ms.
        Bench {
            name: "dict_int",
            src: "def main():\n    dict[int64, int64] d = {}\n    int64 i = 0\n    \
                  while i < 100000:\n        d[i] = i\n        i = i + 1\n    \
                  int64 s = 0\n    i = 0\n    \
                  while i < 100000:\n        s = s + d[i]\n        i = i + 1\n    \
                  print(s)\n",
            expect: "4999950000",
            work: 200_000,
            unit: "ops",
            floor_per_sec: 1_000.0,
        },
        // The string-CONSTRUCTION cost in isolation (#837 split): build 100k
        // `f"k{i}"` and sum their codepoint lengths, no dict. Exercises literal
        // interning (lever 3: the `"k"` prefix is now a shared constant, not a
        // per-iteration allocation) and the one-object layout (lever 2: each
        // formatted piece is one allocation, not two). sum of len(f"k{i}") for
        // i in 0..100000 = 100000*1 + (10*1+90*2+900*3+9000*4+90000*5) = 588890.
        Bench {
            name: "fstring_only",
            src: "def main():\n    int64 s = 0\n    int64 i = 0\n    \
                  while i < 100000:\n        s = s + len(f\"k{i}\")\n        i = i + 1\n    \
                  print(s)\n",
            expect: "588890",
            work: 100_000,
            unit: "strings",
            floor_per_sec: 1_000.0,
        },
        // ARRAY REDUCTION, whole-array (#911): sum a 20M-element i64 array. The
        // reducer is an 8-way-unrolled multi-accumulator loop the compiler packs
        // into SIMD, so this is bandwidth-bound rather than add-latency-bound.
        // sum of 0..20_000_000-1 = n*(n-1)/2.
        Bench {
            name: "array_sum_1d",
            src: "def main():\n    print(arange[i64](0, 20000000).sum())\n",
            expect: "199999990000000",
            work: 20_000_000,
            unit: "elems",
            floor_per_sec: 1_000_000.0,
        },
        // ARRAY REDUCTION, axis= (#911): sum axis 0 of a (1000, 20000) view.
        // inner=20000 > 1, so the vectorized contiguous j-loop (loop-interchange)
        // runs; the grand total equals the whole-array sum above.
        Bench {
            name: "array_sum_axis",
            src: "def main():\n    auto m = arange[i64](0, 20000000).reshape(1000, 20000)\n    \
                  print(sum(m.sum(axis=0)))\n",
            expect: "199999990000000",
            work: 20_000_000,
            unit: "elems",
            floor_per_sec: 1_000_000.0,
        },
        // MATMUL FLOOR (#913): a 256x256 i64 matrix product — the own blocked+SIMD
        // floor kernel (the scalar triple loop the dispatch picks when BLAS is off
        // / for integers). ones@ones = every cell N, so the sum is N^3. work is
        // the multiply-accumulate count N^3.
        Bench {
            name: "array_matmul_floor",
            src: "def main():\n    print(sum(ones[i64]((256, 256)) @ ones[i64]((256, 256))))\n",
            expect: "16777216",
            work: 16_777_216,
            unit: "macs",
            floor_per_sec: 1_000_000.0,
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

/// Resolve an LLVM 18 tool (`$LLVM_SYS_180_PREFIX/bin/<name>`, then `<name>-18`,
/// `<name>`, `/usr/lib/llvm-18/bin/<name>`), or `None`. Mirrors `tests/lto.rs`.
fn find_llvm_tool(name: &str) -> Option<String> {
    let mut cands: Vec<String> = Vec::new();
    if let Ok(prefix) = std::env::var("LLVM_SYS_180_PREFIX") {
        cands.push(format!("{prefix}/bin/{name}"));
    }
    cands.push(format!("{name}-18"));
    cands.push(name.to_string());
    cands.push(format!("/usr/lib/llvm-18/bin/{name}"));
    for cand in cands {
        let ok = Command::new(&cand)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return Some(cand);
        }
    }
    None
}

/// The `(clang, llvm-link, opt)` LLVM 18 tools for the LTO build, or `None` when
/// the toolchain is absent (skip the LTO trend test).
fn lto_tools() -> Option<(String, String, String)> {
    Some((
        find_llvm_tool("clang")?,
        find_llvm_tool("llvm-link")?,
        find_llvm_tool("opt")?,
    ))
}

/// Look up a bench's source + expected output by name.
fn bench_src(name: &str) -> (&'static str, &'static str) {
    let b = benches()
        .into_iter()
        .find(|b| b.name == name)
        .unwrap_or_else(|| panic!("no bench named `{name}`"));
    (b.src, b.expect)
}

/// LTO runtime-inlining TREND (I#841, D-0149). Runs the dict workloads with and
/// without the `ZAP_LTO` build flag and records the speedup from inlining the
/// runtime fast paths into the hot `d[k]` loop. Like the rest of this harness it
/// asserts correctness and only a LENIENT, machine-independent relation — never
/// an exact wall-clock or a fixed speedup (CI machines vary, and the LTO link
/// time is not part of the measured run). The deterministic inlining guard is
/// `tests/lto.rs`; this is the standing wall-clock record of the win.
///
/// Skips when no LLVM-18 clang is on PATH (a dev host without the toolchain); CI
/// installs `clang-18`, so the trend is recorded there.
#[test]
fn perf_lto_dict_inlining_trend() {
    let Some((clang, llvm_link, opt)) = lto_tools() else {
        eprintln!(
            "SKIP perf_lto_dict_inlining_trend: no LLVM 18 clang + llvm-link + opt on PATH \
             (the LTO build needs all three; CI provides the LLVM 18 toolchain)"
        );
        return;
    };

    let mut report = String::new();
    report.push_str("\n### Zap LTO runtime-inlining trend (#841)\n\n");
    report.push_str(
        "Same dict workload with and without `ZAP_LTO`. Speedup is a TREND signal \
         (not gated); the deterministic inlining guard is the `tests/lto.rs` \
         disassembly assertion.\n\n",
    );
    report.push_str("| bench | non-LTO (ms) | LTO (ms) | speedup |\n");
    report.push_str("|---|---:|---:|---:|\n");

    for name in ["dict_int", "dict_string"] {
        let (src, expect) = bench_src(name);

        let (base_out, base_dur) = run_timed_opts(name, src, None);
        assert_eq!(
            base_out, expect,
            "LTO trend: non-LTO `{name}` produced the wrong result (correctness gate)"
        );

        let (lto_out, lto_dur) = run_timed_opts(name, src, Some((&clang, &llvm_link, &opt)));
        assert_eq!(
            lto_out, expect,
            "LTO trend: `{name}` under ZAP_LTO produced the wrong result — the LTO \
             build must stay functionally identical (correctness gate)"
        );

        // REPORT ONLY — never a pass/fail gate. These timings are whole-`zapc
        // run` wall-clocks (compile + run), and the LTO compile (llvm-link +
        // `opt -O2` + clang) is much slower than the object-link compile, so for
        // these short benchmarks the build time dominates the runtime and a
        // compile-vs-run wall-clock comparison says nothing about the inlining
        // win. The deterministic proof that the dict fast path inlines is
        // `tests/lto.rs` (a disassembly assertion); this table is only a trend
        // line in the CI summary, held to the GC-golden rule that CI never gates
        // on wall-clock. The correctness checks above remain the real gate.
        let base_s = base_dur.as_secs_f64();
        let lto_s = lto_dur.as_secs_f64();
        let speedup = base_s / lto_s.max(1e-9);
        report.push_str(&format!(
            "| {} | {:.1} | {:.1} | {:.2}× |\n",
            name,
            base_s * 1e3,
            lto_s * 1e3,
            speedup,
        ));
    }

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
