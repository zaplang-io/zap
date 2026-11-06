//! Precise GC (ticket #48) behavioral tests that need the Immix collector and a
//! runtime knob, so they run only under the `gc-immix` build (and the `llvm`
//! toolchain). The behavior-identical golden programs (`gc_alloc_stress`,
//! `gc_large_live_set`, `gc_parked_coroutine`, and the pre-existing `gc_stress`)
//! run under both collectors via the ordinary golden harness; these assert the
//! precise collector additionally *reclaims* unreachable garbage.

#![cfg(all(feature = "codegen", feature = "gc-immix"))]

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

/// Compile and run `src` with an Immix heap cap and return the full
/// `std::process::Output` (status included), so a test can inspect the exit
/// signal, not just success/stdout/stderr.
fn run_capped_output(src: &str, limit: u64) -> std::process::Output {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(2_000_000);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zapgc-test-{}-{}", std::process::id(), seq));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("prog{}_{}.zap", std::process::id(), seq));
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_zapc"))
        .arg("run")
        .arg(&path)
        .env("ZAP_GC_HEAP_LIMIT", limit.to_string())
        .output()
        .expect("run zapc");
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// OOM policy (D-0118, #697): a program whose LIVE set grows past the heap cap
/// must abort with a clean `zap: out of memory` on stderr and a nonzero exit —
/// NOT a nil-deref SIGSEGV. Before the fix an exhausted allocator returned NULL
/// and generated code stored into it unchecked, faulting at address 0; the fix
/// routes every exhaustion path through `zap_oom`, which prints the message and
/// aborts. This is the deterministic Immix leg (the cap is Immix-only); the Boehm
/// leg — NULL from `GC_MALLOC` when bdwgc's default `GC_oom_fn` returns 0 — shares
/// the same `zap_oom` guard and is described in the PR (bdwgc exhaustion is not
/// deterministically reproducible in a unit test, unlike the explicit cap).
///
/// The list grows without bound, so once the retained elements plus their backing
/// buffer exceed the cap the next collection finds the live set over the limit and
/// `zap_oom` fires; the 100M loop bound is only a safety ceiling — the abort lands
/// after a few MiB of live data, long before it is reached.
#[test]
fn immix_oom_aborts_cleanly_not_segfault() {
    let src = "def main():\n    \
                   list[int64] xs = []\n    \
                   for i in range(0, 100000000):\n        \
                       xs.append(i)\n    \
                   print(len(xs))\n";
    // 8 MiB cap: an ever-growing live list cannot be reclaimed, so it crosses the
    // cap and aborts — deterministic regardless of timing or core count.
    let out = run_capped_output(src, 8 * 1024 * 1024);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "an unbounded live set under the cap must not complete; stdout={stdout:?} stderr={stderr:?}"
    );
    // The clean OOM message is the crux: a wild SIGSEGV prints nothing of ours, so
    // its presence proves the allocator aborted deliberately rather than handing
    // NULL to a generated store (#697).
    assert!(
        stderr.contains("out of memory"),
        "expected a clean 'out of memory' abort, not a crash; stdout={stdout:?} stderr={stderr:?}"
    );
    // The program never reaches its own output.
    assert!(
        !stdout.contains("100000000"),
        "the program must abort before printing; stdout={stdout:?} stderr={stderr:?}"
    );
    // Extra assurance on Unix: the process died by SIGABRT (6, from abort()), never
    // SIGSEGV (11) — the exact wild nil-deref #697 converts into a clean abort.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = out.status.signal() {
            assert_ne!(
                sig, 11,
                "OOM must not be a SIGSEGV; stdout={stdout:?} stderr={stderr:?}"
            );
            assert_eq!(
                sig, 6,
                "OOM should abort via SIGABRT; got signal {sig}; stdout={stdout:?} stderr={stderr:?}"
            );
        }
    }
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

/// Reclamation of monomorphized generic instances (I#154, D-0064). A loop
/// churns far more pointer-bearing generic heap objects (`Optional[str]`,
/// `Pair[int64, str]`) than the heap cap, keeping only a tiny live set.
/// Completing within the cap requires the collector to reclaim them — which it
/// can do correctly only if each monomorphized instance's `ZapTypeDesc` pointer
/// map is right (the "GC metadata is free" theorem: a wrong map would mis-trace
/// the `str` field or follow the `i64` field as a pointer and crash/corrupt).
#[test]
fn immix_reclaims_generic_instances() {
    let src = "struct Pair[A, B]:\n    A first\n    B second\n\n\
               def main():\n    \
                   auto n = 0\n    \
                   for i in range(0, 300000):\n        \
                       Pair[int64, str] p = Pair(i, \"payload\")\n        \
                       Optional[str] o = Optional.Some(p.second)\n        \
                       Optional[int64] q = Optional.Some(p.first)\n        \
                       if o.is_some():\n            \
                           n = n + q.unwrap()\n    \
                   print(n)\n";
    let (ok, stdout, stderr) = run_capped(src, 24 * 1024 * 1024);
    assert!(
        ok,
        "capped generic-churn program should complete via collection; stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(
        stdout.trim_end(),
        "44999850000",
        "stdout={stdout:?} stderr={stderr:?}"
    );
}

/// Compile and run `src` with no heap cap; return (success, stdout, stderr).
fn run(src: &str) -> (bool, String, String) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1_000_000);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zapgc-test-{}-{}", std::process::id(), seq));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("prog{}_{}.zap", std::process::id(), seq));
    std::fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_zapc"))
        .arg("run")
        .arg(&path)
        .output()
        .expect("run zapc");
    let _ = std::fs::remove_dir_all(&dir);
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Finalizers (ticket #92): a registered finalizer must run when its object is
/// collected, exactly once. The `__gc_finalizer_selftest()` runtime hook
/// registers an incrementing stand-in finalizer on 64 unreachable objects,
/// forces two collections, and returns how many finalizers ran — 64 iff the
/// precise collector's post-mark finalizer queue runs each exactly once (no
/// miss, no double-run). This is the Immix parity for the Boehm finalizer that
/// DECREFs a `ZapPy` wrapper's CPython reference; a stand-in counter stands in
/// for `Py_DECREF` because the v1 Python interop gives a real decref no
/// deterministic observable effect (explained in the PR).
#[test]
fn immix_runs_finalizers_once_each() {
    let src = "def main():\n    print(__gc_finalizer_selftest())\n";
    let (ok, stdout, stderr) = run(src);
    assert!(
        ok,
        "finalizer self-test should run; stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(
        stdout.trim_end(),
        "64",
        "stdout={stdout:?} stderr={stderr:?}"
    );
}

/// #586 (residual after #583/#584): the heap-pointer (`str`) fields of a by-value
/// `value struct` held across a collection must stay rooted. Such an aggregate
/// lives in a stack *spill* slot (the D-0051 param/ret spill); its interior `str`
/// pointers are covered by neither the direct SSA roots nor #584's reference-local
/// roots, so before the fix the collector reclaimed them and a later field read
/// dereferenced freed/overwritten memory (SIGSEGV in `zap_str_len`, or a spurious
/// "reclaimed" mismatch once the freed block was reused).
///
/// The tight heap cap forces repeated collections AND block reuse, so a reclaimed
/// field is overwritten and the read deterministically fails when unrooted. The
/// program must instead run to completion — the collector traces the spill's
/// `str` fields in place via the struct's precise descriptor.
#[test]
fn immix_keeps_value_struct_pointer_fields_across_gc() {
    let src = r#"value struct Options:
    str host
    str username
    str password
    str database

def make_options(int64 generation) -> Options:
    return Options(
        host="host-" + str(generation),
        username="user-" + str(generation),
        password="pw-" + str(generation),
        database="db-" + str(generation),
    )

def churn():
    for i in range(0, 20000):
        auto garbage = [str(i), str(i + 1)]
    gc.collect()

def snapshot_options(Options opts, int64 generation):
    if opts.host != "host-" + str(generation):
        raise ValueError("host reclaimed: " + opts.host)
    if opts.username != "user-" + str(generation):
        raise ValueError("username reclaimed: " + opts.username)
    if opts.password != "pw-" + str(generation):
        raise ValueError("password reclaimed: " + opts.password)
    if opts.database != "db-" + str(generation):
        raise ValueError("database reclaimed: " + opts.database)

def main():
    auto config = make_options(0)
    for round in range(0, 4):
        churn()
        snapshot_options(config, 0)
    print("value-struct str fields survive collection")
"#;
    // 8 MiB cap: freed blocks are recycled for the next round's garbage, so a
    // reclaimed `str` field is overwritten before it is read.
    let (ok, stdout, stderr) = run_capped(src, 8 * 1024 * 1024);
    assert!(
        ok,
        "value-struct pointer fields must survive collection; stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(
        stdout.trim_end(),
        "value-struct str fields survive collection",
        "stdout={stdout:?} stderr={stderr:?}"
    );
}

/// Cross-thread root clobber (residual after #583/#584/#586, the redis #50
/// blocker): a live HEAP object held only by the main thread's shadow chain must
/// survive a collection that is triggered while the main thread is blocked inside
/// the runtime (here a `scope:` join).
///
/// The main OS thread runs top-level Zap but is not a coroutine (no `G`), so when
/// it parks it flushes its shadow-stack head into a slot the collector reads. The
/// bug: that slot was a single shared global written by EVERY non-`M` thread
/// (`cur_m == NULL`), not just the main thread. The per-`scope(timeout=...)`
/// deadline helper threads — like the netpoller and other blocking-syscall
/// helpers — run off any scheduler `M`, never push a shadow frame, and flush a
/// NULL head. One of them overwriting the main thread's flushed chain between the
/// flush and the collector's read dropped the main thread's roots, so a heap
/// `Fixture` (and its nested heap `Options` with `str` fields) held by `main`
/// across the join was reclaimed; the next field read hit freed/zeroed memory
/// (SIGSEGV at a small/near-null offset, or a spurious "reclaimed" mismatch once
/// the block was reused). The fix stores each thread's flushed head in its own
/// mutator node so a helper thread's NULL head is inert.
///
/// The workers open `scope(timeout=...)` blocks (spawning the `cur_m == NULL`
/// deadline threads) WHILE `main` is parked on the outer join, and churn under a
/// tight cap to force a collection in that window — deterministically reclaiming
/// the fixture when its root is clobbered. No network is needed: a timed scope
/// alone reproduces the shared-slot overwrite that redis #50 hit via its socket
/// I/O helper threads. The program must instead run to completion with every
/// nested `str` field intact.
#[test]
fn immix_keeps_main_thread_roots_across_cross_thread_collection() {
    let src = r#"struct Options:
    str host
    str username
    str password
    str database

struct Fixture:
    Options config
    str prefix

def make_fixture() -> Fixture:
    auto opts = Options(
        host="127.0.0.1",
        username="user-name",
        password="pass-word",
        database="db-name",
    )
    return Fixture(config=opts, prefix="zap-redis-test:")

def worker():
    auto r = 0
    while r < 6:
        # Each timed scope spawns a deadline helper thread that runs off any
        # scheduler M (cur_m == NULL) and flushes its empty shadow head while the
        # main coroutine is blocked on the outer scope join below.
        scope(timeout=30000):
            auto k = 0
            while k < 3000:
                auto garbage = [str(k), str(k + 1), str(k + 2)]
                k = k + 1
        r = r + 1

def pooling(Fixture f):
    auto cfg = f.config
    cfg.username = "pool-worker"
    scope:
        for i in range(0, 16):
            spawn worker()

def snapshot(Fixture f):
    if f.config.host != "127.0.0.1":
        raise ValueError("host reclaimed: " + f.config.host)
    if f.config.password != "pass-word":
        raise ValueError("password reclaimed: " + f.config.password)
    if f.config.database != "db-name":
        raise ValueError("database reclaimed: " + f.config.database)
    if f.prefix != "zap-redis-test:":
        raise ValueError("prefix reclaimed: " + f.prefix)

def main():
    auto f = make_fixture()
    for round in range(0, 4):
        pooling(f)
        snapshot(f)
    print("main-thread heap roots survive cross-thread collection")
"#;
    // 8 MiB cap: the workers churn far more than the cap between joins, forcing a
    // collection while `main` is parked on the join and a helper thread has
    // flushed its NULL head — reclaiming the unrooted fixture before the fix.
    let (ok, stdout, stderr) = run_capped(src, 8 * 1024 * 1024);
    assert!(
        ok,
        "main-thread heap roots must survive a cross-thread collection; stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(
        stdout.trim_end(),
        "main-thread heap roots survive cross-thread collection",
        "stdout={stdout:?} stderr={stderr:?}"
    );
}

/// Module-global reassignment roots only the CURRENT value (#663, epic #661). A
/// mutable module-level pointer global is reassigned to a fresh large list each
/// round under a heap cap. The slot is a single permanent root that the
/// collector reads move-aware every cycle (`zap_process_slot`), so once a round
/// overwrites the slot the previous round's list is unreachable and reclaimed —
/// the peak plateaus near one live list plus the GC threshold. If reassignment
/// left the old target stale-rooted, every round's list would be retained and
/// the accumulated garbage would blow the cap and abort. Completing within the
/// cap is the deterministic, Immix-only signal that the write path keeps the
/// root tracking the live value with no stale rooting. (Boehm scans the data
/// segment conservatively and is exercised by the behavior-identical dual-leg
/// goldens instead.)
#[test]
fn immix_module_global_reassignment_roots_only_current_value() {
    // 30 rounds × a 60k-element `list[int64]` (~480 KiB of data each). All 30
    // retained would be ~14 MiB, well past the 8 MiB cap; only the current one
    // kept (plus the 4 MiB threshold) stays comfortably under it.
    let src = "list[int64] g = [0]\n\
               def main():\n    \
                   for r in range(0, 30):\n        \
                       list[int64] big = []\n        \
                       for i in range(0, 60000):\n            \
                           big.append(i)\n        \
                       g = big\n    \
                   print(len(g))\n";
    let (ok, stdout, stderr) = run_capped(src, 8 * 1024 * 1024);
    assert!(
        ok,
        "reassigning a module global must drop the prior value so it is reclaimed; \
         stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(
        stdout.trim_end(),
        "60000",
        "the global holds exactly the last-assigned list; stdout={stdout:?} stderr={stderr:?}"
    );
}
