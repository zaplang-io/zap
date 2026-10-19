//! Precise GC (ticket #48) behavioral tests that need the Immix collector and a
//! runtime knob, so they run only under the `gc-immix` build (and the `llvm`
//! toolchain). The behavior-identical golden programs (`gc_alloc_stress`,
//! `gc_large_live_set`, `gc_parked_coroutine`, and the pre-existing `gc_stress`)
//! run under both collectors via the ordinary golden harness; these assert the
//! precise collector additionally *reclaims* unreachable garbage.

#![cfg(all(feature = "llvm", feature = "gc-immix"))]

use std::process::Command;

/// Compile and run `src` with an Immix heap cap (`ZAP_GC_HEAP_LIMIT`, bytes) and
/// return (success, stdout, stderr).
fn run_capped(src: &str, limit: u64) -> (bool, String, String) {
    // Unique per call: the tests run in parallel in one process, so a shared
    // path would race.
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zapgc-test-{}-{}", std::process::id(), seq));
    std::fs::create_dir_all(&dir).unwrap();
    // Unique stem: `zapc run` derives the compiled executable name from the
    // source stem (temp_dir/zap-<stem>), so two parallel runs need distinct
    // stems or they clobber each other's executable.
    let path = dir.join(format!("prog{}_{}.zap", std::process::id(), seq));
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_zapc"))
        .arg("run")
        .arg(&path)
        .env("ZAP_GC_HEAP_LIMIT", limit.to_string())
        .output()
        .expect("run zapc");
    let _ = std::fs::remove_dir_all(&dir);
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Reclamation: a program that allocates far more total garbage than the heap
/// cap, but keeps only a tiny live set, must complete within the cap — which is
/// only possible if the collector reclaims the unreachable objects and reuses
/// their space. Without collection the mapped heap would grow past the cap and
/// the runtime would abort. This is the "collection actually frees garbage"
/// guarantee that a tracing collector provides for any unreachable subgraph,
/// cycles included (user-level cycle literals await `None`/struct-field
/// mutation front-end support, so are not expressed here).
#[test]
fn immix_reclaims_unreachable_garbage() {
    // ~300k short-lived 3-element lists (+ their data buffers) = tens of MB
    // allocated in total; the live set is just the loop counter. (Kept under the
    // pre-existing dynamic-alloca-in-long-loops stack-overflow ceiling, which is
    // a front-end codegen issue unrelated to the GC.)
    let src = "def main():\n    \
                   auto n = 0\n    \
                   for i in range(0, 300000):\n        \
                       auto xs = [i, i, i]\n        \
                       n = n + xs[0]\n    \
                   print(n)\n";
    // 24 MiB mapped cap: the ~30 MB of total garbage cannot fit without the
    // collector reclaiming and reusing freed blocks; a working collector
    // plateaus near the live set + one threshold, well under the cap.
    let (ok, stdout, stderr) = run_capped(src, 24 * 1024 * 1024);
    assert!(
        ok,
        "capped program should complete via collection; stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(
        stdout.trim_end(),
        "44999850000",
        "stdout={stdout:?} stderr={stderr:?}"
    );
}

/// Reclamation across a parked coroutine: `main` churns garbage under a cap
/// while two workers are parked on an unbuffered channel holding heap strings.
/// Completing within the cap proves the churned garbage is reclaimed AND the
/// parked coroutines' live strings are retained precisely (they print correctly).
#[test]
fn immix_reclaims_with_parked_coroutines() {
    let src = "def worker(channel[str] ch, str msg):\n    ch <- msg\n\n\
               def main():\n    \
                   auto ch = channel[str]()\n    \
                   spawn worker(ch, \"kept\")\n    \
                   spawn worker(ch, \"kept\")\n    \
                   auto n = 0\n    \
                   for i in range(0, 300000):\n        \
                       auto xs = [i, i, i]\n        \
                       n = n + xs[0]\n    \
                   auto a = <-ch\n    \
                   auto b = <-ch\n    \
                   print(a)\n    \
                   print(b)\n";
    let (ok, stdout, stderr) = run_capped(src, 24 * 1024 * 1024);
    assert!(
        ok,
        "capped program with parked coroutines should complete; stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(stdout.trim_end(), "kept\nkept");
}
