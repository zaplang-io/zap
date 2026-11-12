//! Safepoint-poll flag coalesce — asm-level guard (I#860, D-0155).
//!
//! WHY THIS EXISTS. The inlined back-edge safepoint poll (#568, D-0144) used to
//! load THREE separate `volatile int` globals every loop iteration
//! (`zap_gc_wanted`, `zap_collecting`, `zap_preempt_any`), OR them, and branch to
//! the out-of-line `zap_safepoint` slow path when any was set. On `loop_sum`'s
//! 30M-iteration tight loop that is 90M loads of globals that are almost always
//! zero — and once #857 (D-0152) register-promotes the loop locals, those three
//! poll loads are the visible remaining per-iteration cost. #860 coalesces the
//! three flags into three bit lanes of ONE `volatile uint32_t zap_safepoint_flags`
//! so the poll is a SINGLE `load volatile i32` + `icmp ne 0` + branch.
//!
//! The deterministic machine-independent guard that each poll is exactly ONE
//! volatile load (down from three) is the codegen IR test
//! (`safepoint_poll_is_a_single_coalesced_load` / `gc_free_loop_keeps_backedge_safepoint`
//! in crates/codegen/src/lib.rs); THIS test confirms the coalesce survives to
//! machine code on the shipping build:
//!   1. the final binary's symbol table (`nm`) contains NONE of the three old
//!      flag symbols and DOES define the single coalesced `zap_safepoint_flags`
//!      word — a surviving old symbol would mean codegen or a runtime read site
//!      still used a pre-#860 global, a silent GC/preemption-correctness hazard —
//!      and
//!   2. the hot loop still calls `zap_safepoint` on the cold slow path (the poll
//!      is not dropped). The symbol checks go through `nm` rather than objdump
//!      annotations because reaching a global is encoded differently per arch
//!      (x86-64 RIP-relative `lea`/`mov` vs aarch64 `adrp`/GOT, often lifted out
//!      of the loop body), but symbol-table presence/absence is arch-independent.
//!
//! Needs the precise collector (the inline poll only exists under `ZAP_GC_IMMIX`;
//! the Boehm build preempts via a direct signal switch and emits no poll) plus
//! `objdump` and `nm`, so it is gated like `gc.rs`/`lto.rs`: if either tool is
//! absent (a dev host without binutils) the test SKIPS rather than failing — CI
//! provides them.

#![cfg(all(feature = "codegen", feature = "gc-immix"))]

use std::process::Command;

/// A GC-free, call-free tight loop in a NAMED function (`spin`), so the loop — and
/// its back-edge safepoint poll — lands in `spin`'s own disassembly region rather
/// than being inlined anonymously into `main`. User functions are external in the
/// AOT path, so `spin` survives the `-O2` pipeline standalone. This is the
/// `loop_sum` / `perf.rs` `tight_loop` shape the issue disassembled.
const HOT_LOOP_SRC: &str = "\
def spin(int64 n) -> int64:
    int64 s = 0
    int64 i = 0
    while i < n:
        s = s + i
        i = i + 1
    return s

def main():
    print(spin(1000))
";

/// Whether `objdump` is available for disassembly. If not, skip (as `lto.rs` does).
fn has_objdump() -> bool {
    Command::new("objdump")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build `HOT_LOOP_SRC` to a native binary (default flags — the coalesce shows in
/// an ordinary build, no LTO needed). Returns the binary path inside `dir`.
fn build_binary(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let src = dir.join("hot_loop.zap");
    std::fs::write(&src, HOT_LOOP_SRC).unwrap();
    let bin = dir.join("hot_loop");
    let out = Command::new(env!("CARGO_BIN_EXE_zapc"))
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("spawn zapc build");
    assert!(
        out.status.success(),
        "`zapc build` failed:\nstdout={}\nstderr={}",
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

/// Whether `nm` is available (used for arch-independent symbol presence/absence).
fn has_nm() -> bool {
    Command::new("nm")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The set of symbol NAMES `nm` lists for `bin` (the last whitespace-delimited
/// column of each line — defined or undefined). Used to check a symbol's presence
/// or absence independently of instruction encoding: x86-64 reaches a global with
/// a RIP-relative `lea`/`mov` that objdump annotates inline, but aarch64 uses an
/// `adrp`/GOT pair often lifted to a literal pool outside the function body, so
/// scraping objdump annotations is not portable. A symbol's presence in the
/// table is.
fn nm_symbols(bin: &std::path::Path) -> std::collections::HashSet<String> {
    let out = Command::new("nm").arg(bin).output().expect("spawn nm");
    assert!(
        out.status.success(),
        "nm {} failed: {}",
        bin.display(),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().next_back().map(str::to_string))
        .collect()
}

/// The disassembly lines of the function whose label is `<sym>:`, from the label
/// up to (not including) the next `<...>:` function label. Empty if not found.
fn fn_region<'a>(disasm: &'a str, sym: &str) -> Vec<&'a str> {
    let label = format!("<{sym}>:");
    let mut out = Vec::new();
    let mut inside = false;
    for line in disasm.lines() {
        if line.contains(&label) {
            inside = true;
            continue;
        }
        if inside {
            // A new function label (`<other>:` at end of a `addr <name>:` line)
            // ends the region.
            if line.trim_end().ends_with(">:") {
                break;
            }
            out.push(line);
        }
    }
    out
}

/// Count references to `sym` in a slice of disassembly lines: the symbol in
/// `<sym>` or `<sym@plt>` form (a call target or a RIP-relative data-load comment
/// `# <addr> <sym>`), never the definition label line and never an internal
/// `<sym+0xNN>` offset. Arch-agnostic (x86-64 `mov`/`callq`, aarch64 `adrp`/`bl`).
fn sym_refs(lines: &[&str], sym: &str) -> usize {
    let exact = format!("<{sym}>");
    let plt = format!("<{sym}@");
    lines
        .iter()
        .filter(|l| !l.trim_end().ends_with(':') && (l.contains(&exact) || l.contains(&plt)))
        .count()
}

#[test]
fn hot_loop_poll_is_one_coalesced_load_not_three() {
    if !has_objdump() || !has_nm() {
        eprintln!(
            "SKIP hot_loop_poll_is_one_coalesced_load_not_three: objdump and nm are \
             required to inspect the final binary (binutils; CI provides it)"
        );
        return;
    }

    let root = std::env::temp_dir().join(format!("zapsp860-{}", std::process::id()));
    let bin = build_binary(&root);

    // Symbol-table guard (arch-independent, via `nm`). The three old flag globals
    // must be GONE — the coalesce removed their definitions and swept every
    // codegen/runtime reference, so neither codegen nor a runtime read site can
    // still touch a pre-#860 global (a missed collection or a missed preempt). The
    // single coalesced word must be present (defined). This holds identically on
    // x86-64 and aarch64, unlike the instruction encoding that reaches the global.
    let syms = nm_symbols(&bin);
    for old in ["zap_gc_wanted", "zap_collecting", "zap_preempt_any"] {
        assert!(
            !syms.contains(old),
            "coalesce #860 must remove the old flag symbol `{old}` from the binary"
        );
    }
    assert!(
        syms.contains("zap_safepoint_flags"),
        "coalesce #860 must define the single `zap_safepoint_flags` word in the binary"
    );

    // Poll-liveness guard (objdump): the hot loop (`spin`) must still reach a
    // safepoint — it CALLS `zap_safepoint` on the cold slow path (one call per
    // poll: entry + back-edge). This is the GC/preemption liveness the coalesce
    // must not drop.
    let dis = disasm(&bin);
    let spin = fn_region(&dis, "spin");
    assert!(
        !spin.is_empty(),
        "could not locate `spin` in the disassembly:\n{dis}"
    );
    let polls = sym_refs(&spin, "zap_safepoint");
    assert!(
        polls >= 1,
        "the hot loop must keep its safepoint poll(s) (GC/preemption liveness); \
         found {polls} `zap_safepoint` calls in spin:\n{}",
        spin.join("\n"),
    );

    // Diagnostic (always printed): the per-poll annotated flag-load count in the
    // hot loop. On x86-64 each poll is a RIP-relative load objdump annotates with
    // `<zap_safepoint_flags>`, so this reads ~1 per poll; on aarch64 the `adrp`/GOT
    // address materialization is often lifted out of the loop body (and may carry
    // no inline symbol comment), so it can read 0 here even though the poll loads
    // the word. The deterministic, machine-independent "exactly one volatile load
    // per poll" guard is the codegen IR test (`gc_free_loop_keeps_backedge_safepoint`);
    // this test only confirms, at the binary level, that the coalesce happened
    // (old symbols gone, one word defined) and the loop still polls.
    let flag_loads = sym_refs(&spin, "zap_safepoint_flags");
    eprintln!(
        "#860 asm: spin polls (zap_safepoint calls) = {polls}, \
         annotated flag-load refs in spin (zap_safepoint_flags) = {flag_loads}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
