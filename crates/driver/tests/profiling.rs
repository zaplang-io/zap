//! CPU profiler integration test (ticket #78, D-0058).
//!
//! The folded-stack output is a side-effect file (not stdout), so it cannot be
//! checked by the stdout/exit-code golden harness; it is checked here. A program
//! runs a CPU-bound coroutine in a distinctively named function (`hot_loop`)
//! with `ZAP_PROFILE=cpu` and `ZAP_PROFILE_OUT` set to a temp file. After it
//! exits we read the folded profile and assert it is non-empty and names
//! `hot_loop` — i.e. the sampler captured on-CPU stacks and offline
//! symbolication (via -rdynamic + backtrace_symbols) resolved the Zap function
//! name. The loop is sized so that many sample intervals elapse while it is
//! on-CPU; we never assert a sample COUNT (that is non-deterministic, like the
//! GC goldens). It is a pure-arithmetic loop (no allocation, no sleep) so it is
//! unaffected by GC pacing.
//!
//! Requires the `llvm` feature and an LLVM toolchain (run in the dev container,
//! `make docker-test`).

#![cfg(feature = "codegen")]

use std::io::Write;
use std::process::Command;

#[test]
fn cpu_profile_folded_names_hot_function() {
    // Spawn a CPU-bound `hot_loop` sized to run a few hundred ms (so many sample
    // intervals elapse) while the profiler samples; ZAP_PROFILE auto-starts at
    // the first spawn and the folded profile is flushed at program exit.
    let src = "\
def hot_loop(Atomic[int64] c, bytes wg):
    auto i = 0
    while i < 60000000:
        i = i + 1
    c.add(1)
    sync.done(wg)

def main() -> int64:
    Atomic[int64] c = Atomic(0)
    auto wg = sync.waitgroup()
    sync.add(wg, 1)
    spawn hot_loop(c, wg)
    sync.wait(wg)
    print(c.load())
    return 0
";
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let zap = dir.join("cpu_profile.zap");
    {
        let mut f = std::fs::File::create(&zap).expect("create temp .zap");
        f.write_all(src.as_bytes()).expect("write temp .zap");
    }
    let exe = dir.join("cpu_profile_bin");
    let folded = dir.join("cpu_profile.folded");
    let _ = std::fs::remove_file(&folded);

    let bin = env!("CARGO_BIN_EXE_zapc");
    let build = Command::new(bin)
        .arg("build")
        .arg(&zap)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("zapc build");
    assert!(
        build.status.success(),
        "zapc build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let output = Command::new(&exe)
        .env("ZAP_PROFILE", "cpu")
        .env("ZAP_PROFILE_HZ", "1000")
        .env("ZAP_PROFILE_OUT", &folded)
        .output()
        .expect("run profiled binary");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "profiled program should exit 0; stderr:\n{stderr}"
    );
    // The end-of-run banner proves the profiler stopped and flushed.
    assert!(
        stderr.contains("zap CPU profile"),
        "expected the profiler banner on stderr, got:\n{stderr}"
    );

    let got = std::fs::read_to_string(&folded).unwrap_or_else(|e| {
        panic!("folded profile {} not written: {e}", folded.display());
    });
    assert!(
        !got.trim().is_empty(),
        "folded profile is empty (no samples captured):\n{got}"
    );
    // Offline symbolication should resolve the hot Zap function by name.
    assert!(
        got.contains("hot_loop"),
        "folded profile should name the hot function `hot_loop`; got:\n{got}"
    );
}
