//! LLVM-IR / ABI / optimizer-pipeline tests: lower Zap -> ZIR -> LLVM and inspect
//! the emitted IR (interning, by-value vs pointer ABI, vectorization, GC frames and
//! safepoints, the O2 pipeline). About codegen, not the C runtime text. Split out of
//! the `lib.rs` crate root (#1025).

use super::*;
use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

/// Translate a source program straight to LLVM IR text (no object/link).
fn llvm_ir(src: &str) -> String {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (zir, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    let ctx = Context::create();
    let llmod = ctx.create_module("test");
    let builder = ctx.create_builder();
    Target::initialize_native(&InitializationConfig::default()).expect("init native");
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).expect("target");
    let tm = target
        .create_target_machine(
            &triple,
            "generic",
            "",
            OptimizationLevel::Default,
            RelocMode::PIC,
            CodeModel::Default,
        )
        .expect("target machine");
    let td = tm.get_target_data();
    llmod.set_data_layout(&td.get_data_layout());
    let _ = &td; // data layout already set on the module; translate derives its own
    let mut cg = Cg::new(&ctx, &llmod, &builder, false, zir::FpPolicy::default());
    cg.translate(&zir).expect("translate");
    llmod
        .verify()
        .map_err(|e| e.to_string())
        .expect("valid LLVM module");
    llmod.print_to_string().to_string()
}

/// Like [`llvm_ir`], but runs the real middle-end optimization pipeline
/// ([`run_opt_pipeline`], the exact code path `compile_object` takes before
/// object emission) and returns the optimized IR text. Used by the #857
/// register-promotion assertion so the test exercises the shipping pipeline,
/// not a hand-rolled pass list.
#[cfg(feature = "gc-immix")]
fn optimized_llvm_ir(src: &str) -> String {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let ctx = Context::create();
    let builder = ctx.create_builder();
    // Reuse the production module builder (which lowers to ZIR internally) so
    // the IR, data layout and triple match exactly what `compile_object`
    // optimizes.
    let (llmod, tm) = build_llvm_module(
        &ctx,
        &builder,
        &ast,
        "test",
        false,
        false,
        zir::FpPolicy::default(),
    )
    .expect("build module");
    run_opt_pipeline(&llmod, &tm).expect("opt pipeline");
    llmod
        .verify()
        .map_err(|e| e.to_string())
        .expect("valid optimized LLVM module");
    llmod.print_to_string().to_string()
}

// The payoff of ticket #24: element-wise array arithmetic must lower to
// explicit LLVM vector instructions (SIMD), not a scalar loop.
#[test]
fn array_elementwise_vectorizes_f64() {
    let ir = llvm_ir(
        "def main():\n    auto a = array[f64]([1.0, 2.0, 3.0, 4.0, 5.0])\n    auto b = array[f64]([5.0, 4.0, 3.0, 2.0, 1.0])\n    print(sum(a + b))\n",
    );
    // Normalize away the FP-policy fast-math flags (#946): under the default
    // policy codegen stamps `contract` on float ops, so the opcode prints as
    // `fadd contract <4 x double>`. Strip the flag tokens so this structural
    // (vectorization-happened) assertion is policy-independent.
    let v = ir.replace(" contract", "").replace(" reassoc", "");
    assert!(
        v.contains("fadd <4 x double>"),
        "expected a vectorized fadd in IR:\n{ir}"
    );
}

// #896: the BROADCAST kernel (unequal but compatible shapes) must vectorize
// its inner axis too, not only the equal-shape flat path. Both kernels are
// always emitted (the shape test picks one at run time), so a vectorized
// broadcast path shows up as MORE than the single flat vector `fadd` — the
// four inner-axis specializations each contribute one. Before #896 the
// broadcast body was a scalar loop, so there was exactly one.
#[test]
fn array_broadcast_vectorizes_f64() {
    let ir = llvm_ir(
        "def main():\n    auto a = array[f64]([[1.0, 2.0, 3.0, 4.0, 5.0], [6.0, 7.0, 8.0, 9.0, 10.0]])\n    auto b = array[f64]([5.0, 4.0, 3.0, 2.0, 1.0])\n    print(sum(a + b))\n",
    );
    // Strip the FP-policy fast-math flags (#946) before counting — see
    // `array_elementwise_vectorizes_f64`.
    let n = ir
        .replace(" contract", "")
        .replace(" reassoc", "")
        .matches("fadd <4 x double>")
        .count();
    assert!(
        n >= 2,
        "expected the broadcast path to also vectorize (saw {n} vector fadds, \
         want the flat one plus the broadcast specializations):\n{ir}"
    );
}

#[test]
fn array_elementwise_vectorizes_i64() {
    let ir = llvm_ir(
        "def main():\n    auto a = array[i64]([1, 2, 3, 4, 5, 6, 7, 8])\n    auto b = array[i64]([8, 7, 6, 5, 4, 3, 2, 1])\n    print(sum(a * b))\n",
    );
    assert!(
        ir.contains("mul <4 x i64>"),
        "expected a vectorized mul in IR:\n{ir}"
    );
}

// #837 / D-0145: a string literal lowers to ONE interned constant-global
// `ZapStr` reused at every use, NOT a fresh `zap_str_new` heap allocation per
// use. Structural (IR-text) guard — machine-independent, unlike a timing floor.
#[test]
fn string_literal_is_interned_constant() {
    let ir = llvm_ir(
        "def main():\n    auto a = \"hello\"\n    auto b = \"hello\"\n    print(a)\n    print(b)\n",
    );
    // The literal's bytes live in a constant byte array...
    assert!(
        ir.contains("c\"hello\""),
        "string literal should be a constant byte array:\n{ir}"
    );
    // ...deduplicated to a single constant across both identical uses...
    assert_eq!(
        ir.matches("c\"hello\"").count(),
        1,
        "two uses of the same literal must share ONE interned constant:\n{ir}"
    );
    // ...and never re-allocated per use (the old per-use `zap_str_new` call,
    // whose result was named `%strnew`, is gone).
    assert!(
        !ir.contains("strnew"),
        "an interned literal must not be re-allocated via zap_str_new:\n{ir}"
    );
}

// D-0051 physical layout: a flat `value struct` is passed and returned *by
// value* as an LLVM aggregate (register/sret ABI), not as a heap pointer.
#[test]
fn inline_value_struct_uses_by_value_abi() {
    let ir = llvm_ir(
        "value struct Vec2:\n    f64 x\n    f64 y\n\
         def scale(Vec2 v) -> Vec2:\n    return v\n\
         def main():\n    auto a = Vec2(x=1.0, y=2.0)\n    auto b = scale(a)\n    print(b.x)\n",
    );
    // The by-value aggregate signature (small struct → registers).
    assert!(
        ir.contains("define { double, double } @scale({ double, double }"),
        "a flat value struct should pass/return by value:\n{ir}"
    );
}

// A reference struct (D-0023) keeps the heap-pointer ABI — it has shared
// identity, so it is never laid out inline.
#[test]
fn reference_struct_keeps_pointer_abi() {
    let ir = llvm_ir(
        "struct Pt:\n    f64 x\n    f64 y\n\
         def id(Pt p) -> Pt:\n    return p\n\
         def main():\n    auto a = Pt(x=1.0, y=2.0)\n    print(id(a).x)\n",
    );
    assert!(
        ir.contains("define ptr @id(ptr"),
        "a reference struct must pass/return as a pointer:\n{ir}"
    );
}

// A tuple shares the inline layout path (D-0051): a multi-return `(i64, i64)`
// comes back by value as an aggregate.
#[test]
fn tuple_return_uses_by_value_abi() {
    let ir = llvm_ir(
        "def divmod(int64 a, int64 b) -> (int64, int64):\n    return a // b, a % b\n\
         def main():\n    auto (q, r) = divmod(17, 5)\n    print(q)\n",
    );
    assert!(
        ir.contains("define { i64, i64 } @divmod("),
        "a flat tuple should be returned by value:\n{ir}"
    );
}

// The key-function ABI guard: a value struct used as a dict/set key has its
// `__eq__`/`__hash__` address taken and called by the runtime through a
// pointer ABI, so those methods must keep pointer parameters even though the
// struct is otherwise inline (D-0051 / D-0047).
#[test]
fn key_functions_keep_pointer_abi() {
    let ir = llvm_ir(
        "@derive(Eq, Hash)\nvalue struct Point:\n    int64 x\n    int64 y\n\
         def main():\n    auto d = {Point(x=1, y=2): 10}\n    print(len(d))\n",
    );
    // __eq__ is called as i1(ptr, ptr) and __hash__ as i64(ptr) by the
    // collection runtime, so both take pointers, not by-value aggregates.
    assert!(
        ir.contains("@Point____eq__(ptr") && ir.contains("@Point____hash__(ptr"),
        "key functions must keep the pointer ABI:\n{ir}"
    );
}

#[test]
fn array_scalar_broadcast_vectorizes() {
    let ir = llvm_ir(
        "def main():\n    auto a = array[f64]([1.0, 2.0, 3.0, 4.0, 5.0])\n    print(sum(a * 2.0))\n",
    );
    // The scalar is broadcast across lanes (a constant splat folds to a
    // constant vector) and the multiply is still vectorized.
    // Strip the FP-policy fast-math flags (#946) — see
    // `array_elementwise_vectorizes_f64`.
    let v = ir.replace(" contract", "").replace(" reassoc", "");
    assert!(
        v.contains("fmul <4 x double>"),
        "expected a vectorized fmul for scalar broadcast:\n{ir}"
    );
}

// ---- #568 (D-0144): GC-frame skip + inlined safepoint poll -------------
//
// These assert the *shape* of the emitted IR (deterministic, machine- and
// timing-independent — the issue asks to prefer an IR assertion over a
// wall-clock one). The wall-clock trend lives in the perf-bench harness
// (`crates/driver/tests/perf.rs`); the runtime safety/liveness lives in the
// GC-stress + preemption tests (`crates/driver/tests/gc.rs`).

/// Slice one function's `define … { … }` out of a whole-module IR dump so a
/// count/contains assertion is scoped to that function and not its callers.
#[cfg(feature = "gc-immix")]
fn one_fn_ir(ir: &str, name: &str) -> String {
    let needle = format!("@{name}(");
    let at = ir
        .find(&needle)
        .unwrap_or_else(|| panic!("fn `{name}` not found in IR:\n{ir}"));
    let start = ir[..at].rfind("\ndefine ").map(|i| i + 1).unwrap_or(0);
    let end = ir[start..]
        .find("\n}")
        .map(|i| start + i + 2)
        .unwrap_or(ir.len());
    ir[start..end].to_string()
}

// A provably GC-free integer function holds no managed references on any
// path, so #568 skips its shadow-stack frame entirely: no `gcframe` alloca
// and no `zap_shadow_head_addr` push/pop. `fib` has three basic blocks but no
// loop, so it emits exactly ONE safepoint poll (function entry) — the old
// FORK-4a emission put a non-inlinable `call zap_safepoint` at the top of
// every block (the "3× safepoint per call" #568 measured). The poll is now an
// inlined flag check that loads `@zap_safepoint_flags` and branches to the cold
// slow path, so the hot path carries no call and no thread-local read.
#[cfg(feature = "gc-immix")]
#[test]
fn gc_free_fn_skips_frame_and_inlines_safepoint() {
    let ir = llvm_ir(
        "def fib(int64 n) -> int64:\n    if n < 2:\n        return n\n    \
         return fib(n - 1) + fib(n - 2)\n\
         def main():\n    print(fib(10))\n",
    );
    let fib = one_fn_ir(&ir, "fib");
    assert!(
        !fib.contains("gcframe") && !fib.contains("zap_shadow_head_addr"),
        "a GC-free function must emit no shadow frame:\n{fib}"
    );
    assert!(
        fib.contains("@zap_safepoint_flags"),
        "the safepoint must be an inlined flag check:\n{fib}"
    );
    let polls = fib.matches("call void @zap_safepoint()").count();
    assert_eq!(
        polls, 1,
        "a loopless GC-free function emits exactly one (entry) safepoint, not one per block; got {polls}:\n{fib}"
    );
}

// A GC-free loop keeps its back-edge safepoint (STW + preemption liveness for
// a tight, call-free loop — see `gc.rs`): entry poll + loop-header poll, and
// still no frame. This is the safepoint #568 must NOT drop.
#[cfg(feature = "gc-immix")]
#[test]
fn gc_free_loop_keeps_backedge_safepoint() {
    let ir = llvm_ir(
        "def spin(int64 n) -> int64:\n    int64 i = 0\n    while i < n:\n        i = i + 1\n    \
         return i\n\
         def main():\n    print(spin(5))\n",
    );
    let spin = one_fn_ir(&ir, "spin");
    assert!(
        !spin.contains("zap_shadow_head_addr"),
        "a GC-free loop needs no shadow frame:\n{spin}"
    );
    assert!(
        spin.contains("@zap_safepoint_flags"),
        "the loop poll must be the inlined flag check:\n{spin}"
    );
    let polls = spin.matches("call void @zap_safepoint()").count();
    assert!(
        polls >= 2,
        "a loop must keep an entry poll and a back-edge (loop-header) poll; got {polls}:\n{spin}"
    );
    // #860 (D-0155): each poll is ONE volatile load of the coalesced word, so
    // the volatile-flag-load count matches the poll count exactly (pre-#860 it
    // was 3× the poll count — three separate flag globals per poll).
    let flag_loads = spin.matches("load volatile i32").count();
    assert_eq!(
        flag_loads, polls,
        "each safepoint poll must be exactly ONE coalesced volatile load \
         (#860); got {flag_loads} loads for {polls} polls:\n{spin}"
    );
}

// #860 (D-0155): the three per-iteration poll flags (`zap_gc_wanted`,
// `zap_collecting`, `zap_preempt_any`) are coalesced into ONE `volatile
// uint32_t zap_safepoint_flags` with three bit lanes, so a safepoint poll is a
// SINGLE `load volatile i32` + `icmp ne 0` + branch — not three loads OR'd
// together. WRITTEN FIRST: it encodes the #860 intent (one load, old symbols
// gone). `fib` is GC-free and loopless, so it emits exactly one (entry) poll
// and therefore exactly one volatile flag load.
#[cfg(feature = "gc-immix")]
#[test]
fn safepoint_poll_is_a_single_coalesced_load() {
    let fib = one_fn_ir(
        &llvm_ir(
            "def fib(int64 n) -> int64:\n    if n < 2:\n        return n\n    \
             return fib(n - 1) + fib(n - 2)\n\
             def main():\n    print(fib(10))\n",
        ),
        "fib",
    );
    // The poll reads the coalesced word...
    assert!(
        fib.contains("@zap_safepoint_flags"),
        "the safepoint poll must read the coalesced flag word:\n{fib}"
    );
    // ...with exactly ONE volatile load (one poll => one load; #860 removed the
    // other two). The only volatile loads compiled code emits are poll loads.
    let loads = fib.matches("load volatile i32").count();
    assert_eq!(
        loads, 1,
        "a single-poll (loopless, GC-free) function must emit exactly ONE \
         volatile flag load after coalescing (#860), not three; got {loads}:\n{fib}"
    );
    // The three old separate flag symbols must be gone entirely — a lingering
    // reference would mean a poll (or a missed read site) still used an old
    // global, a silent correctness hazard the #860 sweep must not leave.
    for old in ["@zap_gc_wanted", "@zap_collecting", "@zap_preempt_any"] {
        assert!(
            !fib.contains(old),
            "coalesce #860 must remove the old separate flag `{old}`:\n{fib}"
        );
    }
}

// Regression guard for the other direction: a function that holds a managed
// reference (a `list`) MUST still emit its shadow frame — roots stay tracked.
// The skip is conservative; it never drops a frame a collection could need.
#[cfg(feature = "gc-immix")]
#[test]
fn gc_using_fn_keeps_frame() {
    let ir = llvm_ir(
        "def build(int64 n) -> int64:\n    list[int64] xs = []\n    xs.append(n)\n    \
         return len(xs)\n\
         def main():\n    print(build(3))\n",
    );
    let build = one_fn_ir(&ir, "build");
    assert!(
        build.contains("zap_shadow_head_addr"),
        "a function holding a GC reference must still link a shadow frame:\n{build}"
    );
    // And the poll is still the inlined flag check, not the old bare call.
    assert!(
        build.contains("@zap_safepoint_flags"),
        "a GC-using function still uses the inlined safepoint poll:\n{build}"
    );
}

// ---- #857 (D-0152): middle-end opt pipeline promotes loop locals ---------
//
// These guard the ONE structural fix of #857: `compile_object` now runs the
// `default<O2>` middle-end pipeline before the backend emits the object, so
// loop/temporary locals are register-promoted instead of round-tripping to
// their `alloca` stack slot every iteration (the ~6×-slower-than-Go defect
// the issue disassembled). IR-text assertions — deterministic and machine-
// independent, per the GC-golden determinism rule; the wall-clock trend lives
// in `crates/driver/tests/perf.rs` (`tight_loop`/`fib`).

/// A GC-free tight loop (`loop_sum`/`fib` shape — the issue's exact evidence).
#[cfg(feature = "gc-immix")]
const HOT_LOOP_SRC: &str = "def spin(int64 n) -> int64:\n    int64 i = 0\n    int64 s = 0\n    \
     while i < n:\n        s = s + i\n        i = i + 1\n    \
     return s\n\
     def main():\n    print(spin(30000000))\n";

// WRITTEN FIRST (reproduces the defect): the RAW translation — what the
// backend used to receive straight from `write_to_file` — keeps `i` and `s`
// in `alloca` stack slots with a per-iteration load + store. This is the
// `0x8(%rsp)`/`0x10(%rsp)` round-trip the issue's `objdump` showed.
#[cfg(feature = "gc-immix")]
#[test]
fn unoptimized_hot_loop_keeps_locals_on_the_stack() {
    let spin = one_fn_ir(&llvm_ir(HOT_LOOP_SRC), "spin");
    assert!(
        spin.contains("alloca i64"),
        "pre-opt, the loop counter/accumulator must live in alloca stack slots \
         (the #857 defect this fix removes):\n{spin}"
    );
    // No SSA phi before the pipeline runs — the value flows through memory.
    assert!(
        !spin.contains("phi i64"),
        "pre-opt IR should carry no register phi for the loop locals:\n{spin}"
    );
}

// THE FIX: after the `default<O2>` pipeline, `spin`'s loop locals are
// promoted to SSA registers (phi nodes) and the hot function keeps nothing in
// an `alloca` — the loop no longer touches the stack per iteration. The loop
// ITSELF survives because its back-edge safepoint poll is a volatile access
// (gc-immix): the test is about promotion, not loop deletion. (User functions
// are external in the AOT path, so `spin` survives the pipeline standalone
// even if also inlined into `main`.)
#[cfg(feature = "gc-immix")]
#[test]
fn opt_pipeline_promotes_hot_loop_locals_to_registers() {
    let ir = optimized_llvm_ir(HOT_LOOP_SRC);
    let spin = one_fn_ir(&ir, "spin");
    // The back-edge safepoint poll is preserved (volatile loads => the loop is
    // not deleted and still polls every iteration).
    assert!(
        spin.contains("@zap_safepoint_flags"),
        "O2 must preserve the back-edge safepoint poll (GC/preemption liveness):\n{spin}"
    );
    // Register promotion happened: the induction variable is now an SSA phi...
    assert!(
        spin.contains("phi i64"),
        "the loop induction/accumulator must be promoted to an SSA phi by O2:\n{spin}"
    );
    // ...and the GC-free hot function keeps NO scalar stack slot — no
    // per-iteration reload/store of `i`/`s` (the register-only hot loop the
    // issue's acceptance criterion asks for).
    assert!(
        !spin.contains("alloca"),
        "a GC-free hot loop must keep its locals in registers, not allocas, after O2:\n{spin}"
    );
}

// Regression guard: optimization must NOT weaken the GC frame of a function
// that holds a managed reference. The shadow-frame alloca escapes via
// `*zap_shadow_head = &frame`, so O2 cannot promote it away — the root stores
// survive and a collection still finds every root (D-0144 under O2).
#[cfg(feature = "gc-immix")]
#[test]
fn opt_pipeline_preserves_gc_frame_root_stores() {
    let ir = optimized_llvm_ir(
        "def build(int64 n) -> int64:\n    list[int64] xs = []\n    xs.append(n)\n    \
         xs.append(n + 1)\n    return len(xs)\n\
         def main():\n    print(build(3))\n",
    );
    let build = one_fn_ir(&ir, "build");
    // The shadow frame is still linked (the head push/pop helper is called)
    // and still stored to — the escaping alloca was not optimized away.
    assert!(
        build.contains("zap_shadow_head_addr"),
        "O2 must keep the shadow frame of a GC-using function (roots stay tracked):\n{build}"
    );
    assert!(
        build.contains("store ptr"),
        "the escaping shadow-frame/root stores must survive O2:\n{build}"
    );
}
