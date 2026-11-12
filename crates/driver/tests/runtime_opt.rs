//! Default-runtime optimization-level assertions (I#867, D-0157).
//!
//! WHY THIS EXISTS. On the default (non-LTO) object-link path the C runtime
//! (`zaprt.c` — the whole dict/set/string/GC machinery) is compiled as its own
//! translation unit and linked against the program object. For most of the
//! project's life that `cc` invocation carried NO `-O` flag, so the system
//! compiler built the entire runtime at `-O0` while the Zap-generated program
//! object was already optimized (`default<O2>`, #857/D-0152). Every hot generated
//! op therefore called into an UNOPTIMIZED runtime: the typed accessors
//! (`zap_dict_get_i64_i64`, …) stayed stack-spilled with ~5 out-of-line helper
//! calls per lookup, versus ~1 call fully register-allocated at `-O2`. Measured
//! end-to-end on `dict_int` (200k int-keyed inserts+lookups), pinned best-of-25
//! median: `-O0` runtime = 26.9ms; `-O2` runtime = 19.0ms (which then beats Go's
//! 22.5ms). So D-0157 makes the runtime compile at `-O2` by DEFAULT, and adds
//! `zap build --debug` to restore `-O0` for fast iteration (a build is ~0.73s vs
//! ~3s — the runtime TU is large), mirroring `cargo`'s debug/release split. This
//! is a plain optimization level, NOT LTO: the opt-in `ZAP_LTO` cross-module path
//! (D-0149/D-0150) is unchanged, and `-flto` buys ≈ nothing over `-O2` here for
//! ~5× the build.
//!
//! This is the DETERMINISTIC guard for that lever (the wall-clock trend is
//! `perf.rs`): it builds a hot `dict[int64, int64]` read/update loop in the
//! DEFAULT mode and under `--debug`, disassembles the runtime's
//! `zap_dict_get_i64_i64` accessor in each binary, and asserts that
//!   1. the DEFAULT build inlines the `static inline` table accessors
//!      (`zap_d_keys`/`zap_d_vals`/`zap_d_tags`/`zap_tag`) into the accessor — NO
//!      out-of-line call to any of them survives (the `-O2` shape), while
//!   2. the `--debug` build does NOT inline them — at least one out-of-line call
//!      to those helpers remains (the `-O0` shape), and the accessor body carries
//!      strictly MORE call instructions than the optimized one, and
//!   3. BOTH builds still call `zap_key_hash` out of line — it carries
//!      `__attribute__((noinline))` and must stay a call at every level (it also
//!      anchors that we isolated the right function).
//!
//! Needs a working default `cc` (to compile+link the runtime) and `objdump` (to
//! inspect the result). `cc` is always present where codegen builds; if `objdump`
//! is absent (a dev host without binutils) the test SKIPS rather than failing —
//! CI installs binutils, so it is exercised there. It is collector-agnostic (the
//! dict accessor and the `noinline` hash helper exist under both the precise
//! Immix and the conservative Boehm builds), so it is gated only on `codegen`.

#![cfg(feature = "codegen")]

use std::process::Command;

/// A hot `dict[int64, int64]` read/update loop. The `d[0]` reads lower to the
/// typed runtime accessor `zap_dict_get_i64_i64` (the inner `zap_dict_get` is
/// `always_inline`, so it folds into that symbol at every level); the
/// `static inline` table accessors it uses inline only once the runtime itself
/// is optimized — exactly the difference `--debug` toggles.
const HOT_DICT_SRC: &str = "\
def hot(int64 n) -> int64:
    dict[int64, int64] d = {}
    d[0] = 0
    int64 i = 0
    while i < n:
        d[0] = d[0] + 1
        i = i + 1
    return d[0]

def main():
    print(hot(1000))
";

/// Whether `objdump` is available for disassembly. If not, skip.
fn has_objdump() -> bool {
    Command::new("objdump")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build `HOT_DICT_SRC` to a native binary. With `debug`, pass `--debug` so the
/// runtime is compiled at `-O0`; otherwise the default `-O2` runtime is built.
/// Returns the binary path (inside `dir`, which the caller removes).
fn build_binary(dir: &std::path::Path, debug: bool) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let src = dir.join("hot_dict.zap");
    std::fs::write(&src, HOT_DICT_SRC).unwrap();
    let bin = dir.join("hot_dict");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zapc"));
    cmd.arg("build").arg(&src).arg("-o").arg(&bin);
    if debug {
        cmd.arg("--debug");
    }
    let out = cmd.output().expect("spawn zapc build");
    assert!(
        out.status.success(),
        "`zapc build`{} failed:\nstdout={}\nstderr={}",
        if debug { " --debug" } else { "" },
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(bin.exists(), "expected a binary at {}", bin.display());
    bin
}

/// Disassemble `bin` with `objdump -d` and return its text.
fn disasm(bin: &std::path::Path) -> String {
    let out = Command::new("objdump")
        .arg("-d")
        .arg(bin)
        .output()
        .expect("spawn objdump -d");
    assert!(
        out.status.success(),
        "objdump -d {} failed: {}",
        bin.display(),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Extract the disassembly lines that make up one function's body: everything
/// after the `<sym>:` header line up to (but not including) the next function
/// header. objdump renders a function header as `<addr> <name>:`; instruction
/// lines inside a function never carry a `<name>:` label, so the next header line
/// is the end of this body. Returns `None` if `sym` is not defined in `bin`.
fn fn_body<'a>(disasm: &'a str, sym: &str) -> Option<Vec<&'a str>> {
    let header = format!("<{sym}>:");
    let mut lines = disasm.lines();
    // Advance to the header line for `sym`.
    for l in lines.by_ref() {
        if l.trim_end().ends_with(&header) {
            break;
        }
    }
    let mut body = Vec::new();
    for l in lines {
        // A new function header (`<addr> <name>:`) ends this body.
        let t = l.trim_end();
        if t.ends_with(">:") && t.contains('<') {
            break;
        }
        body.push(l);
    }
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

/// Count out-of-line CALL/branch references to `sym` among `body` lines. A real
/// call/branch targets `<sym>` or `<sym@plt>`; an internal jump is `<sym+0xNN>`
/// (skipped by requiring `>` or `@` immediately after the name). Arch-agnostic
/// across x86-64 (`callq`) and aarch64 (`bl`). Mirrors `lto.rs`'s `call_refs`.
fn body_call_refs(body: &[&str], sym: &str) -> usize {
    let exact = format!("<{sym}>");
    let plt = format!("<{sym}@");
    body.iter()
        .filter(|l| {
            let t = l.trim_end();
            !t.ends_with(':') && (t.contains(&exact) || t.contains(&plt))
        })
        .count()
}

/// Total number of call/branch-with-link instructions in `body` (any target).
/// Matches the `callq`/`call` (x86-64) and `bl` (aarch64) mnemonics as whole
/// words so a symbol name that merely contains "call" is not miscounted.
fn body_call_count(body: &[&str]) -> usize {
    body.iter()
        .filter(|l| {
            l.split_whitespace()
                .any(|tok| matches!(tok, "call" | "callq" | "bl"))
        })
        .count()
}

/// The `static inline` dict-table accessors that are out-of-line calls at `-O0`
/// and inlined away at `-O2`. (`zap_key_hash` is deliberately NOT here — it is
/// `noinline` and stays a call at every level; it is the anchor instead.)
const INLINE_HELPERS: &[&str] = &["zap_d_keys", "zap_d_vals", "zap_d_tags", "zap_tag"];

#[test]
fn default_runtime_is_optimized_and_debug_is_not() {
    // The `-O2` default is gated to x86_64 (I#867, D-0157): on other targets
    // (aarch64) the default is `-O0` too, pending the aarch64 `-O2` fix (I#869),
    // so this test's premise (default inlines, `--debug` does not) holds only on
    // x86_64. `zapc build` targets the host, so the host arch is the build target.
    if !cfg!(target_arch = "x86_64") {
        eprintln!(
            "SKIP default_runtime_is_optimized_and_debug_is_not: the `-O2` runtime \
             default is x86_64-only (I#867/D-0157); non-x86_64 stays `-O0` pending I#869"
        );
        return;
    }
    if !has_objdump() {
        eprintln!(
            "SKIP default_runtime_is_optimized_and_debug_is_not: no objdump on PATH \
             (binutils is required to inspect the runtime accessor; CI provides it)"
        );
        return;
    }

    let root = std::env::temp_dir().join(format!("zaprtopt-{}", std::process::id()));
    let default_bin = build_binary(&root.join("default"), false);
    let debug_bin = build_binary(&root.join("debug"), true);

    let default_dis = disasm(&default_bin);
    let debug_dis = disasm(&debug_bin);

    let default_body = fn_body(&default_dis, "zap_dict_get_i64_i64");
    let debug_body = fn_body(&debug_dis, "zap_dict_get_i64_i64");

    // Collect diagnostics before any assertion so a CI failure is self-explaining.
    let default_body = default_body.unwrap_or_default();
    let debug_body = debug_body.unwrap_or_default();

    let default_helper_calls: usize = INLINE_HELPERS
        .iter()
        .map(|h| body_call_refs(&default_body, h))
        .sum();
    let debug_helper_calls: usize = INLINE_HELPERS
        .iter()
        .map(|h| body_call_refs(&debug_body, h))
        .sum();
    let default_hash_calls = body_call_refs(&default_body, "zap_key_hash");
    let debug_hash_calls = body_call_refs(&debug_body, "zap_key_hash");
    let default_total = body_call_count(&default_body);
    let debug_total = body_call_count(&debug_body);

    eprintln!(
        "runtime-opt diagnostics (zap_dict_get_i64_i64):\n  \
         default: {dl} lines, {dt} calls, {dh} inline-helper calls, {dk} zap_key_hash calls\n  \
         debug  : {bl} lines, {bt} calls, {bh} inline-helper calls, {bk} zap_key_hash calls",
        dl = default_body.len(),
        dt = default_total,
        dh = default_helper_calls,
        dk = default_hash_calls,
        bl = debug_body.len(),
        bt = debug_total,
        bh = debug_helper_calls,
        bk = debug_hash_calls,
    );

    let _ = std::fs::remove_dir_all(&root);

    // The accessor must be defined (and isolated) in both binaries — it is a
    // public runtime symbol on the object-link path, referenced by the program.
    assert!(
        !default_body.is_empty() && !debug_body.is_empty(),
        "expected `zap_dict_get_i64_i64` to be defined in both binaries \
         (default body = {} lines, debug body = {} lines)",
        default_body.len(),
        debug_body.len(),
    );

    // Anchor: `zap_key_hash` is `__attribute__((noinline))`, so it stays an
    // out-of-line call at BOTH levels. If it is missing we isolated the wrong
    // region (or noinline regressed), and the inlining signal below is moot.
    assert!(
        default_hash_calls >= 1 && debug_hash_calls >= 1,
        "expected a call to the `noinline` `zap_key_hash` in both bodies \
         (default = {default_hash_calls}, debug = {debug_hash_calls})",
    );

    // 1. DEFAULT build (`-O2` runtime): the `static inline` table accessors are
    //    inlined into the accessor — NO out-of-line call to any of them.
    assert_eq!(
        default_helper_calls, 0,
        "default `zap build` left {default_helper_calls} out-of-line call(s) to the \
         `static inline` dict-table accessors ({INLINE_HELPERS:?}) in \
         `zap_dict_get_i64_i64` — the runtime was NOT compiled at `-O2` (D-0157 \
         regressed: the default runtime must be optimized)"
    );

    // 2. `--debug` build (`-O0` runtime): those helpers are NOT inlined — at
    //    least one out-of-line call survives, and the body carries strictly more
    //    call instructions than the optimized one.
    assert!(
        debug_helper_calls >= 1,
        "`zap build --debug` inlined the `static inline` dict-table accessors \
         ({INLINE_HELPERS:?}) in `zap_dict_get_i64_i64` — the runtime was NOT \
         compiled at `-O0`, so `--debug` is not taking effect"
    );
    assert!(
        debug_total > default_total,
        "expected the `--debug` (`-O0`) `zap_dict_get_i64_i64` to contain strictly \
         more call instructions than the default (`-O2`) one \
         (debug = {debug_total}, default = {default_total}); the two builds look \
         identically optimized, so `--debug` is not lowering the runtime opt level"
    );
}
