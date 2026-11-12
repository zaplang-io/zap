//! LTO runtime-inlining assertions (I#841, D-0149).
//!
//! WHY THIS EXISTS. The C runtime is normally compiled as its OWN object and
//! linked against the program object, so at the program's optimize step the hot
//! runtime fast paths (`zap_dict_get_str_i64`, …) are opaque external symbols the
//! inliner never sees — every `d[k]` pays an out-of-line call (#837, #568). The
//! `ZAP_LTO` build flag (D-0149) instead emits the program as LLVM bitcode,
//! merges it with the runtime bitcode (`llvm-link`), optimizes the whole with
//! `opt -O2`, and lets LLVM's cross-module inliner pull the runtime fast paths
//! into the generated hot loops. The dict get is split into a small
//! `always_inline` fast path (first-probe identity hit) and a `noinline` slow
//! path precisely so the fast path is small enough to cross that boundary while
//! the bulky probe/resize logic stays out of line (see `zap_dict_get` in
//! `zaprt.c`); `zap_dict_get_str_i64` is the thin typed wrapper the fast path
//! folds into.
//!
//! This test is the DETERMINISTIC guard for that lever (the wall-clock trend is
//! `perf.rs`): it builds a hot `dict[str,int64]` read/update loop with and
//! without `ZAP_LTO`, disassembles the final binaries, and asserts that
//!   1. the baseline binary really CALLS `zap_dict_get_str_i64` out of line
//!      (otherwise the probe proves nothing), and
//!   2. the LTO binary has NO out-of-line call to it (the fast path inlined into
//!      the hot loop; the `noinline` slow path may still be called), while
//!   3. the LTO binary STILL calls `zap_shadow_head_addr` — that helper carries
//!      `__attribute__((noinline))` and must not be folded into a cached
//!      thread-pointer base even under whole-program LTO, or a migrated
//!      coroutine would push/pop the wrong thread's shadow stack (I#223 / #568).
//!
//! Needs the full LLVM toolchain + the precise collector (the shadow-stack
//! helper only exists under `ZAP_GC_IMMIX`; the conservative Boehm build scans
//! the stack and emits no `zap_shadow_head_addr`), so it is gated like `gc.rs`.
//! It additionally needs a matching `clang` (LLVM 18) and `objdump`; if either
//! is absent (a dev host without the LLVM toolchain) the test SKIPS rather than
//! failing — CI installs `clang-18` + binutils, so it is exercised there.

#![cfg(all(feature = "codegen", feature = "gc-immix"))]

use std::process::Command;

/// A hot `dict[str, int64]` read/update loop. The `d["k"]` reads lower to the
/// typed runtime wrapper `zap_dict_get_str_i64` (the inner `zap_dict_get` is
/// `static`), which is exactly the external symbol LTO should inline here. The
/// shadow-frame push/pop around the GC-referencing body calls
/// `zap_shadow_head_addr`, which must survive as a call.
const HOT_DICT_SRC: &str = "\
def hot(int64 n) -> int64:
    dict[str, int64] d = {}
    d[\"k\"] = 0
    int64 i = 0
    while i < n:
        d[\"k\"] = d[\"k\"] + 1
        i = i + 1
    return d[\"k\"]

def main():
    print(hot(1000))
";

/// Resolve an LLVM 18 tool by trying, in order: `$LLVM_SYS_180_PREFIX/bin/<name>`
/// (CI sets that prefix), the version-suffixed `<name>-18`, the plain `<name>`,
/// and the canonical Ubuntu path `/usr/lib/llvm-18/bin/<name>`. Returns the first
/// that answers `--version`, or `None` (skip the test — no LLVM 18 toolchain).
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

/// The clang driver to pass as `ZAP_LTO_CC`, if available. Prefers the
/// version-matched `clang-18`. `None` => skip (no LTO-capable toolchain here).
fn find_clang() -> Option<String> {
    find_llvm_tool("clang")
}

/// The `llvm-link` tool to pass as `ZAP_LLVM_LINK`, if available.
fn find_llvm_link() -> Option<String> {
    find_llvm_tool("llvm-link")
}

/// The `opt` tool to pass as `ZAP_OPT`, if available.
fn find_opt() -> Option<String> {
    find_llvm_tool("opt")
}

/// Disassemble a kept `.bc` with `llvm-dis` and return the function `define`
/// headers + `attributes #N` groups, so a failure can show exactly which
/// attributes each function carries (chiefly `target-features`, which gates
/// cross-function inlining). Empty string if `llvm-dis` is unavailable.
fn dump_ir_attrs(bc: &std::path::Path) -> String {
    let Some(llvm_dis) = find_llvm_tool("llvm-dis") else {
        return String::from("(llvm-dis unavailable)");
    };
    let out = Command::new(&llvm_dis).arg(bc).arg("-o").arg("-").output();
    let ll = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return String::from("(llvm-dis failed)"),
    };
    ll.lines()
        .filter(|l| {
            (l.starts_with("define") && (l.contains("@hot") || l.contains("zap_dict_get_str_i64")))
                || l.starts_with("attributes #")
        })
        .collect::<Vec<_>>()
        .join("\n    ")
}

/// Whether `objdump` is available for disassembly. If not, skip.
fn has_objdump() -> bool {
    Command::new("objdump")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build `HOT_DICT_SRC` to a native binary, optionally under `ZAP_LTO`. Returns
/// the binary path (inside a unique temp dir the caller must remove). When `lto`
/// is `Some((clang, llvm_link))`, builds under the LTO flag with those tools.
fn build_binary(dir: &std::path::Path, lto: Option<(&str, &str, &str)>) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let src = dir.join("hot_dict.zap");
    std::fs::write(&src, HOT_DICT_SRC).unwrap();
    let bin = dir.join("hot_dict");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zapc"));
    cmd.arg("build").arg(&src).arg("-o").arg(&bin);
    if let Some((clang, llvm_link, opt)) = lto {
        cmd.env("ZAP_LTO", "1")
            .env("ZAP_LTO_CC", clang)
            .env("ZAP_LLVM_LINK", llvm_link)
            .env("ZAP_OPT", opt)
            // Keep the merged bitcode so the test can disassemble it and report
            // why the runtime call did/did not inline (I#841 diagnostics).
            .env("ZAP_LTO_KEEP", dir);
    }
    let out = cmd.output().expect("spawn zapc build");
    assert!(
        out.status.success(),
        "`zapc build`{} failed:\nstdout={}\nstderr={}",
        if lto.is_some() { " (ZAP_LTO=1)" } else { "" },
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

/// Count the out-of-line CALL/branch references TO `sym` in objdump output. A
/// real call/branch to the function targets `<sym>` (or `<sym@plt>`); objdump
/// renders the function's DEFINITION as `<sym>:` (trailing colon) and an
/// INTERNAL jump within the function's own body as `<sym+0xNN>` (an offset into
/// it). We must count only the first kind: the retained out-of-line copy of an
/// otherwise-inlined wrapper (kept live by `-rdynamic`) still contains internal
/// `<sym+0xNN>` branches, and counting those would wrongly report the wrapper as
/// "not inlined". So match the symbol followed immediately by `>` or `@`, never
/// `+`, and skip the definition label line. Arch-agnostic across the x86-64
/// (`callq`) and aarch64 (`bl`) legs.
fn call_refs(disasm: &str, sym: &str) -> usize {
    let exact = format!("<{sym}>"); // e.g. `call … <zap_dict_get_str_i64>`
    let plt = format!("<{sym}@"); // e.g. `… <zap_dict_get_str_i64@plt>`
    disasm
        .lines()
        .filter(|l| !l.trim_end().ends_with(':') && (l.contains(&exact) || l.contains(&plt)))
        .count()
}

#[test]
fn lto_inlines_dict_fast_path_but_not_shadow_head() {
    let Some(clang) = find_clang() else {
        eprintln!(
            "SKIP lto_inlines_dict_fast_path_but_not_shadow_head: no clang/clang-18 on PATH \
             (an LTO-capable LLVM 18 driver is required; CI installs clang-18)"
        );
        return;
    };
    let Some(llvm_link) = find_llvm_link() else {
        eprintln!(
            "SKIP lto_inlines_dict_fast_path_but_not_shadow_head: no llvm-link/llvm-link-18 \
             (the LTO module merge needs it; CI provides the LLVM 18 toolchain)"
        );
        return;
    };
    let Some(opt) = find_opt() else {
        eprintln!(
            "SKIP lto_inlines_dict_fast_path_but_not_shadow_head: no opt/opt-18 \
             (the LTO `opt -O2` step needs it; CI provides the LLVM 18 toolchain)"
        );
        return;
    };
    if !has_objdump() {
        eprintln!(
            "SKIP lto_inlines_dict_fast_path_but_not_shadow_head: no objdump on PATH \
             (binutils is required to inspect the final binary; CI provides it)"
        );
        return;
    }

    let root = std::env::temp_dir().join(format!("zaplto-{}", std::process::id()));
    let base_dir = root.join("baseline");
    let lto_dir = root.join("lto");

    // Baseline (object link, no LTO) — the runtime call must be out of line.
    let base_bin = build_binary(&base_dir, None);
    let base_dis = disasm(&base_bin);

    // LTO link.
    let lto_bin = build_binary(&lto_dir, Some((&clang, &llvm_link, &opt)));
    let lto_dis = disasm(&lto_bin);

    let base_dict_calls = call_refs(&base_dis, "zap_dict_get_str_i64");
    let lto_dict_calls = call_refs(&lto_dis, "zap_dict_get_str_i64");
    let lto_shadow_calls = call_refs(&lto_dis, "zap_shadow_head_addr");

    // Diagnostics (always printed, so a CI failure is self-explaining): whether
    // `zap_dict_get_str_i64` is defined (`T`/`t`) or merely referenced (`U`) in
    // the LTO binary, and the disassembly lines that mention it. If it shows as
    // `U` / `@plt`, the runtime definition never made it into the optimized
    // module (a merge problem); if it is defined but still called, the inliner
    // declined (an attribute/opt problem).
    let nm = Command::new("nm")
        .arg(&lto_bin)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let nm_lines: Vec<&str> = nm
        .lines()
        .filter(|l| l.contains("zap_dict_get_str_i64") || l.contains("zap_shadow_head_addr"))
        .collect();
    let lto_ref_lines: Vec<&str> = lto_dis
        .lines()
        .filter(|l| l.contains("zap_dict_get_str_i64"))
        .collect();
    let ir_attrs = dump_ir_attrs(&lto_dir.join("merged.opt.bc"));
    eprintln!(
        "lto diagnostics:\n  base zap_dict_get_str_i64 call refs = {base_dict_calls}\n  \
         lto  zap_dict_get_str_i64 call refs = {lto_dict_calls}\n  \
         lto  zap_shadow_head_addr call refs = {lto_shadow_calls}\n  \
         nm (relevant):\n    {}\n  objdump lines mentioning zap_dict_get_str_i64:\n    {}\n  \
         merged.opt.bc IR (defines + attribute groups):\n    {}",
        nm_lines.join("\n    "),
        lto_ref_lines.join("\n    "),
        ir_attrs,
    );

    let _ = std::fs::remove_dir_all(&root);

    // 1. The probe is valid only if the baseline really calls the fast path out
    //    of line (so there is something for LTO to inline).
    assert!(
        base_dict_calls >= 1,
        "baseline (non-LTO) binary should call `zap_dict_get_str_i64` out of line, \
         but found no call reference — the inlining probe is not exercising the \
         expected lowering"
    );

    // 2. Under LTO the hot `d[k]` read fast path is inlined: NO out-of-line call
    //    to the typed dict-get wrapper survives (#841 acceptance criterion).
    assert_eq!(
        lto_dict_calls, 0,
        "LTO binary still has {lto_dict_calls} out-of-line call(s) to \
         `zap_dict_get_str_i64` — the runtime fast path was NOT inlined across \
         the program↔runtime boundary (baseline had {base_dict_calls})"
    );

    // 3. `zap_shadow_head_addr` keeps `noinline` even under whole-program LTO: it
    //    must stay an out-of-line call so the per-thread shadow-head slot is
    //    resolved on the current thread, never folded into a cached base after a
    //    coroutine migrates (I#223 / #568).
    assert!(
        lto_shadow_calls >= 1,
        "LTO binary has NO call to `zap_shadow_head_addr` — the `noinline` helper \
         was folded away, which breaks coroutine-migration GC correctness \
         (I#223 / #568); LTO must honour its noinline attribute"
    );
}
