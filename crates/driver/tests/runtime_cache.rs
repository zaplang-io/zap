//! Compiled-runtime-object cache assertions (I#867 follow-up, D-0157).
//!
//! WHY THIS EXISTS. On the default (non-LTO) object-link path the C runtime
//! (`zaprt.c` — a large TU) is compiled to its own object and linked against each
//! program. After the `-O2` runtime default (D-0157) that compile is ~3s (vs
//! ~0.7s at `-O0`), and it does NOT depend on the program, so recompiling it for
//! every one of a golden / `zap test` run's hundreds of builds added tens of
//! minutes. `build_executable` now compiles the runtime to a cached `.o` ONCE per
//! (runtime source, `-O` level, collector, backend gates, CPython, `cc`, triple)
//! and just links it thereafter — the object-link analogue of the LTO bitcode
//! cache. The cache directory is overridable with `$ZAP_RT_CACHE`.
//!
//! These tests drive `zapc build` with an isolated `$ZAP_RT_CACHE` and assert:
//!   1. the runtime object is cached on the first build and REUSED (not
//!      recompiled) on the next build with the same inputs — the cached `.o`'s
//!      mtime is unchanged — and both binaries run with identical output; and
//!   2. a change to a runtime-compile input (here `$ZAP_CC_EXTRA`, which is part
//!      of the key) BUSTS the key: a second, distinct cached object appears.
//!
//! Needs a working `cc` to compile + link the runtime; if none is on PATH the
//! tests SKIP (CI provides one). Collector-agnostic, so gated only on `codegen`.

#![cfg(feature = "codegen")]

use std::process::Command;

/// A tiny program that pulls in the dict runtime (so the runtime object is
/// genuinely linked) and prints a deterministic value.
const PROG: &str = "\
def main():
    dict[int64, int64] d = {}
    d[0] = 7
    print(d[0])
";

/// Whether a C compiler is available (the build links through `cc`/`$ZAP_CC`).
fn has_cc() -> bool {
    let cc = std::env::var("ZAP_CC").unwrap_or_else(|_| "cc".to_string());
    Command::new(cc)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build `PROG` into `dir` with `ZAP_RT_CACHE=cache_dir` plus any `extra_env`.
/// Returns the binary path. Asserts the build succeeded and the binary exists.
fn build(
    dir: &std::path::Path,
    cache_dir: &std::path::Path,
    extra_env: &[(&str, &str)],
) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let src = dir.join("prog.zap");
    std::fs::write(&src, PROG).unwrap();
    let bin = dir.join("prog");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zapc"));
    cmd.arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .env("ZAP_RT_CACHE", cache_dir);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn zapc build");
    assert!(
        out.status.success(),
        "`zapc build` failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(bin.exists(), "expected a binary at {}", bin.display());
    bin
}

/// Run `bin` and return its stdout, asserting it exited 0.
fn run(bin: &std::path::Path) -> String {
    let out = Command::new(bin).output().expect("spawn built binary");
    assert!(
        out.status.success(),
        "built binary {} exited non-zero: {}",
        bin.display(),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The published runtime objects in `cache_dir`: files named `zaprt-<key>.o`,
/// excluding the `zaprt-<key>.<pid>.<nanos>.tmp.o` in-flight temporaries.
fn cached_objects(cache_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(rd) = std::fs::read_dir(cache_dir) else {
        return Vec::new();
    };
    let mut v: Vec<std::path::PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            name.starts_with("zaprt-") && name.ends_with(".o") && !name.contains(".tmp.")
        })
        .collect();
    v.sort();
    v
}

#[test]
fn runtime_object_cached_and_reused_without_recompile() {
    if !has_cc() {
        eprintln!(
            "SKIP runtime_object_cached_and_reused_without_recompile: no `cc` on PATH \
             (the link step needs a C compiler; CI provides one)"
        );
        return;
    }

    let root = std::env::temp_dir().join(format!("zaprtcache-reuse-{}", std::process::id()));
    let cache = root.join("cache");
    std::fs::create_dir_all(&cache).unwrap();

    // Cold build: the runtime object is compiled and published.
    let bin1 = build(&root.join("a"), &cache, &[]);
    let out1 = run(&bin1);

    let objs = cached_objects(&cache);
    assert_eq!(
        objs.len(),
        1,
        "expected exactly one cached runtime object after the cold build, found {}: {:?}",
        objs.len(),
        objs
    );
    let obj = &objs[0];
    let mtime_cold = std::fs::metadata(obj).unwrap().modified().unwrap();

    // Warm build (same program, same inputs, same cache): the runtime object is a
    // cache HIT, so it is NOT recompiled — same file, unchanged mtime — and the
    // produced binary runs identically.
    let bin2 = build(&root.join("b"), &cache, &[]);
    let out2 = run(&bin2);

    let objs_after = cached_objects(&cache);
    let mtime_warm = std::fs::metadata(obj).unwrap().modified().unwrap();

    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(
        objs_after.len(),
        1,
        "the warm build must not add a second cached object (same inputs = same key); found {:?}",
        objs_after
    );
    assert_eq!(
        mtime_cold, mtime_warm,
        "the cached runtime object was rewritten on the warm build — it should have been \
         reused as-is (a cache hit never recompiles)"
    );
    assert_eq!(
        out1, out2,
        "cold and warm builds produced binaries with different output ({out1:?} vs {out2:?})"
    );
    assert!(!out1.trim().is_empty(), "expected non-empty program output");
}

#[test]
fn cache_key_distinguishes_runtime_compile_flags() {
    if !has_cc() {
        eprintln!(
            "SKIP cache_key_distinguishes_runtime_compile_flags: no `cc` on PATH \
             (the link step needs a C compiler; CI provides one)"
        );
        return;
    }

    let root = std::env::temp_dir().join(format!("zaprtcache-key-{}", std::process::id()));
    let cache = root.join("cache");
    std::fs::create_dir_all(&cache).unwrap();

    // Default compile flags: one cached object.
    let bin_plain = build(&root.join("plain"), &cache, &[]);
    let out_plain = run(&bin_plain);
    assert_eq!(
        cached_objects(&cache).len(),
        1,
        "expected one cached object after the default build"
    );

    // A changed runtime-compile input (`$ZAP_CC_EXTRA`, part of the cache key)
    // must miss and publish a DISTINCT object rather than reuse the first.
    let bin_g = build(&root.join("g"), &cache, &[("ZAP_CC_EXTRA", "-g")]);
    let out_g = run(&bin_g);

    let objs = cached_objects(&cache);
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        objs.len() >= 2,
        "changing `$ZAP_CC_EXTRA` (a runtime-compile input in the cache key) must bust the key \
         and produce a second distinct cached object, but found {}: {:?}",
        objs.len(),
        objs
    );
    // Both builds are behaviourally identical (the flag change is compile-only).
    assert_eq!(
        out_plain, out_g,
        "the `-g` build changed observable output ({out_plain:?} vs {out_g:?}) — it must not"
    );
}
