//! Zap code generation (Phase 1) — lowers ZIR to native code via LLVM
//! (`inkwell`), behind a small [`Backend`] trait so the backend stays
//! swappable.
//!
//! Pipeline: the AST is lowered to ZIR by `zapc_ir` (host-safe, no LLVM), and
//! this crate translates ZIR -> LLVM IR -> object file, linked with `cc`. See
//! `design/IR.md`. The ZIR -> LLVM step is mechanical; all type-directed
//! decisions already happened during AST -> ZIR lowering.

use std::path::Path;
use std::process::Command;

use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module as LlvmModule;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetData, TargetMachine,
};
use inkwell::types::{
    BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FloatType, IntType, VectorType,
};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValue, BasicValueEnum, FunctionValue, PointerValue, VectorValue,
};
use inkwell::{
    AddressSpace, AtomicOrdering, AtomicRMWBinOp, FloatPredicate, IntPredicate, OptimizationLevel,
};

use zapc_ir::{
    self as zir, lower_module_opts_with_types_located, AtomicOrd, AtomicRmwOp, CastKind, CmpPred,
    Func, Inst, Owner, SourceLoc, Term, Ty, TypeMap,
};
use zapc_syntax::ast::Module as AstModule;

/// Built-in stdlib module registry (ticket #121): per-module runtime-symbol
/// declarations live in `modules/<name>.rs`, grouped by module instead of a
/// single hand-maintained `declare_rt` cluster, so new modules are conflict-free
/// parallel-addable. See [`modules`] for the seam contract.
/// CPU-feature taxonomy — the Rust mirror of `zap_cpu_features.h`, locked to the
/// C header by an agreement test (#943, ISA-acceleration epic #139, D-0192).
/// Consumed by the function-multiversioning cloner/resolvers (#944).
pub mod cpu_features;
mod modules;
mod runtime_manifest;

/// The REPL JIT engine (I#159): executes typed Zap incrementally in one
/// long-lived process via an in-process MCJIT, reusing this crate's ZIR->LLVM
/// translation so REPL code has identical semantics to compiled code.
pub mod jit;

/// A code-generation backend.
pub trait Backend {
    /// Compile a (type-checked) AST module to a native object file. `race`
    /// enables the runtime data-race detector instrumentation (`--race`, #677);
    /// `fp` is the floating-point determinism policy (`--fp`, #946).
    fn compile_object(
        &self,
        module: &AstModule,
        types: &TypeMap,
        obj_path: &Path,
        race: bool,
        fp: zir::FpPolicy,
        loc: Option<SourceLoc>,
    ) -> Result<(), String>;
}

/// The LLVM backend.
pub struct LlvmBackend;

impl Backend for LlvmBackend {
    fn compile_object(
        &self,
        module: &AstModule,
        types: &TypeMap,
        obj_path: &Path,
        race: bool,
        fp: zir::FpPolicy,
        loc: Option<SourceLoc>,
    ) -> Result<(), String> {
        let ctx = Context::create();
        let builder = ctx.create_builder();
        let (llmod, tm) =
            build_llvm_module(&ctx, &builder, module, types, "zap", false, race, fp, loc)?;
        // Run the LLVM middle-end optimization pipeline BEFORE object emission
        // (I#857, D-0152). `write_to_file` is `llc`-equivalent — it runs the
        // backend (isel/regalloc/sched) but NOT the IR transform passes
        // (`mem2reg`/SROA, instcombine, GVN, LICM, …), which live in the
        // middle-end `opt` pipeline. Without them every local stays in its
        // `alloca` stack slot and tight loops round-trip `i`/`s` to memory each
        // iteration (~6× slower than Go). This is the normal (non-LTO) native
        // build path; the LTO path (`compile_bitcode`, D-0149/D-0150) runs
        // `opt -O2` on the merged module instead, so neither path double-opts.
        run_opt_pipeline(&llmod, &tm)?;
        tm.write_to_file(&llmod, FileType::Object, obj_path)
            .map_err(|e| format!("failed to write object file: {}", e.to_string()))?;
        Ok(())
    }
}

/// The new-pass-manager pipeline string for the normal (non-LTO) native build
/// (I#857). `default<O2>` is the full middle-end pipeline at `-O2` — the same
/// pipeline the LTO path already runs on its merged module (D-0149) and the same
/// one the target backend expects to have run before codegen. Overridable with
/// `$ZAP_OPT_LEVEL` (`0`/`1`/`2`/`3`/`s`/`z`) for A/B measurement and bisection;
/// anything unrecognised falls back to `-O2`.
fn opt_pipeline_spec() -> &'static str {
    match std::env::var("ZAP_OPT_LEVEL").as_deref() {
        Ok("0") => "default<O0>",
        Ok("1") => "default<O1>",
        Ok("3") => "default<O3>",
        Ok("s") => "default<Os>",
        Ok("z") => "default<Oz>",
        // "2" and everything else (including unset): the default.
        _ => "default<O2>",
    }
}

/// The `-O` flag the C runtime translation unit (`zaprt.c`) is compiled with on
/// the default (non-LTO) object-link path (I#867, D-0157).
///
/// The runtime is the whole dict / set / string / GC machinery, and every hot
/// generated op calls into it, so an UNOPTIMIZED runtime dominated end-to-end
/// time even after the program module itself was optimized (D-0152): the
/// accessors (`zap_dict_get_i64_i64`, …) stay stack-spilled with ~5 out-of-line
/// helper calls per lookup at `-O0`, versus ~1 call fully register-allocated at
/// `-O2`. So the default is `-O2` (release intuition); `--debug` compiles it at
/// `-O0` for fast iteration (a build is ~0.73s vs ~3s — the runtime TU is large),
/// mirroring `cargo`'s debug/release split. This is a plain optimization level, NOT
/// LTO: the opt-in `ZAP_LTO` cross-module path (D-0149/D-0150) is unchanged, and
/// the perf data shows `-flto` buys ≈ nothing over `-O2` here for ~5× the build.
/// The program module's own opt level is governed separately by D-0152
/// (`ZAP_OPT_LEVEL` / `ZAP_NO_OPT`); `--debug` deliberately touches only the
/// runtime TU, which is the dominant compile-time cost.
///
/// **No target gate (I#869).** The `-O2` default applies on every target. It was
/// briefly gated to `x86_64` (#867/#868) because the aarch64 runtime SIGSEGV-ed
/// at `-O2`; that was root-caused (I#869) to `g_bootstrap` re-reading the
/// thread-local `cur_m` after a coroutine parked — on aarch64 `-O2` caches the
/// thread-local's *address* across the call, so a coroutine resumed on another
/// worker thread read the old thread's `M`. `g_bootstrap` now reads the resuming
/// `M` from its own `G` (set by `switch_to_g`), independent of the compiler's TLS
/// addressing model, so the gate is gone. `--debug` forces `-O0` on every target.
fn runtime_opt_flag(debug: bool) -> &'static str {
    if debug {
        "-O0"
    } else {
        "-O2"
    }
}

/// Run the LLVM middle-end optimization pipeline on `llmod` in place, before the
/// target backend emits the object (I#857, D-0152).
///
/// ## Why this is safe under the precise-Immix moving GC (D-0144)
///
/// The optimizer must not break the invariants the collector relies on; it does
/// not, and the same `-O2` already runs over generated code on the LTO path
/// (D-0149), where the GC / `--race` / preemption goldens are green:
///
/// * **Shadow-stack root stores survive.** The frame alloca is address-taken via
///   `*zap_shadow_head = &frame` (it escapes to an external function), so
///   `mem2reg`/SROA cannot promote it to a register and the per-root stores are
///   not dead — a collection walking the shadow stack still finds every root.
/// * **Safepoint polls are not hoisted, reordered or deleted.** Each poll loads
///   the coalesced `zap_safepoint_flags` word (#860) with a **volatile** load
///   (see [`Cg::gc_safepoint`]); LICM cannot hoist a volatile access out of the
///   loop and DCE cannot drop it, and a volatile memory op also keeps the loop
///   itself from being deleted (so a tight GC-free loop still polls every
///   iteration — the `*preempt*` / `gc_*_stress` goldens stay live). The cold
///   slow path's `zap_safepoint` is an opaque external call, never eliminated.
/// * GC-pointer values stay reloaded from their root cells across each safepoint
///   (D-0144 keeps loads un-CSE'd across the opaque poll call), so a moved object
///   is seen at its new address.
///
/// Register promotion of the *non-escaping scalar* locals (`i`, `s`, temporaries)
/// — which have no GC meaning — is exactly the win: they collapse from stack
/// slots to SSA registers, and the hot loop stops round-tripping to memory.
///
/// `$ZAP_NO_OPT` skips the pipeline entirely (byte-for-byte the pre-#857
/// behaviour) for bisecting a suspected mis-optimization.
fn run_opt_pipeline(llmod: &LlvmModule, tm: &TargetMachine) -> Result<(), String> {
    if std::env::var_os("ZAP_NO_OPT").is_some() {
        return Ok(());
    }
    let passes = opt_pipeline_spec();
    llmod
        .run_passes(passes, tm, PassBuilderOptions::create())
        .map_err(|e| {
            format!(
                "optimization pipeline (`{passes}`) failed: {}",
                e.to_string()
            )
        })?;
    Ok(())
}

impl LlvmBackend {
    /// Emit a (type-checked) AST module as an LLVM **bitcode** file for the LTO
    /// link path (I#841, D-0149). Unlike [`Backend::compile_object`], which runs
    /// the target backend to native code, this writes the program as LLVM IR
    /// bitcode so it can be merged with the runtime bitcode (`llvm-link`) and
    /// optimized together (`clang -O2`) — inlining across the program↔runtime
    /// boundary. The hot runtime fast paths (`zap_dict_get_*`, `zap_str_concat`,
    /// `zap_i64_to_str`, …) are opaque external declarations in the program
    /// module and never inlinable until that merge. Everything stays on LLVM 18
    /// (inkwell's `llvm18-0` here, `clang-18` for the runtime), so the bitcode
    /// and clang agree with no version skew.
    fn compile_bitcode(
        &self,
        module: &AstModule,
        types: &TypeMap,
        bc_path: &Path,
        race: bool,
        fp: zir::FpPolicy,
        loc: Option<SourceLoc>,
    ) -> Result<(), String> {
        let ctx = Context::create();
        let builder = ctx.create_builder();
        let (llmod, _tm) =
            build_llvm_module(&ctx, &builder, module, types, "zap", false, race, fp, loc)?;
        // `write_bitcode_to_path` returns false on an I/O failure (it has no
        // richer error channel); surface the path so a build failure is legible.
        if llmod.write_bitcode_to_path(bc_path) {
            Ok(())
        } else {
            Err(format!(
                "failed to write bitcode file: {}",
                bc_path.display()
            ))
        }
    }
}

/// Strip the per-function `target-cpu` / `target-features` / `tune-cpu` string
/// attributes from every function in the bitcode module at `path`, rewriting it
/// in place (I#841, D-0149).
///
/// WHY. clang stamps these attributes on every C-runtime function (e.g.
/// `target-features="+sse2,…"`), but the inkwell-emitted program module carries
/// none. LLVM's cross-module inliner refuses to inline a callee whose
/// `target-features` are not a subset of the caller's (`areInlineCompatible`),
/// so a runtime wrapper (clang attrs) will NOT inline into generated code
/// (inkwell, no attrs) even after the two modules are merged — the thin
/// `zap_dict_get_str_i64` wrapper stayed a call and the whole LTO win was lost
/// (the symptom: the `tests/lto.rs` probe still saw the out-of-line call).
/// Clearing the attributes on both sides makes the merged module uniformly
/// attribute-free, so it is optimized + codegen'd with the final driver's
/// DEFAULT target for the triple (the x86-64 / aarch64 baseline the runtime was
/// written for — no feature is lost, correctness is unchanged) and the inliner
/// can finally cross the program↔runtime boundary.
/// The internal global that caches a `@multiversion` group's resolved clone
/// pointer (#944, epic #139): null until the first call, then the chosen clone's
/// address. One per public dispatcher name.
fn mv_selected_global(public_name: &str) -> String {
    format!("{public_name}$mv$selected")
}

fn strip_target_attrs_bitcode(path: &Path) -> Result<(), String> {
    let ctx = Context::create();
    let m = LlvmModule::parse_bitcode_from_path(path, &ctx).map_err(|e| {
        format!(
            "failed to parse bitcode {} for attribute strip: {}",
            path.display(),
            e.to_string()
        )
    })?;
    // Every function string attribute that LLVM's `areInlineCompatible` /
    // `hasCompatibleFnAttrs` consults: target features AND the FP/math mode
    // attributes clang stamps on the runtime (-O2) but the inkwell program
    // module does not carry. A mismatch in ANY of them makes the inliner refuse
    // to inline a runtime callee into a generated caller. Clearing all of them
    // on every function leaves the merged module uniform, so `opt -O2` uses the
    // target defaults everywhere and the inliner crosses the boundary.
    const INLINE_BLOCKING_FN_ATTRS: &[&str] = &[
        "target-features",
        "target-cpu",
        "tune-cpu",
        "denormal-fp-math",
        "denormal-fp-math-f32",
        "no-trapping-math",
        "no-infs-fp-math",
        "no-nans-fp-math",
        "no-signed-zeros-fp-math",
        "unsafe-fp-math",
        "approx-func-fp-math",
        "use-soft-float",
    ];
    let mut f = m.get_first_function();
    while let Some(func) = f {
        // EXCEPTION: a multiversioned clone MUST keep its `target-features` — that
        // attribute is the whole point of the clone, and it is exactly what makes
        // it an un-inlinable dispatch barrier (rule #3). Stripping it would
        // silently demote the clone to baseline and (for a clone carrying
        // already-selected wider intrinsics, e.g. an AVX2 runtime kernel) leave
        // the backend unable to legalize them — the green-debug / broken-LTO
        // failure mode (#845). Two markers: `$mv$` is a #944 Zap-function clone;
        // `_mvsimd_` is a hand-written C runtime SIMD clone (#945, e.g.
        // `zap_memchr_mvsimd_avx2`). Every other function is stripped so the
        // I#841 cross-boundary inlining holds.
        // `$mv$` is a #944 Zap-function clone, `_mvsimd_` a hand-written C SIMD
        // clone (#945), and `zap_arch_` an `arch.*` intrinsic shim (#962, D-0199):
        // a `target(...)`-attributed C runtime function (e.g.
        // `zap_arch_x86_crc32c_u8` carries `+sse4.2`). All MUST keep their
        // `target-features`: a crc32 shim stripped to baseline cannot select its
        // `@llvm.x86.sse42.crc32.*` intrinsic under LTO ("Cannot select") — the
        // #845-class failure. Keeping the feature also makes it correctly an
        // inlining barrier into a caller that lacks the feature (no SIGILL).
        let is_mv_clone = func
            .get_name()
            .to_str()
            .map(|n| n.contains("$mv$") || n.contains("_mvsimd_") || n.contains("zap_arch_"))
            .unwrap_or(false);
        // A standalone `@target_feature` function (#963, D-0200) carries an
        // ordinary name, so codegen marks it with the `zap-keep-target-features`
        // sentinel; keep its `target-features` like a clone's, then drop the
        // marker (it is internal and must not leak into the final object).
        let is_target_feature = func
            .get_string_attribute(AttributeLoc::Function, "zap-keep-target-features")
            .is_some();
        if is_target_feature {
            func.remove_string_attribute(AttributeLoc::Function, "zap-keep-target-features");
        }
        if !is_mv_clone && !is_target_feature {
            for key in INLINE_BLOCKING_FN_ATTRS {
                func.remove_string_attribute(AttributeLoc::Function, key);
            }
        }
        f = func.get_next_function();
    }
    if m.write_bitcode_to_path(path) {
        Ok(())
    } else {
        Err(format!(
            "failed to rewrite bitcode {} after attribute strip",
            path.display()
        ))
    }
}

/// Obtain the C runtime as attribute-stripped LLVM bitcode for the LTO link,
/// cached so it is compiled at most once per (runtime source, collector,
/// portable, clang, triple) rather than once per program (I#841 perf).
///
/// The runtime bitcode does not depend on the program being compiled, so the
/// hundreds of programs an LTO `zap test`/golden run builds can all share one
/// compiled+stripped copy. Without this the whole runtime was recompiled at
/// `-O2` and the whole merged module re-stripped for every build, which made
/// the LTO test suites crawl. The cache lives under `$ZAP_LTO_CACHE` (default a
/// stable temp subdir); the key is a hash of everything that determines the
/// bitcode, so a runtime edit or a different clang/collector/triple misses
/// cleanly. Writes are published by atomic rename, so concurrent builds never
/// read a half-written file (a lost race just recompiles — same bytes).
///
/// `rt_path` must already have `zaprt.c` + its `#include`d `zapgc.inc.c` /
/// `zaprace.inc.c` written beside it (the caller does this).
/// A process-global sequence for naming the runtime caches' pre-publish temp
/// files. Combined with the pid it makes the temp name unique across threads in
/// this process (same pid) and across concurrent builds (distinct pids), with
/// **no clock reading** (#917): a microsecond-quantised clock could otherwise
/// hand two threads compiling the same cache `key` the same `zaprt-<key>.<pid>.
/// <nanos>.tmp[.o]` name, so they'd write the same temp and race the atomic
/// rename that publishes it into the shared cache entry. `Relaxed` suffices —
/// we need only that each call observe a distinct value.
fn cache_tmp_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

#[allow(clippy::too_many_arguments)]
fn lto_runtime_bitcode(
    cc: &str,
    rt_path: &Path,
    triple: &str,
    no_cpython: bool,
    no_regex: bool,
    no_zlib: bool,
    no_openssl: bool,
    blas_defines: &[String],
) -> Result<std::path::PathBuf, String> {
    use std::hash::{Hash, Hasher};

    let immix = cfg!(feature = "gc-immix");
    // Only the `--cflags` of CPython embedding (the `-I...`) reach a `-c`
    // compile; compute them once so they feed both the cache key and the
    // compile. CPython is dropped (`-DZAP_NO_CPYTHON`, below) for the portable
    // profile (I#287), on Windows v1 (I#198), AND for any program whose import
    // graph never reaches `import python` (#852) — in all of which the runtime
    // compiles its `zap_py_*` entry points as catchable-error stubs and links no
    // libpython. When dropped we do not even probe `pkg-config`, so a non-Python
    // program builds on a host without `python3-dev`.
    let py_cflags: Vec<String> = if no_cpython {
        Vec::new()
    } else {
        python_embed_flags()?
    };
    let extra = std::env::var("ZAP_CC_EXTRA").unwrap_or_default();

    // Cache key: everything that determines the runtime bitcode.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    runtime_manifest::hash_runtime_fragments(&mut h);
    cc.hash(&mut h);
    triple.hash(&mut h);
    immix.hash(&mut h);
    no_cpython.hash(&mut h);
    no_regex.hash(&mut h);
    no_zlib.hash(&mut h);
    no_openssl.hash(&mut h);
    py_cflags.hash(&mut h);
    blas_defines.hash(&mut h);
    extra.hash(&mut h);
    // Bump when the runtime compile flags below change (e.g. adding
    // -fexceptions, #852 keying the CPython drop on `no_cpython`, or #853 adding
    // the PCRE2/zlib gates) so a stale pre-change cache entry is never reused.
    "rt-bc-flags-v6-blas-seam".hash(&mut h);
    // Different clang builds emit different bitcode — fold the version in.
    if let Ok(out) = Command::new(cc).arg("--version").output() {
        out.stdout.hash(&mut h);
    }
    let key = format!("{:016x}", h.finish());

    let cache_dir = std::env::var_os("ZAP_LTO_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("zap-lto-cache"));
    let _ = std::fs::create_dir_all(&cache_dir);
    let cached = cache_dir.join(format!("zaprt-{key}.bc"));
    if cached.exists() {
        return Ok(cached);
    }

    // Miss: compile the runtime to bitcode, strip inline-blocking attrs, and
    // publish atomically. Compile into a unique temp beside the cache entry.
    let tmp = cache_dir.join(format!(
        "zaprt-{key}.{}.{}.tmp",
        std::process::id(),
        cache_tmp_seq()
    ));
    let mut rc = Command::new(cc);
    rc.arg("-O2")
        // Compile the runtime with C++/Itanium exceptions ENABLED. zaprt.c
        // reaches `__cxa_throw` (zap_raise, zap_scope_exit, the except* group
        // machinery, …), so its functions genuinely participate in unwinding.
        // Without -fexceptions clang blanket-marks every C function `nounwind`;
        // once this bitcode is merged with the program (LTO path only), `opt
        // -O2` trusts that `nounwind`, rewrites the program's `invoke`s to the
        // runtime into plain `call`s and deletes the landing pads — so a thrown
        // Zap exception unwinds past every handler straight to std::terminate
        // (I#841). The non-LTO object link never merges the two modules or
        // re-runs the optimizer, so it is unaffected and stays as-is.
        .arg("-fexceptions")
        .arg("-emit-llvm")
        .arg("-c")
        .arg(rt_path)
        .arg("-o")
        .arg(&tmp);
    if immix {
        rc.arg("-DZAP_GC_IMMIX");
    }
    if no_cpython {
        rc.arg("-DZAP_NO_CPYTHON");
    } else {
        rc.args(&py_cflags);
    }
    // Optional-backend compile gates (#853): stub the regex / zlib entry points
    // in the cached runtime bitcode so a program that never reaches them links
    // no pcre2 / zlib (matching the `-l` omission at the link step).
    if no_regex {
        rc.arg("-DZAP_NO_PCRE2");
    }
    if no_zlib {
        rc.arg("-DZAP_NO_ZLIB");
    }
    if no_openssl {
        rc.arg("-DZAP_NO_OPENSSL");
    }
    // Numeric accelerator (#912): define ZAP_HAVE_BLAS + the provider macro (and
    // any `pkg-config --cflags`) for a program that reaches a `zap_accel_*` seam;
    // empty otherwise, so the seam compiles as its no-provider stubs.
    for d in blas_defines {
        rc.arg(d);
    }
    for a in extra.split_whitespace() {
        rc.arg(a);
    }
    let status = rc
        .status()
        .map_err(|e| format!("failed to run LTO runtime compile (clang -emit-llvm): {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("LTO runtime bitcode compile failed: {status}"));
    }
    // Strip the runtime's target/FP attributes once here so the merged module
    // never needs a per-build strip (see `strip_target_attrs_bitcode`).
    if let Err(e) = strip_target_attrs_bitcode(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // Publish. If another concurrent build won the race, its copy is identical;
    // rename overwrites atomically on the same filesystem either way.
    if let Err(e) = std::fs::rename(&tmp, &cached) {
        let _ = std::fs::remove_file(&tmp);
        // A concurrent build may already have published it — accept that.
        if cached.exists() {
            return Ok(cached);
        }
        return Err(format!(
            "failed to publish runtime bitcode to cache {}: {e}",
            cached.display()
        ));
    }
    Ok(cached)
}

/// Obtain the C runtime as a compiled native object (`.o`) for the default
/// (non-LTO) object-link path, cached so `zaprt.c` is compiled at most once per
/// (runtime source, opt level, collector, backend gates, CPython, `cc`, triple)
/// rather than once per program (I#867 follow-up).
///
/// The runtime object does not depend on the program being compiled, so the
/// hundreds of programs a `zap test` / golden run builds can all share one
/// compiled copy. This matters especially after the `-O2` runtime default
/// (D-0157): without this cache, `golden.rs`'s 442 fixtures each recompiled the
/// whole large runtime TU — ~3s at `-O2` vs ~0.7s at `-O0` — adding tens of
/// minutes across the matrix. The LTO path already caches runtime *bitcode* the
/// same way ([`lto_runtime_bitcode`]); this is the object-link analogue.
///
/// The cache lives under `$ZAP_RT_CACHE` (default a stable temp subdir); the key
/// hashes everything that determines the object, so a runtime edit, a different
/// `-O` level (so the `-O2` default and `--debug` `-O0` never collide), a
/// different collector / backend gate / CPython profile / `cc` / triple misses
/// cleanly. Writes are published by atomic rename, so the parallel golden pool's
/// concurrent builds never read a half-written object (a lost race just
/// recompiles the same bytes). Only the compile-relevant flags enter here; the
/// link-only flags (`-l*`, `-static`, `-rdynamic`, …) stay on the link command.
///
/// `rt_path` must already have `zaprt.c` + its `#include`d `zapgc.inc.c` /
/// `zaprace.inc.c` written beside it (the caller does this). `opt_flag` is the
/// `-O` level from [`runtime_opt_flag`].
#[allow(clippy::too_many_arguments)]
fn native_runtime_object(
    cc: &str,
    rt_path: &Path,
    triple: &str,
    opt_flag: &str,
    no_cpython: bool,
    no_regex: bool,
    no_zlib: bool,
    no_openssl: bool,
    blas_defines: &[String],
) -> Result<std::path::PathBuf, String> {
    use std::hash::{Hash, Hasher};

    let immix = cfg!(feature = "gc-immix");
    // Same CPython-cflags logic as the LTO cache: only the `-I...` `--cflags`
    // affect a `-c` compile, and they are probed only when CPython is actually
    // embedded (never for a portable / Windows / non-`import python` build), so a
    // non-Python program builds on a host without `python3-dev`.
    let py_cflags: Vec<String> = if no_cpython {
        Vec::new()
    } else {
        python_embed_flags()?
    };
    let extra = std::env::var("ZAP_CC_EXTRA").unwrap_or_default();

    // Cache key: everything that determines the runtime object.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    runtime_manifest::hash_runtime_fragments(&mut h);
    cc.hash(&mut h);
    triple.hash(&mut h);
    opt_flag.hash(&mut h);
    immix.hash(&mut h);
    no_cpython.hash(&mut h);
    no_regex.hash(&mut h);
    no_zlib.hash(&mut h);
    no_openssl.hash(&mut h);
    py_cflags.hash(&mut h);
    blas_defines.hash(&mut h);
    extra.hash(&mut h);
    // Bump when the runtime compile flags below change so a stale pre-change
    // cache entry is never reused.
    "rt-obj-flags-v2-blas-seam".hash(&mut h);
    // Different cc builds emit different objects — fold the version in.
    if let Ok(out) = Command::new(cc).arg("--version").output() {
        out.stdout.hash(&mut h);
    }
    let key = format!("{:016x}", h.finish());

    let cache_dir = std::env::var_os("ZAP_RT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("zap-rt-cache"));
    let _ = std::fs::create_dir_all(&cache_dir);
    let cached = cache_dir.join(format!("zaprt-{key}.o"));
    if cached.exists() {
        return Ok(cached);
    }

    // Miss: compile the runtime to an object and publish atomically into a unique
    // temp beside the cache entry. These are EXACTLY the compile-relevant flags
    // the old single-step `cc <prog.o> zaprt.c …` passed (no `-fexceptions`: the
    // non-LTO object link never merges modules or re-runs the optimizer, so the
    // runtime's default `nounwind` is harmless here — see `lto_runtime_bitcode`).
    let tmp = cache_dir.join(format!(
        "zaprt-{key}.{}.{}.tmp.o",
        std::process::id(),
        cache_tmp_seq()
    ));
    let mut rc = Command::new(cc);
    rc.arg(opt_flag).arg("-c").arg(rt_path).arg("-o").arg(&tmp);
    if immix {
        rc.arg("-DZAP_GC_IMMIX");
    }
    if no_cpython {
        rc.arg("-DZAP_NO_CPYTHON");
    } else {
        rc.args(&py_cflags);
    }
    if no_regex {
        rc.arg("-DZAP_NO_PCRE2");
    }
    if no_zlib {
        rc.arg("-DZAP_NO_ZLIB");
    }
    if no_openssl {
        rc.arg("-DZAP_NO_OPENSSL");
    }
    // Numeric accelerator (#912): define ZAP_HAVE_BLAS + the provider macro (and
    // any `pkg-config --cflags`) for a program that reaches a `zap_accel_*` seam;
    // empty otherwise, so the seam compiles as its no-provider stubs.
    for d in blas_defines {
        rc.arg(d);
    }
    for a in extra.split_whitespace() {
        rc.arg(a);
    }
    let status = rc
        .status()
        .map_err(|e| format!("failed to run runtime object compile (cc -c): {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("runtime object compile failed: {status}"));
    }
    // Publish. If another concurrent build won the race, its copy is equivalent;
    // rename overwrites atomically on the same filesystem either way.
    if let Err(e) = std::fs::rename(&tmp, &cached) {
        let _ = std::fs::remove_file(&tmp);
        if cached.exists() {
            return Ok(cached);
        }
        return Err(format!(
            "failed to publish runtime object to cache {}: {e}",
            cached.display()
        ));
    }
    Ok(cached)
}

/// Lower a type-checked AST module to a verified in-memory LLVM module (shared
/// by the object-file backend and the REPL JIT, I#159). Returns the module and
/// the host target machine whose data layout drove the GC type-descriptor field
/// offsets (ticket #48). The `builder` must outlive the returned module's
/// construction; it is unused afterwards. `jit` selects the REPL JIT code path
/// (shadow-head access via a runtime helper; see [`Cg::gc_head`]).
#[allow(clippy::too_many_arguments)] // +`types` for sema-driven inference (#1167)
pub(crate) fn build_llvm_module<'ctx>(
    ctx: &'ctx Context,
    builder: &Builder<'ctx>,
    module: &AstModule,
    types: &TypeMap,
    name: &str,
    jit: bool,
    race: bool,
    fp: zir::FpPolicy,
    loc: Option<SourceLoc>,
) -> Result<(LlvmModule<'ctx>, TargetMachine), String> {
    let (zir_module, errs) = lower_module_opts_with_types_located(module, race, types, loc);
    if !errs.is_empty() {
        return Err(errs
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
            .join("; "));
    }

    let llmod = ctx.create_module(name);

    // Create the target machine before translation so its data layout drives
    // both the emitted code and the GC type-descriptor field offsets
    // (ticket #48 — see Cg::translate).
    Target::initialize_native(&InitializationConfig::default())
        .map_err(|e| format!("failed to initialize native target: {e}"))?;
    let triple = TargetMachine::get_default_triple();
    let target =
        Target::from_triple(&triple).map_err(|e| format!("unknown target: {}", e.to_string()))?;
    let tm = target
        .create_target_machine(
            &triple,
            "generic",
            "",
            OptimizationLevel::Default,
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| "failed to create target machine".to_string())?;
    let td = tm.get_target_data();
    llmod.set_data_layout(&td.get_data_layout());
    llmod.set_triple(&triple);

    let mut cg = Cg::new(ctx, &llmod, builder, jit, fp);
    cg.translate(&zir_module)?;

    llmod
        .verify()
        .map_err(|e| format!("internal error: invalid LLVM module:\n{}", e.to_string()))?;

    Ok((llmod, tm))
}

/// Compile an AST module to a native executable (object + `cc` link).
/// The Zap C runtime, compiled and linked with each program.
const RUNTIME_C: &str = include_str!("zaprt.c");
/// The precise Immix collector, `#include`d by `zaprt.c` under `ZAP_GC_IMMIX`
/// (ticket #48). Written next to the runtime so the relative include resolves.
const RUNTIME_GC_C: &str = include_str!("runtime/gc.inc.c");
/// The runtime data-race detector hooks (`--race`, #677, epic #665, D-0114),
/// `#include`d unconditionally by `zaprt.c`. Written next to the runtime so the
/// relative include resolves. Slice 2 (#678) replaces the slice-1 no-op stubs
/// with the happens-before / vector-clock engine. A `build.rs` declares this and
/// the other embedded runtime files as `rerun-if-changed` inputs so a runtime
/// edit always rebuilds this crate (CI's cache otherwise kept a stale runtime).
const RUNTIME_RACE_C: &str = include_str!("runtime/race.inc.c");
/// The numeric acceleration seam (#912, D-0179), `#include`d unconditionally by
/// `zaprt.c`. Written next to the runtime so the relative include resolves. Its
/// tier/thread knobs are always compiled; the provider thread-pool call is gated
/// behind `ZAP_HAVE_BLAS`, which codegen defines only for a program that links a
/// BLAS provider (see `blas_build_flags`).
const RUNTIME_BLAS_C: &str = include_str!("runtime/accel/blas.inc.c");
/// The NumPy-compatible bit-exact PCG64 `Generator` surface (#930/#931, D-0189),
/// `#include`d unconditionally by `zaprt.c`. Written next to the runtime so the
/// relative include resolves, along with the vendored NumPy ziggurat tables it
/// `#include`s (`ziggurat_constants.h`). Pointer-free, no provider — always
/// compiled into the runtime like the rest of `random`.
const RUNTIME_RANDOM_C: &str = include_str!("runtime/accel/random.inc.c");
const ZIGGURAT_H: &str = include_str!("runtime/accel/ziggurat_constants.h");
/// The `cpu_features` runtime detector (#943, ISA-acceleration epic #139,
/// D-0192) and the shared feature-bit taxonomy header it `#include`s. Both are
/// `#include`d unconditionally by `zaprt.c` and written next to it so the
/// relative include resolves. The detector is malloc-free/reentrant (it may run
/// in an ifunc resolver before libc init, #944) and process-lifetime-constant;
/// the header is the single source of truth for the feature indices, mirrored in
/// Rust by `crate::cpu_features` and locked by its agreement test.
const RUNTIME_CPU_C: &str = include_str!("runtime/accel/cpu.inc.c");
const CPU_FEATURES_H: &str = include_str!("runtime/accel/zap_cpu_features.h");
/// The SIMD byte/substring search kernel (#945, ISA-acceleration epic #139,
/// D-0194), `#include`d by `zaprt.c` after `zapcpu.inc.c` (it dispatches on the
/// detector). Written next to the runtime so the relative include resolves. The
/// first multiversioned kernel: hand-written clones (scalar/SSE2/AVX2/NEON) +
/// pointer-once-init dispatch reading `cpu_features`.
const RUNTIME_MEMCHR_C: &str = include_str!("runtime/accel/memchr.inc.c");
/// The `arch.x86_64` / `arch.aarch64` intrinsic shims (`crc32c_*`, `popcount_*`,
/// `prefetch`) and the portable `hash.crc32c` wrapper (#962, epic #139, D-0199),
/// `#include`d by `zaprt.c` after `zapcpu.inc.c`. Each arch shim carries the
/// `target(...)` attribute its hardware instruction needs; `hash.crc32c` is a
/// bit-identical software CRC32C (Castagnoli).
const RUNTIME_ARCH_C: &str = include_str!("runtime/accel/arch.inc.c");
/// The SIMD sort accelerator dispatch seam (#933, epic #139, D-0195),
/// `#include`d by `zaprt.c` after `zapcpu.inc.c`. Always compiled; its entry
/// points fall through to the own floor unless `-DZAP_HAVE_SIMDSORT` and a
/// per-ISA kernel object are linked (BackendUse::simdsort).
const RUNTIME_SORTACCEL_C: &str = include_str!("runtime/accel/sortaccel.inc.c");
/// The per-ISA SIMD sort kernels (standalone C++ objects, like `zapfft.cpp`):
/// x86 via vendored Intel x86-simd-sort, aarch64 via vendored Google Highway
/// vqsort. Compiled + linked only for a program that sorts (BackendUse::simdsort)
/// on the matching arch. The vendored headers they `#include` are embedded by
/// `build.rs` as `VENDORED_SORT_FILES` and written beside them at compile time.
const RUNTIME_XSORT_CPP: &str = include_str!("zapxsort.cpp");
const RUNTIME_HWYSORT_CPP: &str = include_str!("zaphwysort.cpp");
include!(concat!(env!("OUT_DIR"), "/vendored_sort.rs"));
/// The Windows x86_64 coroutine context switch (ticket #195 / A1): the asm
/// backend for the `zap_ctx_swap`/`zap_ctx_make` seam, a separate translation
/// unit so it links into the Windows program build AND compiles standalone in
/// the on-Windows verification harness (`zap_ctx_win_x64_test.c`). POSIX uses
/// `ucontext` inline in `zaprt.c`, so this is added to the link on Windows only.
const RUNTIME_CTX_WIN_C: &str = include_str!("zap_ctx_win_x64.c");

/// The Windows x86_64 A3 runtime primitives (ticket #197): the preemption thread
/// hijack + trampoline, the last-chance fault diagnostic filter (+ dbghelp
/// backtrace), and the console-control signal mapping. A separate translation
/// unit — like the A1 context switch — so it links into the Windows program build
/// AND compiles standalone in the on-Windows verification harness
/// (`zap_win_a3_test.cc`). POSIX uses `sigaction`/`sigaltstack` inline in
/// `zaprt.c`, so this is added to the link on Windows only.
const RUNTIME_WIN_A3_C: &str = include_str!("zap_win_a3.c");

/// The Windows IOCP netpoller backend (ticket #196 / A2): the AFD-readiness-over-
/// IOCP mechanism that implements the `np_backend_*` seam on Windows (see
/// design/0020). A separate translation unit — like the A1 context switch and A3
/// primitives — so it links into the Windows program build AND compiles standalone
/// in the on-Windows verification harness (`zap_np_iocp_test.c`). POSIX uses epoll
/// inline in `zaprt.c`, so this is added to the link on Windows only. Its header is
/// `#include`d by `zaprt.c`'s Windows arm, so it is written alongside the runtime.
const RUNTIME_NP_IOCP_C: &str = include_str!("zap_np_iocp.c");
const RUNTIME_NP_IOCP_H: &str = include_str!("zap_np_iocp.h");

/// The FFT backend (#929): the C++ PocketFFT wrapper and the vendored header it
/// includes. Written out and compiled to a native object (linked against the C++
/// stdlib already on the link line) only for a program that reaches a `zap_fft_*`
/// seam (`BackendUse::fft`).
const RUNTIME_FFT_CPP: &str = include_str!("zapfft.cpp");
const POCKETFFT_H: &str = include_str!("../../../third_party/pocketfft/pocketfft_hdronly.h");

/// Whether the (already module-linked, D-0081) program reaches `import python`
/// anywhere in its import graph (#852). `modlink` drops Zap `import <name>`
/// statements when merging but KEEPS every `import python` item so sema can bind
/// its aliases (see `modlink.rs`), so a single scan of the merged module's items
/// is the whole-program answer: every source module's `import python` is present
/// here. When this is false the program references no `zap_py_*`/libpython symbol
/// (the CPython C-API calls are lowered only for a bound `import python` alias,
/// see `crates/ir/src/lower.rs`), so libpython can be dropped from the link.
pub(crate) fn program_uses_cpython(module: &AstModule) -> bool {
    use zapc_syntax::ast::{Import, Item};
    module
        .items
        .iter()
        .any(|it| matches!(it, Item::Import(Import::Python { .. })))
}

/// The optional C backends a program's import graph reaches (#853), detected
/// the same way as [`program_uses_cpython`] (#852) but over the *lowered*
/// program: a backend's heavy system library (`libpcre2-8` for regex, `libz`
/// for zlib/gzip) is a `NEEDED` dynamic dependency worth paying for only when
/// the program actually calls one of that backend's runtime seam functions.
/// Every other program mapped and relocated them at process start — and refused
/// to run without them present — for nothing (#853).
///
/// Soundness: the predicates test the EXACT symbol the generated code
/// references (`Inst::Call`/`Invoke`/`FuncAddr`'s `func`), so there are no false
/// negatives — a reached backend is always detected and so never wrongly stubbed
/// (which would turn a real call into a runtime error). The regex/zlib seams are
/// distinct prefixes with no non-backend members, so a prefix test is exact. (A
/// prefix that could over-match — e.g. `zap_crypto_*`, whose SHA/HMAC/CSPRNG are
/// self-contained and do NOT use OpenSSL — is handled where that backend is
/// added, erring toward over-linking, never toward a miss.)
#[derive(Clone, Copy, Default)]
pub(crate) struct BackendUse {
    /// Regex via PCRE2 (`-lpcre2-8`): the program calls a `zap_regex_*` seam.
    pub regex: bool,
    /// zlib/gzip DEFLATE (`-lz`): the program calls a `zap_zlib_*`/`zap_gzip_*`
    /// seam. Independent of `http` — the net/http runtime does not use zlib.
    pub zlib: bool,
    /// OpenSSL (`-lssl -lcrypto`): the program calls a `zap_tls_*` (TLS), a
    /// `zap_http_*` (the http client embeds the OpenSSL `https://` path inline,
    /// so any http use is treated as needing OpenSSL — conservative but sound),
    /// or an OpenSSL-backed `zap_crypto_*` seam. The plain `zap_crypto_sha*`/
    /// `md5`/`hmac_*`/CSPRNG family is pure C (verified: no OpenSSL symbol is
    /// referenced below the OpenSSL region in `zaprt.c`), so it is EXCLUDED — a
    /// program that only hashes links no libssl. Over-matching a future
    /// self-contained `zap_crypto_*` would only over-link (never a miss).
    pub openssl: bool,
    /// Numeric accelerator (#912): the program calls a `zap_accel_*` seam that
    /// needs the BLAS provider (set by the matmul/accel dispatch, #904). Gates
    /// `-DZAP_HAVE_BLAS` on the runtime compile and the provider link flags
    /// (`-lopenblas` / `-framework Accelerate`). Off ⇒ `fast` degrades to
    /// `native` and no provider is linked. The always-compiled tier/thread knobs
    /// (`zap_accel_tier` / `zap_accel_threads_env`) do not set this.
    pub blas: bool,
    /// FFT via vendored PocketFFT (#929): the program calls a `zap_fft_*` seam
    /// whose transform lives in the C++ backend TU (`zapfft.cpp`). Gates
    /// `-DZAP_HAVE_FFT` on the runtime compile, compiles + links the C++ object,
    /// and switches the final link driver to the C++ variant (which pulls the C++
    /// stdlib). Off ⇒ no C++ object, no libc++/libstdc++, and the `zap_fft_*`
    /// entry points compile as catchable-error stubs. (`zap_fft_fftfreq` /
    /// `rfftfreq` are pure C and do not need the backend, but referencing any
    /// `zap_fft_*` conservatively links it — only over-linking, never a miss.)
    pub fft: bool,
    /// SIMD sort accelerator (#933): the program reaches the array sort path
    /// (`zap_array_sort_inplace` / `zap_array_sorted` / `zap_array_argsort`),
    /// whose unstable kernels live in the vendored C++ TUs (x86: x86-simd-sort
    /// `zapxsort.cpp`; aarch64: Highway vqsort `zaphwysort.cpp`). Gates
    /// `-DZAP_HAVE_SIMDSORT` on the runtime compile, compiles + links the per-ISA
    /// C++ object(s), and pulls the C++ stdlib. Off ⇒ no C++ object and the
    /// accelerator seam falls through to the own introsort floor (#932).
    pub simdsort: bool,
}

/// `zap_crypto_*` seams that are implemented in pure C (SHA-2 family, SHA-1,
/// MD5, the streaming-hash handle API, HMAC, and the CSPRNG helpers) and so do
/// NOT pull OpenSSL — the exclusion set for the OpenSSL predicate (#853). Kept
/// in sync with the un-gated `zap_crypto_*` definitions in `zaprt.c` (those
/// below the `#if !defined(ZAP_NO_OPENSSL)` crypto region). Being conservative
/// here (treating an unknown `zap_crypto_*` as OpenSSL-backed) only over-links;
/// a missed OpenSSL seam would instead break a real call, so the default — not
/// listed here ⇒ needs OpenSSL — is the safe one.
const SELF_CONTAINED_CRYPTO: &[&str] = &[
    "zap_crypto_sha256",
    "zap_crypto_sha512",
    "zap_crypto_sha384",
    "zap_crypto_sha224",
    "zap_crypto_sha1",
    "zap_crypto_md5",
    "zap_crypto_new",
    "zap_crypto_update",
    "zap_crypto_digest",
    "zap_crypto_hexdigest",
    "zap_crypto_list_index",
    "zap_crypto_hmac_sha256",
    "zap_crypto_hmac_sha512",
    "zap_crypto_hmac_sha256_raw",
    "zap_crypto_hmac_new",
    "zap_crypto_hmac_update",
    "zap_crypto_hmac_digest",
    "zap_crypto_hmac_hexdigest",
    "zap_crypto_random_bytes",
    "zap_crypto_token_hex",
    "zap_crypto_token_urlsafe",
    "zap_crypto_randbelow",
];

/// Scan the lowered program for the optional-backend seam symbols it references
/// (#853). Lowers the module once for the scan; the backend lowers again for
/// codegen, but lowering is deterministic, so both see the same symbol set.
pub(crate) fn program_backend_use(module: &AstModule, types: &TypeMap, race: bool) -> BackendUse {
    // Symbol-scan only (no runtime diagnostics), so no source location is needed.
    let (zir, _errs) = lower_module_opts_with_types_located(module, race, types, None);
    let mut u = BackendUse::default();
    for f in &zir.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                let callee = match inst {
                    Inst::Call { func, .. }
                    | Inst::Invoke { func, .. }
                    | Inst::FuncAddr { func, .. } => func.as_str(),
                    _ => continue,
                };
                if callee.starts_with("zap_regex_") {
                    u.regex = true;
                } else if callee.starts_with("zap_zlib_") || callee.starts_with("zap_gzip_") {
                    u.zlib = true;
                } else if callee.starts_with("zap_tls_")
                    || callee.starts_with("zap_http_")
                    || (callee.starts_with("zap_crypto_")
                        && !SELF_CONTAINED_CRYPTO.contains(&callee))
                {
                    u.openssl = true;
                } else if callee.starts_with("zap_array_matmul")
                    || callee.starts_with("zap_linalg_")
                {
                    // #904: matrix multiply (`@` / 2-D `dot`) auto-dispatches to
                    // a BLAS gemm for large float products (#912 seam); #926:
                    // `import linalg` (solve/inv/det) is LAPACK-backed. Either
                    // makes the program compile the runtime with the provider
                    // (`ZAP_HAVE_BLAS`) and link it — exactly like the pcre2/zlib
                    // seams. 1-D·1-D `dot` (`zap_array_dot{i,f}`) stays the scalar
                    // accumulator and never needs BLAS.
                    u.blas = true;
                } else if callee.starts_with("zap_fft_") {
                    // #929: `import fft` is PocketFFT-backed; its transform lives
                    // in the C++ backend TU, so a program that reaches it compiles
                    // the runtime with `ZAP_HAVE_FFT`, compiles + links the C++
                    // object, and links the C++ stdlib. (fftfreq/rfftfreq are pure
                    // C, but referencing any `zap_fft_*` conservatively links the
                    // backend — only over-linking, never a miss.)
                    u.fft = true;
                } else if callee.starts_with("zap_array_sort")
                    || callee.starts_with("zap_array_argsort")
                {
                    // #933: the array sort path (sort/argsort) has a vendored SIMD
                    // accelerator (x86-simd-sort / Highway vqsort) in a C++ TU; a
                    // program that sorts compiles the runtime with
                    // `-DZAP_HAVE_SIMDSORT`, compiles + links the per-ISA object(s),
                    // and links the C++ stdlib. The own introsort floor stays the
                    // fallback (small/strided/stable/other-dtype), so this is only
                    // over-linking (the accelerator), never a correctness miss.
                    u.simdsort = true;
                }
            }
        }
    }
    u
}

/// The numeric-acceleration provider's build flags (#912, D-0179) as
/// `(compile_defines, link_args)`, or `None` when none is available. Called only
/// for a program that reaches a `zap_accel_*` seam (`BackendUse::blas`), so a
/// program with no accelerated op pays nothing. Detection is best-effort and
/// build-time (there is no opt-out knob — #925 dropped `ZAP_NO_BLAS`: a program
/// that uses no accel-eligible op already links no provider, via LTO/dead-strip
/// and the per-program `BackendUse::blas` gate, and `ZAP_ACCEL` is the runtime
/// override):
///   * macOS ⇒ Apple Accelerate (a system framework, always present):
///     `-DZAP_BLAS_ACCELERATE` + `-framework Accelerate`.
///   * otherwise (Linux) ⇒ OpenBLAS when `pkg-config openblas` resolves (the CI
///     image / dev container ship `libopenblas-dev`), else `None`.
///
/// `None` (no provider found) leaves `ZAP_HAVE_BLAS` undefined, so
/// `zap_accel_available()` is 0 and the `fast` tier degrades to `native` (the
/// graceful fallback for an exotic host without a provider). The compile defines
/// always include `-DZAP_HAVE_BLAS` and the provider macro; the OpenBLAS arm also
/// folds in `pkg-config --cflags` so a later `#include <cblas.h>` (the gemm
/// dispatch, #904) finds its header.
fn blas_build_flags() -> Option<(Vec<String>, Vec<String>)> {
    let triple = TargetMachine::get_default_triple();
    let triple_str = triple.as_str().to_string_lossy().into_owned();
    let is_macos =
        triple_str.contains("darwin") || triple_str.contains("macos") || cfg!(target_os = "macos");
    if is_macos {
        return Some((
            vec!["-DZAP_HAVE_BLAS".into(), "-DZAP_BLAS_ACCELERATE".into()],
            vec!["-framework".into(), "Accelerate".into()],
        ));
    }
    // Linux / other: require pkg-config to resolve OpenBLAS, so a host without
    // the provider falls back gracefully to `native` instead of a link error.
    let libs = Command::new("pkg-config")
        .args(["--libs", "openblas"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let link: Vec<String> = String::from_utf8_lossy(&libs.stdout)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if link.is_empty() {
        return None;
    }
    let mut defines = vec![
        "-DZAP_HAVE_BLAS".to_string(),
        "-DZAP_BLAS_OPENBLAS".to_string(),
    ];
    if let Ok(cf) = Command::new("pkg-config")
        .args(["--cflags", "openblas"])
        .output()
    {
        if cf.status.success() {
            defines.extend(
                String::from_utf8_lossy(&cf.stdout)
                    .split_whitespace()
                    .map(str::to_string),
            );
        }
    }
    Some((defines, link))
}

/// Build a native executable. With `portable`, use the portable link profile
/// (I#287): statically link (`-static`) and drop CPython embedding
/// (`-DZAP_NO_CPYTHON`, no `pkg-config python3-embed`) so the binary is
/// self-contained and runs on minimal / older-glibc distros such as Amazon
/// Linux 2023 (the AWS `provided.al2023` Lambda base), which a dynamically
/// glibc-2.39-linked Ubuntu build cannot. `ZAP_PORTABLE=1` forces it on too, so
/// a toolchain image / CI can default every build to portable without the flag.
///
/// Build flags read from the environment (so a toolchain image / CI can set a
/// default without threading a parameter through every caller):
///   * `ZAP_PORTABLE` — the self-contained static link profile (above).
///   * `ZAP_LTO` — LTO the C runtime into the program (I#841, D-0149): emit the
///     program as LLVM bitcode, compile the runtime to bitcode, merge the two
///     with `llvm-link`, and optimize/codegen the merged module with `clang -O2`
///     so LLVM inlines the hot runtime fast paths into generated code. OFF by
///     default; the non-LTO object-link path and its output are unchanged when
///     unset. The driver is `clang` (override `ZAP_LTO_CC`, e.g. `clang-18`) and
///     must match the LLVM that emits the program bitcode (18).
///   * `ZAP_LLVM_LINK` — the `llvm-link` tool for the LTO merge (default
///     `llvm-link`; CI pins the matching LLVM 18 one).
///   * `ZAP_OPT` — the `opt` tool that runs `-O2` (incl. the inliner) over the
///     merged LTO module (default `opt`; CI pins the LLVM 18 one).
///   * `ZAP_LTO_CACHE` — directory for the cached runtime bitcode (default a
///     stable temp subdir); the runtime is compiled to bitcode once per
///     (source, collector, portable, clang, triple) and reused across builds.
///   * `ZAP_CC` / `ZAP_LTO_CC` — the linker driver for the default / LTO path.
///   * `ZAP_CC_EXTRA` — extra driver flags (diagnostics).
///
/// `debug` (the `zap build --debug` flag, I#867/D-0157) compiles the C runtime
/// translation unit at `-O0` instead of the default `-O2` for fast iteration; it
/// affects only the default object-link path's runtime compile (see
/// [`runtime_opt_flag`]; `-O2` by default on every target) and is a no-op under
/// `ZAP_LTO`.
#[allow(clippy::too_many_arguments)] // +`loc` for located NullReferenceError messages (#1493)
pub fn build_executable(
    module: &AstModule,
    types: &TypeMap,
    out_path: &Path,
    portable: bool,
    race: bool,
    debug: bool,
    fp: zir::FpPolicy,
    loc: Option<SourceLoc>,
) -> Result<(), String> {
    // `ZAP_PORTABLE` (any value) is an environment-level alias for `--portable`:
    // it lets a deployment image default all builds to the portable profile.
    let portable = portable || std::env::var_os("ZAP_PORTABLE").is_some();
    // LTO the C runtime into the program (I#841, D-0149). Gated behind the
    // `ZAP_LTO` env build flag — OFF by default, so the normal object-link path
    // below (and the binary it produces) is byte-for-byte unchanged. When set,
    // the program is emitted as LLVM bitcode, the runtime is compiled to LLVM
    // bitcode, the two are **merged into one module with `llvm-link`**, and that
    // single module is optimized + codegen'd by `clang -O2`. Merging explicitly
    // (rather than passing a `.bc` + `-flto` and relying on the linker's LTO
    // plugin) is what actually lets LLVM's cross-module inliner pull the hot
    // runtime fast paths (`zap_dict_get_*`, `zap_str_concat`, `zap_i64_to_str`,
    // …) into the generated hot loops: with a merged module the inliner sees
    // both sides and needs no gold/lld plugin. The language exposes no `inline`
    // keyword — inlining is purely the optimizer's job, and this flag extends
    // its reach across the program↔runtime boundary (D-0149). A toolchain image
    // / CI can default every build to LTO by exporting `ZAP_LTO=1` (the same
    // env-alias pattern as `ZAP_PORTABLE`). The default is kept conservative
    // (off) because the merge+`-O2` is slower than the object link and, unlike
    // it, is not yet the long-exercised default. See DECISIONS.md.
    let lto = std::env::var_os("ZAP_LTO").is_some();
    // Under LTO, strip the dead runtime at link time (`-ffunction-sections` +
    // `--gc-sections`): the WHOLE runtime (zaprt.c — http/json/crypto/regex/…)
    // is merged into every program, but the linker can drop the functions no
    // program path reaches, so a program that uses only `dict`+`print` links a
    // fraction of it — a much smaller binary, which also cuts exec/page-in time
    // (e.g. AWS Lambda cold start). This is sound because it never re-optimizes:
    // `opt -O2` ran with the runtime at external linkage, so threaded codegen is
    // unchanged (IR-level `internalize`+`globaldce` before `-O2` would instead
    // let the optimizer treat scheduler/GC/race globals as private and miscompile
    // the concurrent runtime — rejected). `ZAP_LTO_NO_PRUNE` keeps the whole
    // runtime (and restores `-rdynamic` profiler visibility). Default ON for LTO.
    let prune = lto && std::env::var_os("ZAP_LTO_NO_PRUNE").is_none();
    // The program artifact handed to the linker driver: a native object on the
    // default path, LLVM bitcode under LTO so the cross-module inliner sees it.
    let prog_path = out_path.with_extension(if lto { "bc" } else { "o" });
    if lto {
        LlvmBackend.compile_bitcode(module, types, &prog_path, race, fp, loc)?;
        // Clear any inline-blocking target/FP attributes from the (small) program
        // module up front. It carries none today, but stripping it here — plus
        // stripping the cached runtime bitcode once (below) — means the merged
        // module is uniformly attribute-free with NO per-build strip of the big
        // merged module, so the LTO inliner can always cross the boundary
        // (I#841). Cheap: the program module is small.
        strip_target_attrs_bitcode(&prog_path)?;
    } else {
        LlvmBackend.compile_object(module, types, &prog_path, race, fp, loc)?;
    }

    // Write the runtime into a per-build temp dir and let cc compile + link it
    // in. The GC collector include is written alongside so `#include
    // "zapgc.inc.c"` (relative) resolves (ticket #48).
    let build_dir = std::env::temp_dir().join(format!("zap-build-{}", std::process::id()));
    std::fs::create_dir_all(&build_dir).map_err(|e| format!("failed to create build dir: {e}"))?;
    let rt_path = build_dir.join("zaprt.c");
    // Write `zaprt.c` and every in-TU fragment (incl. the `runtime/` subtree) in
    // one shot from the manifest, so this site cannot drift from the REPL/JIT one.
    runtime_manifest::write_tu_fragments(&build_dir)
        .map_err(|e| format!("failed to write runtime: {e}"))?;

    // Target-aware link step (ticket #199, platform epic #158 / Area A5). Today
    // codegen emits for the host triple only, so "Windows target" means "running
    // the Zap compiler on a Windows (MinGW-w64) host"; we branch on the triple
    // string so the Windows arm is still type-checked by the Linux/macOS build
    // (it just isn't taken there), keeping this behaviour-preserving off Windows.
    //
    // NOTE: a full Windows link does not succeed yet — the C runtime (`zaprt.c`)
    // still needs the A1 context switch (#195), A3 signals/preemption/faults and
    // unwinder (#197), and A2/A4 before it compiles on Windows; the runtime's
    // platform gate fails the compile with that map. This arm wires the correct
    // MinGW-w64 toolchain so that work drops straight in. See the PR body and
    // design/0016-platform-1.0.md (Area A).
    let triple = TargetMachine::get_default_triple();
    let triple_str = triple.as_str().to_string_lossy().into_owned();
    let is_windows = triple_str.contains("windows");

    // CPython link gate (#852). Embedding CPython — and so emitting the
    // `libpython` `NEEDED` dynamic dependency and its `pkg-config` cflags/libs —
    // is worthwhile ONLY for a program that actually `import python`s. Every
    // other program linked libpython for nothing, paying its loader mmap/reloc at
    // every process start and refusing to run without a matching libpython
    // present (the startup-floor / portability cost in the issue). So link
    // CPython only when the import graph reaches `import python` AND neither the
    // portable profile (I#287) nor Windows v1 (I#198) has already opted out — in
    // which case the runtime's `import python` entry points compile as the
    // existing catchable-error stubs (`-DZAP_NO_CPYTHON`) and no libpython links.
    // The explicit `--portable`/`ZAP_PORTABLE` override is unchanged: it still
    // forces the no-CPython path regardless of the import graph.
    let needs_cpython = program_uses_cpython(module);
    let link_cpython = needs_cpython && !portable && !is_windows;
    let no_cpython = !link_cpython;

    // Optional-backend link gate (#853), the same idea as the CPython gate but
    // for the stdlib's optional C backends: link `libpcre2-8` only when the
    // program reaches the regex seam, and `libz` only when it reaches the
    // zlib/gzip seam. When a backend is unused the runtime compiles its entry
    // points as catchable-error stubs (`-DZAP_NO_PCRE2` / `-DZAP_NO_ZLIB`, like
    // `-DZAP_NO_CPYTHON`) so it references no pcre2/zlib symbol and the matching
    // `-l` can be dropped — no `NEEDED` entry, no loader cost, and the program
    // builds on a host without that `-dev` package. Applies on every profile
    // (incl. portable, which still benefits from a smaller static link).
    // OpenSSL (`-lssl -lcrypto`) is gated the same way: linked only when the
    // program reaches a TLS / crypto / http seam, else the OpenSSL code in
    // `zaprt.c` (crypto block, TLS transport, and the https branches inside the
    // http client) is `#if !defined(ZAP_NO_OPENSSL)`-compiled out. No stubs are
    // needed, unlike CPython: a program that reaches no OpenSSL seam references
    // none of those symbols, so the compiled-out functions are simply absent.
    let backends = program_backend_use(module, types, race);
    let no_regex = !backends.regex;
    let no_zlib = !backends.zlib;
    let no_openssl = !backends.openssl;
    // Numeric accelerator (#912): resolve the provider's build flags only for a
    // program that reaches a `zap_accel_*` seam (the matmul/accel dispatch, #904)
    // — otherwise nothing probes for a provider and nothing links. A reaching
    // program with no provider available gets empty flags (the seam compiles as
    // its no-provider stubs: `fast` degrades to `native`).
    let (mut blas_defines, blas_libs): (Vec<String>, Vec<String>) = if backends.blas {
        blas_build_flags().unwrap_or_default()
    } else {
        (Vec::new(), Vec::new())
    };
    // FFT backend (#929). A program that reaches a `zap_fft_*` seam compiles the
    // runtime with `-DZAP_HAVE_FFT` (so the PocketFFT-backed transforms are
    // included, not the no-backend stubs) — folded into the runtime compile
    // defines here (same mechanism as the BLAS gate) — and compiles the C++
    // wrapper to a native object linked below (the C++ stdlib is already on the
    // link line). The wrapper and the vendored PocketFFT header are written next
    // to the runtime so the `#include` resolves.
    let fft_obj: Option<std::path::PathBuf> = if backends.fft {
        blas_defines.push("-DZAP_HAVE_FFT".to_string());
        let cpp_path = build_dir.join("zapfft.cpp");
        if let Err(e) = std::fs::write(&cpp_path, RUNTIME_FFT_CPP)
            .and_then(|_| std::fs::write(build_dir.join("pocketfft_hdronly.h"), POCKETFFT_H))
        {
            let _ = std::fs::remove_file(&prog_path);
            let _ = std::fs::remove_dir_all(&build_dir);
            return Err(format!("failed to write fft backend: {e}"));
        }
        let obj = build_dir.join("zapfft.o");
        // Compile the C++ TU with the C driver in C++ mode (`-x c++`), so no
        // separate C++ compiler path is needed; `-fexceptions` because PocketFFT
        // throws on a malformed transform and the wrapper catches at the boundary
        // (I#841: a unit that unwinds must be built with exceptions). It is a
        // standalone native object (never merged into the program's LTO bitcode),
        // so the C↔C++ boundary stays an ordinary extern-"C" call.
        let cxx = if lto {
            std::env::var("ZAP_LTO_CC").unwrap_or_else(|_| "clang".to_string())
        } else {
            std::env::var("ZAP_CC").unwrap_or_else(|_| "cc".to_string())
        };
        let status = Command::new(&cxx)
            .arg("-x")
            .arg("c++")
            .arg("-std=c++17")
            .arg("-O2")
            .arg("-fexceptions")
            .arg("-c")
            .arg(&cpp_path)
            .arg(format!("-I{}", build_dir.display()))
            .arg("-o")
            .arg(&obj)
            .status();
        match status {
            Ok(s) if s.success() => Some(obj),
            Ok(_) => {
                let _ = std::fs::remove_file(&prog_path);
                let _ = std::fs::remove_dir_all(&build_dir);
                return Err("failed to compile the FFT backend (zapfft.cpp)".to_string());
            }
            Err(e) => {
                let _ = std::fs::remove_file(&prog_path);
                let _ = std::fs::remove_dir_all(&build_dir);
                return Err(format!(
                    "failed to run the C++ compiler for zapfft.cpp: {e}"
                ));
            }
        }
    } else {
        None
    };

    // SIMD sort accelerator objects (#933): for a program that sorts
    // (BackendUse::simdsort), compile the per-ISA vendored kernels and gate the
    // dispatch seam on `-DZAP_HAVE_SIMDSORT`. x86-64 → two x86-simd-sort tiers
    // (AVX2 + AVX-512); aarch64 → one Highway NEON object + Highway's one needed
    // `abort.cc`. The vendored headers (embedded by build.rs) are written under a
    // `vendored/` subtree so each library's internal `#include`s resolve. Native
    // objects (never merged into the program bitcode), linked below with the C++
    // stdlib that is already on the link line.
    let simdsort_objs: Vec<std::path::PathBuf> = if backends.simdsort {
        blas_defines.push("-DZAP_HAVE_SIMDSORT".to_string());
        let fail = |build_dir: &std::path::Path, prog_path: &std::path::Path, msg: String| {
            let _ = std::fs::remove_file(prog_path);
            let _ = std::fs::remove_dir_all(build_dir);
            msg
        };
        // Write the embedded vendored headers under build_dir/vendored/<relpath>.
        let vroot = build_dir.join("vendored");
        for (rel, content) in VENDORED_SORT_FILES {
            let dst = vroot.join(rel);
            if let Some(parent) = dst.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return Err(fail(
                        &build_dir,
                        &prog_path,
                        format!("failed to write vendored sort header dir: {e}"),
                    ));
                }
            }
            if let Err(e) = std::fs::write(&dst, content) {
                return Err(fail(
                    &build_dir,
                    &prog_path,
                    format!("failed to write vendored sort header {rel}: {e}"),
                ));
            }
        }
        let cxx = if lto {
            std::env::var("ZAP_LTO_CC").unwrap_or_else(|_| "clang".to_string())
        } else {
            std::env::var("ZAP_CC").unwrap_or_else(|_| "cc".to_string())
        };
        let triple_s = triple.as_str().to_string_lossy().to_string();
        let is_x86 = triple_s.contains("x86_64") || triple_s.contains("x86-64");
        let is_arm = triple_s.contains("aarch64") || triple_s.contains("arm64");
        // Compile one C++ TU to a native object with the given extra flags.
        let compile = |src_name: &str,
                       src: &str,
                       obj_name: &str,
                       extra: &[&str]|
         -> Result<std::path::PathBuf, String> {
            let src_path = build_dir.join(src_name);
            std::fs::write(&src_path, src).map_err(|e| format!("write {src_name}: {e}"))?;
            let obj = build_dir.join(obj_name);
            let mut c = Command::new(&cxx);
            c.arg("-x")
                .arg("c++")
                .arg("-std=c++17")
                .arg("-O2")
                .arg("-c")
                .arg(&src_path);
            for a in extra {
                c.arg(a);
            }
            c.arg("-o").arg(&obj);
            match c.status() {
                Ok(s) if s.success() => Ok(obj),
                Ok(_) => Err(format!(
                    "failed to compile the SIMD sort kernel ({src_name})"
                )),
                Err(e) => Err(format!(
                    "failed to run the C++ compiler for {src_name}: {e}"
                )),
            }
        };
        let mut objs = Vec::new();
        let r: Result<(), String> = (|| {
            if is_x86 {
                let xinc = format!("-I{}", vroot.join("x86-simd-sort/src").display());
                objs.push(compile(
                    "zapxsort_avx2.cpp",
                    RUNTIME_XSORT_CPP,
                    "zapxsort_avx2.o",
                    &["-DZAP_XSORT_TIER=avx2", "-mavx2", &xinc],
                )?);
                objs.push(compile(
                    "zapxsort_avx512.cpp",
                    RUNTIME_XSORT_CPP,
                    "zapxsort_avx512.o",
                    &[
                        "-DZAP_XSORT_TIER=avx512",
                        "-mavx512f",
                        "-mavx512dq",
                        "-mavx512vl",
                        "-mavx512bw",
                        &xinc,
                    ],
                )?);
            } else if is_arm {
                let hinc = format!("-I{}", vroot.join("highway").display());
                objs.push(compile(
                    "zaphwysort.cpp",
                    RUNTIME_HWYSORT_CPP,
                    "zaphwysort.o",
                    &[&hinc],
                )?);
                // Highway's static vqsort references hwy::Abort (abort.cc only).
                let abort_src = std::fs::read_to_string(vroot.join("highway/hwy/abort.cc"))
                    .map_err(|e| format!("read vendored highway abort.cc: {e}"))?;
                objs.push(compile(
                    "hwy_abort.cpp",
                    &abort_src,
                    "hwy_abort.o",
                    &[&hinc],
                )?);
            }
            Ok(())
        })();
        if let Err(e) = r {
            return Err(fail(&build_dir, &prog_path, e));
        }
        objs
    } else {
        Vec::new()
    };

    // Linker driver. On the default path, `cc` (the MinGW-w64 `gcc`/`cc` on a
    // Windows host provides the `x86_64-pc-windows-gnu` target); override with
    // `$ZAP_CC` (e.g. a cross driver `x86_64-w64-mingw32-gcc`). Added for the
    // Windows bring-up but useful on every platform. Under LTO, the driver MUST
    // be the clang matching the LLVM that emitted the program bitcode (LLVM 18):
    // a plain `gcc`/`cc` cannot consume LLVM bitcode or drive the LTO backend.
    // Default `clang`, override with `$ZAP_LTO_CC` (CI pins `clang-18`, which
    // also backs the embedded runtime's `#include` resolution).
    let cc = if lto {
        std::env::var("ZAP_LTO_CC").unwrap_or_else(|_| "clang".to_string())
    } else {
        std::env::var("ZAP_CC").unwrap_or_else(|_| "cc".to_string())
    };

    // Under LTO, build a single merged bitcode module (program + runtime) and
    // hand THAT to the driver, so the cross-module inliner has already seen both
    // sides by the time we optimize. `program_input` is the primary artifact the
    // final driver command consumes: the merged module under LTO, else the
    // program object (with the runtime source compiled alongside it).
    let program_input = if lto {
        // 1. Obtain the runtime as LLVM bitcode — CACHED (I#841 perf). The
        //    runtime bitcode depends only on the embedded runtime source, the
        //    collector, the CPython/portable compile flags, and the clang/triple
        //    — NOT on the program being built — so it is identical across every
        //    program. The non-cached path recompiled the whole runtime (and
        //    stripped the whole merged module) for EVERY build; the golden corpus
        //    builds hundreds of programs, which is why the LTO suite crawled.
        //    Compile + strip it once, publish into a shared cache, and reuse.
        let rt_bc = match lto_runtime_bitcode(
            &cc,
            &rt_path,
            triple.as_str().to_string_lossy().as_ref(),
            no_cpython,
            no_regex,
            no_zlib,
            no_openssl,
            &blas_defines,
        ) {
            Ok(path) => path,
            Err(e) => {
                let _ = std::fs::remove_file(&prog_path);
                let _ = std::fs::remove_dir_all(&build_dir);
                return Err(e);
            }
        };
        // 2. Merge the program and runtime modules into ONE with `llvm-link`, so
        //    `clang -O2` below inlines across the former program↔runtime
        //    boundary with no dependence on a linker LTO plugin. `$ZAP_LLVM_LINK`
        //    overrides the tool (CI pins the matching LLVM 18 `llvm-link`). Both
        //    inputs are already attribute-free (the program was stripped above,
        //    the cached runtime was stripped before caching), so the merged
        //    module needs no further strip — the inliner can cross the boundary.
        let llvm_link = std::env::var("ZAP_LLVM_LINK").unwrap_or_else(|_| "llvm-link".to_string());
        let merged = build_dir.join("merged.bc");
        let status = Command::new(&llvm_link)
            .arg(&prog_path)
            .arg(&rt_bc)
            .arg("-o")
            .arg(&merged)
            .status()
            .map_err(|e| format!("failed to run llvm-link: {e}"))?;
        if !status.success() {
            let _ = std::fs::remove_file(&prog_path);
            let _ = std::fs::remove_dir_all(&build_dir);
            return Err(format!(
                "llvm-link (program + runtime bitcode) failed: {status}"
            ));
        }
        // 3. Optimize the merged module with `opt -O2`. This UNAMBIGUOUSLY runs
        //    the middle-end pipeline — crucially the inliner — over the whole
        //    module, so the runtime fast paths inline into generated code.
        //    (`clang -O2` on a bitcode input codegens it but does not reliably
        //    re-run the IR optimizer, so the inlining must happen here.) clang
        //    below then only codegens + links the already-optimized module.
        //    `$ZAP_OPT` overrides the tool (CI pins the LLVM 18 `opt`).
        //
        //    NOTE on dead-runtime removal: the WHOLE runtime (zaprt.c —
        //    http/json/crypto/regex/…) is merged into every program, and a
        //    single-module `-O2` can NOT drop the unused parts (they have
        //    external linkage, so they are not provably dead). We deliberately
        //    do NOT prune here with `internalize` + `globaldce`: internalizing
        //    the runtime before `-O2` lets the optimizer treat the scheduler /
        //    GC / race-detector globals as unobservable and hoist or cache loads
        //    another thread mutates, which miscompiles the concurrent runtime
        //    (false `--race` reports and hangs). Instead the dead runtime is
        //    stripped soundly at LINK time via `-ffunction-sections` +
        //    `--gc-sections` (see the link step), which never re-optimizes, so
        //    codegen for live code — and the threaded runtime — is unchanged.
        let opt = std::env::var("ZAP_OPT").unwrap_or_else(|_| "opt".to_string());
        let merged_opt = build_dir.join("merged.opt.bc");
        let status = Command::new(&opt)
            .arg("-O2")
            .arg(&merged)
            .arg("-o")
            .arg(&merged_opt)
            .status()
            .map_err(|e| format!("failed to run opt: {e}"))?;
        if !status.success() {
            let _ = std::fs::remove_file(&prog_path);
            let _ = std::fs::remove_dir_all(&build_dir);
            return Err(format!("opt -O2 (merged module) failed: {status}"));
        }
        // Diagnostic hook (I#841): when `ZAP_LTO_KEEP=<dir>` is set, copy the
        // pre- and post-optimize merged modules out of the temp build dir so a
        // test (or a developer) can disassemble them and see why a runtime call
        // did or did not inline. Off unless the env var is set; never affects a
        // normal build.
        if let Some(keep) = std::env::var_os("ZAP_LTO_KEEP") {
            let keep = std::path::PathBuf::from(keep);
            let _ = std::fs::create_dir_all(&keep);
            let _ = std::fs::copy(&merged, keep.join("merged.bc"));
            let _ = std::fs::copy(&merged_opt, keep.join("merged.opt.bc"));
        }
        merged_opt
    } else {
        prog_path.clone()
    };

    let mut cmd = Command::new(&cc);
    cmd.arg(&program_input);
    if lto {
        // The merged module was already inlined/optimized by `opt -O2` above;
        // clang here codegens + links it (-O2 for backend codegen quality).
        // `zap_shadow_head_addr` keeps its `__attribute__((noinline))` and so
        // stays an out-of-line call even after inlining (coroutine-migration
        // correctness: it must resolve the per-thread shadow-head slot on the
        // CURRENT thread, not a folded/cached base — I#223 / #568). The GC-frame
        // + safepoint discipline (D-0144) is preserved: the shadow-frame alloca
        // escapes via `*zap_shadow_head = &frame` so -O2 cannot promote it away,
        // and inlining a runtime call that contains a safepoint keeps the
        // safepoint. `crates/driver/tests/lto.rs` asserts the hot `d[k]` fast
        // path inlines while that call survives.
        cmd.arg("-O2");
        // Strip the dead runtime at link time (see `prune` above). Emit one
        // section per function/datum so the linker's `--gc-sections` can drop
        // the ones no retained section references (roots: `main`, the GC's
        // `.init_array` constructors, anything address-taken). Sound — no
        // re-optimization — so the threaded runtime's codegen is unchanged.
        // Not on Windows (PE/COFF link; the port is still stubbed).
        if prune && !is_windows {
            cmd.arg("-ffunction-sections")
                .arg("-fdata-sections")
                .arg("-Wl,--gc-sections");
        }
    } else {
        // Default path: the runtime is its own translation unit linked against the
        // already-codegen'd program object, so runtime calls stay out-of-line (the
        // opaque-external-symbol boundary #841/#568 measured).
        //
        // Optimize that runtime TU (I#867, D-0157). It carried NO `-O` flag, so
        // the system `cc` compiled the whole dict/set/string/GC machinery at
        // `-O0`, and every hot generated op paid into an unoptimized runtime
        // (`dict_int`: 26.9ms at `-O0` vs 19.0ms at `-O2`, which then beats Go's
        // 22.5ms). `-O2` by default on every target (the aarch64 `-O2` coroutine
        // crash, I#869, is fixed in the runtime itself — see `runtime_opt_flag`);
        // `--debug` restores `-O0` for fast iteration. The already-codegen'd
        // program object is only linked,
        // so its code is unaffected. The LTO path (above) is untouched.
        let opt_flag = runtime_opt_flag(debug);
        if is_windows {
            // Windows still compiles the runtime inline alongside the A1/A3/A4 TUs
            // below (the port is stubbed and not the CI-hot path, so it does not
            // use the object cache). `-O` applies to all those source TUs here.
            cmd.arg(opt_flag);
            cmd.arg(&rt_path);
        } else {
            // Compile `zaprt.c` to a cached `.o` ONCE per (source, opt, collector,
            // backend gates, CPython, cc, triple) and just LINK it here, instead
            // of recompiling the large runtime TU for every program. At the `-O2`
            // default this is what keeps a golden / `zap test` run (hundreds of
            // builds) from each paying the ~3s runtime compile — the object-link
            // analogue of the LTO bitcode cache. Only the compile-relevant flags
            // go into the cached object; the link-only flags (`-l*`, `-static`,
            // `-rdynamic`, the `-D*` that merely mirror the cache key) stay on this
            // link command below and are harmless no-ops against an object input.
            let rt_obj = match native_runtime_object(
                &cc,
                &rt_path,
                triple_str.as_str(),
                opt_flag,
                no_cpython,
                no_regex,
                no_zlib,
                no_openssl,
                &blas_defines,
            ) {
                Ok(p) => p,
                Err(e) => {
                    let _ = std::fs::remove_file(&prog_path);
                    let _ = std::fs::remove_dir_all(&build_dir);
                    return Err(e);
                }
            };
            cmd.arg(&rt_obj);
        }
    }
    cmd.arg("-o").arg(out_path);
    // Select the collector (ticket #48): the precise Immix GC is compiled into
    // the runtime with -DZAP_GC_IMMIX and needs no Boehm library; the default
    // build links the conservative Boehm collector (-lgc). On Windows the Boehm
    // library is `-lgc` too (MinGW-w64 `mingw-w64-x86_64-boehm-gc`, built with
    // thread support — the conservative fallback the A1 spike keeps working).
    #[cfg(feature = "gc-immix")]
    cmd.arg("-DZAP_GC_IMMIX");
    #[cfg(not(feature = "gc-immix"))]
    cmd.arg("-lgc");
    // Optional-backend compile gates (#853). When the program never reaches a
    // backend, compile its runtime entry points as catchable-error stubs so the
    // runtime references no pcre2/zlib symbol and the matching `-l` below can be
    // dropped. These reach the non-LTO in-line `zaprt.c` compile (this `cmd`
    // compiles `rt_path`); under LTO the same gates are baked into the cached
    // runtime bitcode by `lto_runtime_bitcode`. A `-D` is harmless (ignored)
    // when the backend IS used and the flag is absent. Added on every profile.
    if no_regex {
        cmd.arg("-DZAP_NO_PCRE2");
    }
    if no_zlib {
        cmd.arg("-DZAP_NO_ZLIB");
    }
    if no_openssl {
        cmd.arg("-DZAP_NO_OPENSSL");
    }
    // Libraries common to the POSIX (`cc` + system libs) and Windows (MinGW-w64)
    // link lines — the dev container / CI provide the `-dev` packages on Linux,
    // and the MinGW-w64 `mingw-w64-x86_64-{pcre2,zlib,openssl}` packages provide
    // the import libraries with the same `-l` names on Windows (MSYS2).
    // FFT backend object (#929): the PocketFFT C++ wrapper, listed before the C++
    // stdlib it references. Present only for a program that reaches `zap_fft_*`.
    if let Some(o) = &fft_obj {
        cmd.arg(o);
    }
    // SIMD sort accelerator objects (#933): the per-ISA x86-simd-sort / Highway
    // kernels, before the C++ stdlib they reference. Empty unless the program
    // sorts (BackendUse::simdsort).
    for o in &simdsort_objs {
        cmd.arg(o);
    }
    cmd.arg("-lstdc++")
        // `-lm` provides libm `pow`/`floor`, used by the scalar `**` and `//`
        // runtime helpers (ticket #19).
        .arg("-lm");
    // `-lpcre2-8` provides the 8-bit PCRE2 API the regex runtime wraps (ticket
    // #23); the dev container / CI provide `libpcre2-dev`. Linked only when the
    // program reaches the regex seam (#853); otherwise the runtime's regex entry
    // points are `-DZAP_NO_PCRE2` stubs that reference no pcre2 symbol.
    if !no_regex {
        cmd.arg("-lpcre2-8");
    }
    // `-lz` provides the system zlib backing the `zlib`/`gzip` compression
    // modules (ticket #106); the dev container / CI provide `zlib1g-dev` and it
    // ships on macOS. Linked only when the program reaches the zlib/gzip seam
    // (#853); otherwise those entry points are `-DZAP_NO_ZLIB` stubs.
    if !no_zlib {
        cmd.arg("-lz");
    }
    // `-lssl -lcrypto` provide the system OpenSSL/LibreSSL backing the `tls`
    // module, the OpenSSL-backed `crypto` primitives, and the `http` `https://`
    // client (ticket #147, D-0062); the dev container / CI provide `libssl-dev`
    // (OpenSSL 3.x on Ubuntu 24.04) and LibreSSL ships on macOS. Linked only when
    // the program reaches a TLS/crypto/http seam (#853); otherwise that code is
    // `-DZAP_NO_OPENSSL`-compiled out and references no OpenSSL symbol. `-lssl`
    // depends on `-lcrypto`, so that order.
    if !no_openssl {
        cmd.arg("-lssl").arg("-lcrypto");
    }
    // Numeric accelerator (#912): the BLAS/LAPACK provider's link flags
    // (`-lopenblas` on Linux via `pkg-config`, `-framework Accelerate` on macOS),
    // resolved into `blas_libs` only for a program that reaches a `zap_accel_*`
    // seam (#904). Empty otherwise, so a program with no accelerated op — and any
    // program on a host with no provider — links nothing extra.
    for a in &blas_libs {
        cmd.arg(a);
    }
    cmd
        // `-lpthread` backs the coroutine runtime (ticket #25): each `spawn`
        // starts a GC-registered pthread, and channels block on pthread
        // mutex/condvars. `libgc` must be thread-aware (the runtime defines
        // `GC_THREADS` and uses `GC_pthread_create`); the dev container's
        // `libgc-dev` is built with thread support. On Windows, MinGW-w64's
        // `winpthreads` provides `-lpthread` with the same API.
        .arg("-lpthread");
    if is_windows {
        // A1 coroutine context switch (ticket #195): the hand-written x86_64 asm
        // backend for the zap_ctx_swap/zap_ctx_make seam. POSIX uses `ucontext`
        // inline in zaprt.c; Windows has no ucontext, so this translation unit
        // provides the implementation and is compiled + linked in only here. It
        // is a no-op in the link until zaprt.c itself compiles on Windows (A3/A2/
        // A4), but wiring it now keeps the Windows arm a single drop-in and lets
        // the standalone harness (zap_ctx_win_x64_test.c) exercise the same file.
        let ctx_path = build_dir.join("zap_ctx_win_x64.c");
        std::fs::write(&ctx_path, RUNTIME_CTX_WIN_C)
            .map_err(|e| format!("failed to write ctx runtime: {e}"))?;
        cmd.arg(&ctx_path);
        // A3 signals/preemption/faults primitives (ticket #197): hijack-based
        // preemption, the fault diagnostic filter, and console signal mapping.
        // Like the A1 ctx file it is a drop-in TU, dead in the link until
        // `zaprt.c` itself compiles on Windows (still gated on A2/#196 + A4/#198);
        // wiring it now keeps the Windows arm a single drop-in.
        let a3_path = build_dir.join("zap_win_a3.c");
        std::fs::write(&a3_path, RUNTIME_WIN_A3_C)
            .map_err(|e| format!("failed to write a3 runtime: {e}"))?;
        cmd.arg(&a3_path);
        // A2 IOCP netpoller backend (ticket #196): the AFD-readiness-over-IOCP
        // mechanism behind the np_backend_* seam. Its header is written alongside
        // `zaprt.c` so the Windows arm's `#include "zap_np_iocp.h"` resolves; the
        // `.c` is added to the link. Like the A1/A3 files it is a drop-in TU, dead
        // in the link until `zaprt.c` itself compiles on Windows (still gated on
        // A4/#198). The `-lws2_32` the Winsock calls need is already on the link
        // line above.
        std::fs::write(build_dir.join("zap_np_iocp.h"), RUNTIME_NP_IOCP_H)
            .map_err(|e| format!("failed to write iocp header: {e}"))?;
        let iocp_path = build_dir.join("zap_np_iocp.c");
        std::fs::write(&iocp_path, RUNTIME_NP_IOCP_C)
            .map_err(|e| format!("failed to write iocp runtime: {e}"))?;
        cmd.arg(&iocp_path);
        // Windows (MinGW-w64 / `x86_64-pc-windows-gnu`) extras, ticket #199:
        //   * No `-rdynamic`: it is an ELF/dynamic-symbol-table flag; PE/COFF has
        //     no equivalent and the profiler/fault symbolication path it serves
        //     is itself part of the A3 (#197) Windows work, so it is simply
        //     omitted here.
        //   * No `pkg-config` CPython: Python embedding is gated OFF for Windows
        //     v1 (no reliable `python3-embed.pc` under MinGW; `import python`
        //     support on Windows is deferred — a known A4/#198 gap).
        //   * System libraries the *ported* runtime will need, wired ahead so A2/
        //     A3 drop in without touching the link line: `-lws2_32` (Winsock, for
        //     the A2/#196 IOCP netpoller), `-lbcrypt` (CNG `BCryptGenRandom`, the
        //     Windows `getentropy` replacement for crypto, A4/#198), `-ldbghelp`
        //     (`StackWalk64`/`SymFromAddr` for the A3/#197 fault backtrace). The
        //     linker ignores a listed library with no referenced symbols, so
        //     listing them now is harmless while the runtime is still stubbed.
        cmd.arg("-lws2_32").arg("-lbcrypt").arg("-ldbghelp");
    } else if portable {
        // Portable link profile (I#287, D-0082): produce a self-contained binary
        // that runs on minimal / older-glibc distros — in particular Amazon Linux
        // 2023 (glibc 2.34, the AWS `provided.al2023` Lambda base), where a
        // dynamically glibc-2.39-linked Ubuntu build fails to start (exit 127).
        //
        //   * `-static`: link everything statically, so there is NO glibc symbol-
        //     version floor at all — the binary carries its own libc, libstdc++,
        //     PCRE2, zlib, OpenSSL and (Immix) GC. The `-dev` packages in the
        //     toolchain image ship the `.a` archives these resolve against. (A
        //     dynamic glibc floor would instead require building on an older-glibc
        //     host; a full static link needs no such host and runs anywhere.)
        //   * `-DZAP_NO_CPYTHON`: drop CPython embedding. libpython is not present
        //     on AL2023 and static libpython is impractical, and CPython embedding
        //     is fundamentally at odds with a self-contained static binary. zaprt.c
        //     then compiles its `import python` entry points as catchable-error
        //     stubs (the same path Windows v1 uses, I#198) instead of linking
        //     libpython, so no `pkg-config python3-embed` flags are added below.
        //   * No `-rdynamic`: it is a dynamic-symbol-table flag at odds with a
        //     static link; the profiler/fault symbolication it serves is a dev
        //     feature, deliberately omitted from deployment binaries.
        //
        // Documented trade-offs (see docs/DOCKER.md): static glibc means NSS-backed
        // `getaddrinfo` cannot dlopen nsswitch plugins (standard DNS resolution is
        // unaffected), OpenSSL engine dlopen is unavailable (unused by the TLS
        // client), the binary is larger, and `import python` raises at runtime.
        //
        // `-ldl` after `-lcrypto`: static `libcrypto.a` references `dlopen`/`dlsym`
        // (engine loading), so a static link needs libdl explicitly and in this
        // order (GNU ld is single-pass). On glibc ≥ 2.34 libdl is a stub merged
        // into libc, so this is harmless where it is not strictly required.
        cmd.arg("-static").arg("-DZAP_NO_CPYTHON").arg("-ldl");
    } else {
        // Export program symbols to the dynamic symbol table so the CPU
        // profiler's offline symbolication (`backtrace_symbols`, ticket #78)
        // resolves Zap function names in folded stacks. This enlarges `.dynsym`
        // slightly but adds NO hot-path or codegen cost (unlike forcing frame
        // pointers) and preserves the "zero-cost when profiling is unused"
        // property — nothing runs until a SIGUSR2 / ZAP_PROFILE toggle. On the
        // fault-handler backtrace (ticket #77) it is a bonus: crash reports now
        // carry symbol names too. Skipped under LTO pruning: `-rdynamic` exports
        // every global symbol to `.dynsym`, which anchors it as a `--gc-sections`
        // root and would defeat the dead-runtime strip. The profiler/fault
        // symbolication of Zap names is a dev feature, deliberately traded away
        // in a pruned (size-optimized) LTO build, exactly as the portable/static
        // profile already does.
        if !prune {
            cmd.arg("-rdynamic");
        }
        if link_cpython {
            // CPython embedding flags (ticket #27, D-0012): the `-I...` cflags let
            // `zaprt.c` find `<Python.h>`, and the libs link libpython. Discovered
            // with `pkg-config` (the dev container / CI provide `python3-dev` +
            // `pkg-config`) so the exact libpython version and path are not
            // hardcoded. Added only when the import graph reaches `import python`
            // (#852); see the `link_cpython` gate above.
            cmd.args(python_embed_flags()?);
        } else {
            // The program never `import python`s (#852): compile the runtime's
            // `import python` entry points as catchable-error stubs — the same
            // `-DZAP_NO_CPYTHON` path the portable profile (I#287) and Windows v1
            // (I#198) take — and link NO libpython, so a plain `print(0)` carries
            // no `libpython` `NEEDED` and pays none of its loader cost. (On the
            // non-LTO path this flag reaches the in-line `zaprt.c` compile below;
            // under LTO the runtime bitcode was already built `-DZAP_NO_CPYTHON`
            // via `no_cpython`, so here it only keeps the define consistent.) We
            // also skip the `pkg-config python3-embed` probe entirely, so a
            // non-Python program builds on a host without `python3-dev`.
            cmd.arg("-DZAP_NO_CPYTHON");
        }
    }
    // Debug hook: extra cc flags from $ZAP_CC_EXTRA (e.g. "-g -fsanitize=address")
    // for diagnosing runtime/GC issues. Unset in normal builds and CI.
    if let Ok(extra) = std::env::var("ZAP_CC_EXTRA") {
        for a in extra.split_whitespace() {
            cmd.arg(a);
        }
    }
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run linker (cc): {e}"))?;
    let _ = std::fs::remove_file(&prog_path);
    let _ = std::fs::remove_dir_all(&build_dir);
    if !status.success() {
        return Err(format!("linker (cc) failed: {status}"));
    }
    Ok(())
}

/// The compile + link flags for embedding CPython (ticket #27, D-0012), as
/// individual `cc` arguments. Resolved via `pkg-config`: `python3-embed` is the
/// canonical module for embedding an interpreter (Python 3.8+); `python3` is the
/// fallback for older layouts. Includes both `--cflags` (so `zaprt.c` finds
/// `<Python.h>`) and `--libs` (so libpython links). The `python3-dev` and
/// `pkg-config` packages provide these (added to the dev container and CI).
pub(crate) fn python_embed_flags() -> Result<Vec<String>, String> {
    for module in ["python3-embed", "python3"] {
        let out = Command::new("pkg-config")
            .args(["--cflags", "--libs", module])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                return Ok(text.split_whitespace().map(|s| s.to_string()).collect());
            }
            Ok(_) => continue, // module not found; try the next one
            Err(e) => {
                return Err(format!(
                    "failed to run pkg-config for Python embedding flags: {e} \
                     (install `pkg-config` and `python3-dev`)"
                ))
            }
        }
    }
    Err("pkg-config found neither 'python3-embed' nor 'python3' \
         (install `python3-dev` and `pkg-config` to build programs that `import python`)"
        .to_string())
}

/// Whether a block's first instruction is a `LandingPad` (which must be the
/// block's first non-phi LLVM instruction, so no safepoint may precede it).
#[cfg(feature = "gc-immix")]
fn block_starts_with_landing_pad(b: &zir::Block) -> bool {
    matches!(b.insts.first(), Some(zir::Inst::LandingPad { .. }))
}

/// A mask of the low `width` bits set (`width` is `1..=64`), for C-style
/// bitfield extraction/insertion (I#173). Widening to the backing integer's own
/// width is done by `const_int`, which truncates this `u64` to the int type.
fn width_mask(width: u32) -> u64 {
    if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    }
}

/// The SSA value an instruction defines, if any (ticket #48: used to root every
/// GC-pointer-typed definition at its definition site; see I#167). Non-pointer
/// dests are filtered out downstream by their absent root slot.
#[cfg(feature = "gc-immix")]
fn inst_dest(i: &zir::Inst) -> Option<u32> {
    use zir::Inst::*;
    match i {
        IConst { dest, .. }
        | FConst { dest, .. }
        | BConst { dest, .. }
        | NullConst { dest, .. }
        | IsNull { dest, .. }
        | Bin { dest, .. }
        | ICmp { dest, .. }
        | FCmp { dest, .. }
        | Un { dest, .. }
        | Cast { dest, .. }
        | Alloca { dest, .. }
        | Load { dest, .. }
        | Select { dest, .. }
        | StrConst { dest, .. }
        | StructNew { dest, .. }
        | ArenaNew { dest, .. }
        | StackStruct { dest, .. }
        | StrBuild { dest, .. }
        | FieldGet { dest, .. }
        | LandingPad { dest, .. }
        | ArrayBinOp { dest, .. }
        | ArrayUnaryOp { dest, .. }
        | FuncAddr { dest, .. }
        | GlobalAddr { dest, .. }
        | ClosureMake { dest, .. }
        | IfaceMake { dest, .. }
        | AtomicLoad { dest, .. }
        | AtomicRmw { dest, .. }
        | AtomicCas { dest, .. }
        | IfaceRender { dest, .. }
        | IfaceRenderInvoke { dest, .. } => Some(*dest),
        Call { dest, .. }
        | Invoke { dest, .. }
        | CallClosure { dest, .. }
        | IfaceCall { dest, .. }
        | IfaceInvoke { dest, .. } => *dest,
        _ => None,
    }
}

/// The shared context for the vectorized broadcast kernel (#896), so its four
/// inner-axis specializations ((lhs contiguous?, rhs contiguous?)) share one
/// argument bundle rather than a 20-argument call. All values live in the
/// current function; `emit_bcast_kernel` reads them and emits one row-walking
/// loop with a vectorized inner body + scalar tail.
struct BcastKernel<'ctx> {
    fv: FunctionValue<'ctx>,
    la: PointerValue<'ctx>,
    ra: PointerValue<'ctx>,
    res: PointerValue<'ctx>,
    n: inkwell::values::IntValue<'ctx>,
    inner: inkwell::values::IntValue<'ctx>,
    lp: PointerValue<'ctx>,
    rp: PointerValue<'ctx>,
    op_ptr: PointerValue<'ctx>,
    op: zir::BinOp,
    out_float: bool,
    scalar_lhs: bool,
    out: zir::Dtype,
    out_elem: BasicTypeEnum<'ctx>,
    out_vec: VectorType<'ctx>,
    out_align: u32,
    lhs_dtype: zir::Dtype,
    lhs_elem: BasicTypeEnum<'ctx>,
    lhs_vec: VectorType<'ctx>,
    lhs_align: u32,
    rhs_dtype: zir::Dtype,
    rhs_elem: BasicTypeEnum<'ctx>,
    rhs_vec: VectorType<'ctx>,
    rhs_align: u32,
    done: BasicBlock<'ctx>,
}

struct Cg<'a, 'ctx> {
    ctx: &'ctx Context,
    module: &'a LlvmModule<'ctx>,
    builder: &'a Builder<'ctx>,
    fns: std::collections::HashMap<String, FunctionValue<'ctx>>,
    structs: Vec<inkwell::types::StructType<'ctx>>,
    /// Per-struct `inline` flag (D-0051, mirrors [`zir::StructLayout::inline`]):
    /// an inline aggregate is passed/returned **by value** (register/sret) rather
    /// than as a heap pointer. Indexed by struct id, parallel to `structs`.
    struct_inline: Vec<bool>,
    /// Per-struct "has at least one GC-pointer field" flag, indexed by struct id.
    /// A stack-promoted ([`zir::Inst::StackStruct`]) *reference* struct with
    /// pointer fields needs an aggregate-root `(base, desc)` pair so the collector
    /// traces its interior fields in place (#1081/#586); a pointer-free one needs
    /// no root at all (#839). Computed from the logical field types (a packed
    /// `value struct`, I#173, is all-bitfield and so never pointer-bearing).
    /// Read only by the precise-GC frame setup (aggregate roots), so it is dead
    /// under the Boehm build.
    #[cfg_attr(not(feature = "gc-immix"), allow(dead_code))]
    struct_has_ptr: Vec<bool>,
    /// Per-struct C-style bitfield packing (I#173, mirrors
    /// [`zir::StructLayout::packed`]). `Some` for a packed `value struct` whose
    /// LLVM type is a single `{ iN }` backing integer and whose fields are read
    /// and written by shift/mask instead of a GEP. Indexed by struct id.
    struct_packed: Vec<Option<zir::ir::PackedLayout>>,
    /// Per-struct GC type-descriptor global (ticket #48), indexed by struct id.
    /// Passed to `zap_alloc_typed` so the precise collector can trace a struct
    /// instance's pointer fields. Empty under the Boehm build.
    #[cfg_attr(not(feature = "gc-immix"), allow(dead_code))]
    struct_descs: Vec<PointerValue<'ctx>>,
    /// Module-level global slots (D-0112, #658 Phase 1), indexed by the `u32` in
    /// [`zir::ir::Inst::GlobalAddr`] / [`zir::ir::Module::globals`]. Each is an
    /// LLVM global variable (zero/null initialized) whose address a `GlobalAddr`
    /// yields; a module `const` reads it with a following `Load`. The one-time
    /// initializing stores (and, for pointer slots, `zap_gc_add_root`) are
    /// emitted by `@__zap_init_globals`, called at the top of `main`.
    globals: Vec<PointerValue<'ctx>>,
    /// The current function's shadow-stack frame (ticket #48, gc-immix). `None`
    /// under Boehm or for a function with no GC-pointer-typed values.
    #[cfg_attr(not(feature = "gc-immix"), allow(dead_code))]
    gc_frame: Option<GcFrame<'ctx>>,
    /// The target data layout, for computing GC descriptor field offsets (#48).
    #[cfg_attr(not(feature = "gc-immix"), allow(dead_code))]
    target_data: Option<TargetData>,
    /// The current function's entry block (ticket #91). Fixed-size `alloca`s are
    /// hoisted here via [`entry_alloca`] so they are reserved once per call
    /// instead of once per execution of a loop body — otherwise a long-running
    /// loop accumulates per-iteration stack slots and overflows the stack.
    entry_block: Option<BasicBlock<'ctx>>,
    /// Struct names by id (parallel to `structs`); used to name an implementor's
    /// `Struct__method` when building an interface dispatch table (D-0016).
    struct_names: Vec<String>,
    /// Structural interfaces by id (`iid`), cloned from the ZIR module (D-0016).
    /// Carries each interface's method slots (names + signatures) for building
    /// itables and dynamic-dispatch call types.
    interfaces: Vec<zir::ir::InterfaceLayout>,
    /// Display-protocol renderer per concrete type used behind an interface
    /// (#707, D-0124), cloned from the ZIR module: `concrete_sid` -> the symbol of
    /// its renderer function. Appended as the itable's *renderer slot* (past the
    /// method slots) so an `Inst::IfaceRender` dispatches to the concrete type's
    /// renderer through a type-erased interface value.
    iface_renderers: std::collections::HashMap<u32, String>,
    /// Cache of interface dispatch tables: `(concrete_sid, iid)` -> the static
    /// `[N+1 x ptr]` itable global (N method slots + one renderer slot, #707),
    /// built once per implementing pair.
    itables: std::cell::RefCell<std::collections::HashMap<(u32, u32), PointerValue<'ctx>>>,
    /// Interned string-literal constants (#837, D-0145): distinct literal bytes ->
    /// a never-freed constant-global `ZapStr` `{ i64 len, [N x i8] bytes }` built
    /// once and reused at every `Inst::StrConst` use, instead of re-allocating a
    /// fresh heap string via `zap_str_new` at each use site. Interning is
    /// transparent — `str` is a value type with no observable identity (`is` on
    /// `str` is a compile error), so this is a pure allocation optimization; the
    /// static-data pointer is non-heap so the moving collector leaves it untouched.
    str_consts: std::cell::RefCell<std::collections::HashMap<Vec<u8>, PointerValue<'ctx>>>,
    /// The C++ exception personality routine's symbol name (ticket #197, Area A3,
    /// design/0019). The runtime's `landingpad`s name it and the backend lowers
    /// unwind info against it. The *name* is the only Itanium/DWARF-specific piece
    /// of the exception pipeline: `uwtable(2)` is target-agnostic (the backend
    /// emits DWARF `.eh_frame` on ELF and SEH `.pdata`/`.xdata` on COFF from it),
    /// and the `landingpad`/`catch ptr null` representation plus the `__cxa_*`
    /// runtime glue are identical across the two unwind models. So the only
    /// per-target change is this name: `__gxx_personality_v0` (Itanium, the
    /// Linux/macOS default — byte-for-byte unchanged) vs `__gxx_personality_seh0`
    /// (the MinGW-w64 SEH model, which keeps the `landingpad` IR and libstdc++
    /// `__cxa_*` glue). Chosen once in [`Cg::new`] from the target triple.
    personality: &'static str,
    /// REPL JIT mode (I#159): when true, the shadow-stack head is reached via the
    /// `zap_shadow_head_addr()` runtime helper instead of a direct `__thread`
    /// global reference, because MCJIT cannot emit a correct dynamic-TLS access
    /// to a thread-local defined in the dlopen'd runtime. Only affects
    /// [`Cg::gc_head`]; false for the AOT object-file path.
    #[cfg_attr(not(feature = "gc-immix"), allow(dead_code))]
    jit: bool,
    /// Floating-point determinism policy (#946, D-0196): the fast-math flags
    /// (`contract`, `reassoc`) stamped onto every scalar and vector float
    /// arithmetic instruction. `Default` contracts (FMA) but never reassociates;
    /// `Strict` stamps nothing (bit-exact); `Fast` adds `reassoc`.
    fp: zir::FpPolicy,
}

/// A function's shadow-stack frame (ticket #48). Codegen reserves one root slot
/// per GC-pointer-typed SSA value plus writable storage for reference locals.
/// It links the frame onto the per-thread `zap_shadow_head` on entry, stores
/// SSA pointers at their definitions, and unlinks on return. Reference-local
/// stores and loads use their canonical root cells directly. A GC safepoint (at
/// function entry and loop headers, #568/D-0144) then finds every live pointer via
/// the chain. Over-retention of dead-but-slotted values is safe for the
/// non-moving collector.
///
/// A by-value `value struct` / tuple (D-0051) held across a safepoint lives in a
/// stack *spill* slot whose interior reference fields neither of the above cover
/// (the spill's SSA pointer addresses the stack, which the collector ignores, and
/// its alloca has a *struct* pointee, so #584's reference-local rooting skips it).
/// A trailing *aggregate-root* section therefore records `(base, desc)` pairs so
/// the collector can trace such a spill's pointer fields in place via the struct's
/// precise type descriptor — the same descriptor heap instances use (#586).
#[derive(Clone)]
#[cfg_attr(not(feature = "gc-immix"), allow(dead_code))]
struct GcFrame<'ctx> {
    /// Alloca of `{ ptr prev; i64 nroots; i64 naggr; [N x ptr] roots }` in the
    /// entry block. The `roots` array holds `n_direct` value/reference-local root
    /// slots followed by `n_aggr` `(base, desc)` aggregate-root pairs.
    frame_ptr: PointerValue<'ctx>,
    frame_ty: inkwell::types::StructType<'ctx>,
    /// value id -> root slot index, or `-1` if the value is not a GC pointer.
    slot_of: Vec<i32>,
    /// GC-reference local storage -> canonical writable root slot. Rooting an
    /// alloca's stack address does not let evacuation rewrite its contents.
    local_slot_of: Vec<i32>,
    /// value id -> aggregate-root pair index, or `-1`. Set for every by-value
    /// `value struct` / tuple SSA value; the pair's `base` word holds the spill
    /// pointer and its `desc` word the struct's type descriptor.
    aggr_of: Vec<i32>,
    /// Number of direct root slots (SSA values plus reference-local contents);
    /// the aggregate-root pairs begin at `roots[n_direct]`.
    n_direct: u32,
    /// Total length of the `roots` array: `n_direct + 2 * n_aggr` (the collector
    /// reads the pair count from the frame's `naggr` word, not from here).
    n: u32,
}

impl<'a, 'ctx> Cg<'a, 'ctx> {
    fn new(
        ctx: &'ctx Context,
        module: &'a LlvmModule<'ctx>,
        builder: &'a Builder<'ctx>,
        jit: bool,
        fp: zir::FpPolicy,
    ) -> Cg<'a, 'ctx> {
        Cg {
            ctx,
            module,
            builder,
            jit,
            fp,
            fns: std::collections::HashMap::new(),
            structs: Vec::new(),
            struct_inline: Vec::new(),
            struct_has_ptr: Vec::new(),
            struct_packed: Vec::new(),
            struct_descs: Vec::new(),
            globals: Vec::new(),
            gc_frame: None,
            target_data: None,
            entry_block: None,
            struct_names: Vec::new(),
            interfaces: Vec::new(),
            iface_renderers: std::collections::HashMap::new(),
            itables: std::cell::RefCell::new(std::collections::HashMap::new()),
            str_consts: std::cell::RefCell::new(std::collections::HashMap::new()),
            // Pick the exception personality by target (ticket #197, A3). Codegen
            // emits for the host triple only, so "windows" here means the Zap
            // compiler is running on a Windows (MinGW-w64) host — exactly when the
            // module's triple is a Windows one and the backend emits SEH tables.
            // Off Windows this is verbatim the historical `__gxx_personality_v0`,
            // so the emitted IR/asm is unchanged on Linux/macOS.
            personality: if TargetMachine::get_default_triple()
                .as_str()
                .to_string_lossy()
                .contains("windows")
            {
                "__gxx_personality_seh0"
            } else {
                "__gxx_personality_v0"
            },
        }
    }

    /// Emit a fixed-size `alloca` in the current function's **entry block**
    /// rather than at the builder's current position (ticket #91). The entry
    /// block executes exactly once per call and dominates every other block, so
    /// a slot reserved here is reused across loop iterations instead of being
    /// re-allocated each time — which otherwise overflows the stack in a
    /// long-running loop. Semantics are unchanged: only the storage location is
    /// hoisted; each logical variable is still (re)initialized by its in-body
    /// `store`s, so per-iteration *values* are unaffected.
    ///
    /// The builder is repositioned before the entry block's terminator while the
    /// `alloca` is emitted, then restored. When the entry block is still being
    /// built (no terminator yet — i.e. the request comes from within the entry
    /// block itself), the alloca is appended in place, which is already correct.
    fn entry_alloca(
        &self,
        ty: BasicTypeEnum<'ctx>,
        name: &str,
    ) -> Result<PointerValue<'ctx>, String> {
        let entry = self
            .entry_block
            .ok_or("entry_alloca called with no current function")?;
        let saved = self.builder.get_insert_block();
        match entry.get_terminator() {
            Some(term) => self.builder.position_before(&term),
            None => self.builder.position_at_end(entry),
        }
        let p = self
            .builder
            .build_alloca(ty, name)
            .map_err(|e| e.to_string())?;
        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        Ok(p)
    }

    /// Whether this build emits the precise Immix GC's metadata (ticket #48):
    /// per-object type descriptors, shadow-stack roots and safepoint polls. Off
    /// by default (the Boehm collector needs none of it).
    #[cfg(feature = "gc-immix")]
    const GC_IMMIX: bool = true;
    #[cfg(not(feature = "gc-immix"))]
    const GC_IMMIX: bool = false;

    fn translate(&mut self, m: &zir::Module) -> Result<(), String> {
        // The data layout was set on the module by the caller (from the target
        // machine); derive a TargetData for GC descriptor field offsets.
        self.target_data = Some(TargetData::create(
            self.module
                .get_data_layout()
                .as_str()
                .to_str()
                .map_err(|e| e.to_string())?,
        ));
        // printf: i32 printf(ptr, ...)
        let i32t = self.ctx.i32_type();
        let i64t = self.ctx.i64_type();
        let ptr = self.ctx.ptr_type(AddressSpace::default());
        self.module
            .add_function("printf", i32t.fn_type(&[ptr.into()], true), None);

        // Runtime string entry points (see design/RUNTIME-ABI.md).
        let str_new = self.module.add_function(
            "zap_str_new",
            ptr.fn_type(&[ptr.into(), i64t.into()], false),
            None,
        );
        self.fns.insert("zap_str_new".to_string(), str_new);
        let str_concat = self.module.add_function(
            "zap_str_concat",
            ptr.fn_type(&[ptr.into(), ptr.into()], false),
            None,
        );
        self.fns.insert("zap_str_concat".to_string(), str_concat);
        // Single-pass string build (#839; folds in the #837 lever-3(b) f-string
        // join): `zap_strbuild(ZapStr **parts, i64 n)` allocates one heap string.
        // `zap_strbuild_into(void *buf, i64 cap, ZapStr **parts, i64 n)` builds
        // into a caller-provided inline stack buffer of `cap` bytes when the
        // summed length fits, else spills to a heap `zap_strbuild`. (Named
        // `strbuild`, not `str_join`, to avoid the `str.join(list)` runtime fn.)
        let str_build = self.module.add_function(
            "zap_strbuild",
            ptr.fn_type(&[ptr.into(), i64t.into()], false),
            None,
        );
        self.fns.insert("zap_strbuild".to_string(), str_build);
        let str_build_into = self.module.add_function(
            "zap_strbuild_into",
            ptr.fn_type(&[ptr.into(), i64t.into(), ptr.into(), i64t.into()], false),
            None,
        );
        self.fns
            .insert("zap_strbuild_into".to_string(), str_build_into);
        let print_str = self.module.add_function(
            "zap_print_str",
            self.ctx.void_type().fn_type(&[ptr.into()], false),
            None,
        );
        self.fns.insert("zap_print_str".to_string(), print_str);
        // zap_alloc(size) -> ptr (zeroed); routes to the GC later (ticket #10).
        let alloc = self
            .module
            .add_function("zap_alloc", ptr.fn_type(&[i64t.into()], false), None);
        self.fns.insert("zap_alloc".to_string(), alloc);
        let str_len =
            self.module
                .add_function("zap_str_len", i64t.fn_type(&[ptr.into()], false), None);
        self.fns.insert("zap_str_len".to_string(), str_len);

        // Scope-bound arena / region allocator (#553, D-0158). Backend-uniform:
        // open(size_hint, cap) -> arena handle; alloc_bytes(arena, n) -> bytes;
        // alloc_desc(arena, desc) -> leaf struct; close(arena). A NULL alloc
        // result signals cap / heap OOM and codegen raises MemoryError.
        self.declare_rt(
            "zap_arena_open",
            &[i64t.into(), i64t.into()],
            Some(ptr.into()),
        );
        self.declare_rt(
            "zap_arena_alloc_bytes",
            &[ptr.into(), i64t.into()],
            Some(ptr.into()),
        );
        self.declare_rt(
            "zap_arena_alloc_atomic",
            &[ptr.into(), i64t.into()],
            Some(ptr.into()),
        );
        self.declare_rt("zap_arena_close", &[ptr.into()], None);
        self.declare_rt("zap_arena_oom", &[], None);

        // String surface (ticket #284, design/0007-strings.md): codepoint
        // indexing/slicing/len/iteration, char conversions, and the core method
        // set. `char` is an `i32` Unicode scalar; `str`/`list` are `ptr`.
        {
            let p = ptr;
            let i = i64t;
            let c = i32t;
            self.declare_rt("zap_str_len_cp", &[p.into()], Some(i.into()));
            self.declare_rt("zap_str_index", &[p.into(), i.into()], Some(c.into()));
            self.declare_rt("zap_str_cp_at_byte", &[p.into(), i.into()], Some(c.into()));
            self.declare_rt("zap_str_adv_byte", &[p.into(), i.into()], Some(i.into()));
            self.declare_rt(
                "zap_str_slice",
                &[
                    p.into(),
                    i.into(),
                    i.into(),
                    i.into(),
                    i.into(),
                    i.into(),
                    i.into(),
                ],
                Some(p.into()),
            );
            // `bytes` surface (ticket #337): byte-wise index/slice, construction
            // from a list of ints, and UTF-8 decode. All operate on the ZapStr
            // layout (a `ptr`); `b[i]` yields an `int64` byte.
            self.declare_rt("zap_bytes_index", &[p.into(), i.into()], Some(i.into()));
            self.declare_rt(
                "zap_bytes_slice",
                &[
                    p.into(),
                    i.into(),
                    i.into(),
                    i.into(),
                    i.into(),
                    i.into(),
                    i.into(),
                ],
                Some(p.into()),
            );
            self.declare_rt("zap_bytes_from_list", &[p.into()], Some(p.into()));
            self.declare_rt("zap_bytes_to_str", &[p.into()], Some(p.into()));
            // str(bytes, encoding[, errors]) decode (#1459).
            self.declare_rt(
                "zap_str_decode",
                &[p.into(), p.into(), p.into()],
                Some(p.into()),
            );
            // Bulk byte-table translation (ticket #1099). `maketrans(frm, to)`
            // builds a 256-byte table (raises on unequal lengths); `translate`
            // rewrites the whole buffer in one pass (raises when table != 256
            // bytes). The 4th arg of `translate` is a 0/1 `has_delete` flag; when
            // set, the 3rd arg is the delete-set `bytes`.
            self.declare_rt("zap_bytes_maketrans", &[p.into(), p.into()], Some(p.into()));
            self.declare_rt(
                "zap_bytes_translate",
                &[p.into(), p.into(), p.into(), i.into()],
                Some(p.into()),
            );
            // Display-protocol repr helpers (#274, D-0124): Python-quoted string
            // / `b'…'` bytes forms for nested (field / element) rendering.
            self.declare_rt("zap_str_repr", &[p.into()], Some(p.into()));
            self.declare_rt("zap_bytes_repr", &[p.into()], Some(p.into()));
            self.declare_rt("zap_chr", &[i.into()], Some(c.into()));
            self.declare_rt("zap_char_to_str", &[c.into()], Some(p.into()));
            self.declare_rt("zap_str_parse_int", &[p.into()], Some(i.into()));
            // int(s, base) base parse and bytes(n) zero-fill (#1386).
            self.declare_rt(
                "zap_str_parse_int_base",
                &[p.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt("zap_bytes_zeros", &[i.into()], Some(p.into()));
            self.declare_rt("zap_str_upper", &[p.into()], Some(p.into()));
            self.declare_rt("zap_str_lower", &[p.into()], Some(p.into()));
            self.declare_rt("zap_str_strip", &[p.into(), i.into()], Some(p.into()));
            self.declare_rt("zap_str_startswith", &[p.into(), p.into()], Some(i.into()));
            self.declare_rt("zap_str_endswith", &[p.into(), p.into()], Some(i.into()));
            self.declare_rt("zap_str_contains", &[p.into(), p.into()], Some(i.into()));
            // Borrowed-bytes view ABI (#1082): a view is `(backing:ptr, off:i64,
            // len:i64)`; these entry points read `backing->data` fresh and never
            // allocate or root (except `zap_strview_to_str`, the materializer).
            self.declare_rt(
                "zap_strview_off",
                &[p.into(), i.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt(
                "zap_strview_len",
                &[p.into(), i.into(), i.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt(
                "zap_strview_sub_off",
                &[p.into(), i.into(), i.into(), i.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt(
                "zap_strview_sub_len",
                &[p.into(), i.into(), i.into(), i.into(), i.into(), i.into()],
                Some(i.into()),
            );
            // bytesview creation (#1082 follow-up): byte-indexed bounds; the
            // probes / hash / eq / materializer are shared with strview.
            self.declare_rt(
                "zap_bytesview_off",
                &[p.into(), i.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt(
                "zap_bytesview_len",
                &[p.into(), i.into(), i.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt(
                "zap_bytesview_sub_off",
                &[p.into(), i.into(), i.into(), i.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt(
                "zap_bytesview_sub_len",
                &[p.into(), i.into(), i.into(), i.into(), i.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt(
                "zap_strview_startswith",
                &[p.into(), i.into(), i.into(), p.into(), i.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt(
                "zap_strview_endswith",
                &[p.into(), i.into(), i.into(), p.into(), i.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt(
                "zap_strview_contains",
                &[p.into(), i.into(), i.into(), p.into(), i.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt(
                "zap_strview_cp_count",
                &[p.into(), i.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt(
                "zap_strview_to_str",
                &[p.into(), i.into(), i.into()],
                Some(p.into()),
            );
            self.declare_rt(
                "zap_strview_hash",
                &[p.into(), i.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt(
                "zap_strview_eq_str",
                &[p.into(), i.into(), i.into(), p.into()],
                Some(i.into()),
            );
            self.declare_rt(
                "zap_strview_eq",
                &[p.into(), i.into(), i.into(), p.into(), i.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt("zap_str_find", &[p.into(), p.into()], Some(i.into()));
            self.declare_rt("zap_str_index_of", &[p.into(), p.into()], Some(i.into()));
            self.declare_rt(
                "zap_str_replace",
                &[p.into(), p.into(), p.into()],
                Some(p.into()),
            );
            self.declare_rt("zap_str_split", &[p.into(), p.into()], Some(p.into()));
            self.declare_rt("zap_str_join", &[p.into(), p.into()], Some(p.into()));
            self.declare_rt("zap_str_encode", &[p.into()], Some(p.into()));
            // Thin str method set (#971, D-0210). Split family: an `int64`
            // maxsplit trails each split variant (-1 = unlimited).
            self.declare_rt("zap_str_split_ws", &[p.into(), i.into()], Some(p.into()));
            self.declare_rt(
                "zap_str_split_sep",
                &[p.into(), p.into(), i.into()],
                Some(p.into()),
            );
            self.declare_rt("zap_str_rsplit_ws", &[p.into(), i.into()], Some(p.into()));
            self.declare_rt(
                "zap_str_rsplit_sep",
                &[p.into(), p.into(), i.into()],
                Some(p.into()),
            );
            self.declare_rt("zap_str_splitlines", &[p.into()], Some(p.into()));
            self.declare_rt("zap_str_count", &[p.into(), p.into()], Some(i.into()));
            self.declare_rt("zap_str_rfind", &[p.into(), p.into()], Some(i.into()));
            self.declare_rt("zap_str_rindex", &[p.into(), p.into()], Some(i.into()));
            self.declare_rt(
                "zap_str_removeprefix",
                &[p.into(), p.into()],
                Some(p.into()),
            );
            self.declare_rt(
                "zap_str_removesuffix",
                &[p.into(), p.into()],
                Some(p.into()),
            );
            self.declare_rt(
                "zap_str_strip_chars",
                &[p.into(), p.into(), i.into()],
                Some(p.into()),
            );
            // Pad family: (str, width:int64, fill:str) -> str; zfill drops fill.
            self.declare_rt(
                "zap_str_ljust",
                &[p.into(), i.into(), p.into()],
                Some(p.into()),
            );
            self.declare_rt(
                "zap_str_rjust",
                &[p.into(), i.into(), p.into()],
                Some(p.into()),
            );
            self.declare_rt(
                "zap_str_center",
                &[p.into(), i.into(), p.into()],
                Some(p.into()),
            );
            self.declare_rt("zap_str_zfill", &[p.into(), i.into()], Some(p.into()));
            // Case family: str -> str.
            self.declare_rt("zap_str_title", &[p.into()], Some(p.into()));
            self.declare_rt("zap_str_capitalize", &[p.into()], Some(p.into()));
            self.declare_rt("zap_str_casefold", &[p.into()], Some(p.into()));
            self.declare_rt("zap_str_swapcase", &[p.into()], Some(p.into()));
            // is* predicates: str -> int64 (0/1).
            self.declare_rt("zap_str_isdigit", &[p.into()], Some(i.into()));
            self.declare_rt("zap_str_isalpha", &[p.into()], Some(i.into()));
            self.declare_rt("zap_str_isalnum", &[p.into()], Some(i.into()));
            self.declare_rt("zap_str_isspace", &[p.into()], Some(i.into()));
            self.declare_rt("zap_str_islower", &[p.into()], Some(i.into()));
            self.declare_rt("zap_str_isupper", &[p.into()], Some(i.into()));
            // translate / format_map / expandtabs (#1034, D-0214). translate and
            // format_map take a dict[str,str] (a pointer); expandtabs an int.
            self.declare_rt("zap_str_expandtabs", &[p.into(), i.into()], Some(p.into()));
            self.declare_rt("zap_str_translate", &[p.into(), p.into()], Some(p.into()));
            self.declare_rt("zap_str_format_map", &[p.into(), p.into()], Some(p.into()));
            // partition / rpartition (#1029, D-0218): a result object (ptr) that
            // three getters unpack into a (str, str, str) tuple.
            self.declare_rt("zap_str_partition", &[p.into(), p.into()], Some(p.into()));
            self.declare_rt("zap_str_rpartition", &[p.into(), p.into()], Some(p.into()));
            self.declare_rt("zap_str_part_head", &[p.into()], Some(p.into()));
            self.declare_rt("zap_str_part_sep", &[p.into()], Some(p.into()));
            self.declare_rt("zap_str_part_tail", &[p.into()], Some(p.into()));
            let f = self.ctx.f64_type();
            // Strict float parse for `float(s)` (#282/#285), the f64 analog of
            // `zap_str_parse_int`; faults on a malformed string.
            self.declare_rt("zap_str_parse_float", &[p.into()], Some(f.into()));
            // f-string / `format()` mini-language (D-0026). All spec fields are
            // passed as i64 so the IR can emit plain integer constants; width
            // and precision use -1 for "unset".
            self.declare_rt(
                "zap_fmt_int",
                &[
                    i.into(), // value
                    i.into(), // base (2/8/10/16)
                    i.into(), // upper (0/1)
                    i.into(), // alt (0/1, '#')
                    i.into(), // sign (0='-',1='+',2=' ')
                    i.into(), // width (-1 = unset)
                    i.into(), // fill char
                    i.into(), // align ('<','>','^','=' or 0)
                    i.into(), // zero (0/1)
                ],
                Some(p.into()),
            );
            self.declare_rt(
                "zap_fmt_flt",
                &[
                    f.into(), // value
                    i.into(), // ftype ('e','f','g')
                    i.into(), // sign
                    i.into(), // width (-1 = unset)
                    i.into(), // precision (-1 = unset)
                    i.into(), // fill char
                    i.into(), // align
                    i.into(), // zero
                ],
                Some(p.into()),
            );
            self.declare_rt(
                "zap_fmt_str",
                &[
                    p.into(), // value
                    i.into(), // width (-1 = unset)
                    i.into(), // precision (-1 = unset, else truncate)
                    i.into(), // fill char
                    i.into(), // align
                ],
                Some(p.into()),
            );
        }

        // Exception / unwinding entry points (ticket #22, see
        // design/RUNTIME-ABI.md §"Exception / unwinding ABI"). `zap_panic` begins
        // unwinding; the landing-pad glue extracts the in-flight exception object
        // (`zap_eh_begin_catch`) and balances the catch (`zap_eh_end_catch`);
        // `zap_uncaught` reports an exception that reached the top-level handler.
        self.declare_rt("zap_panic", &[ptr.into()], None);
        self.declare_rt("zap_panic_typed", &[i64t.into(), ptr.into()], None);
        self.declare_rt("zap_raise", &[ptr.into()], None);
        self.declare_rt(
            "zap_exception_new",
            &[i64t.into(), ptr.into()],
            Some(ptr.into()),
        );
        self.declare_rt("zap_exception_message", &[ptr.into()], Some(ptr.into()));
        self.declare_rt(
            "zap_exception_is_a",
            &[ptr.into(), i64t.into()],
            Some(i64t.into()),
        );
        // `@exception(Parent)` family-edge registration (#1278): records a
        // user member type-id -> parent type-id edge at startup (called from
        // `@__zap_init_globals`), consulted by `zap_exception_parent`'s chain walk.
        self.declare_rt(
            "zap_exception_register_parent",
            &[i64t.into(), i64t.into()],
            None,
        );
        // Explicit `raise ... from <cause>` chaining (#604, D-0103): set the
        // cause slot before raising, and read the cause's message for the Zap
        // `<caught>.cause` attribute.
        self.declare_rt("zap_exception_set_cause", &[ptr.into(), ptr.into()], None);
        self.declare_rt(
            "zap_exception_cause_message",
            &[ptr.into()],
            Some(ptr.into()),
        );
        // Structured-field payload on a package-defined exception (#1432): attach
        // the declared `@exception` struct instance at the raise site and read it
        // back in a typed catch, so `failure.error` resolves off the struct
        // without parsing the message. The `message` slot is unaffected.
        self.declare_rt("zap_exception_set_payload", &[ptr.into(), ptr.into()], None);
        self.declare_rt("zap_exception_payload", &[ptr.into()], Some(ptr.into()));
        // `except*` ExceptionGroup split / inspect (#605, D-0129). The group
        // carries its real sub-exceptions; `remaining` yields a mutable working
        // set, `split` peels the matches of a type into a sub-group (or null),
        // `finalize_remainder` decides the auto-re-raise, and `exceptions` gives
        // the `list[str]` of sub-exception messages for `eg.exceptions`.
        self.declare_rt("zap_eg_remaining", &[ptr.into()], Some(ptr.into()));
        self.declare_rt("zap_eg_split", &[ptr.into(), i64t.into()], Some(ptr.into()));
        self.declare_rt("zap_eg_finalize_remainder", &[ptr.into()], Some(ptr.into()));
        self.declare_rt("zap_exception_exceptions", &[ptr.into()], Some(ptr.into()));
        self.declare_rt("zap_eh_begin_catch", &[ptr.into()], Some(ptr.into()));
        self.declare_rt("zap_eh_end_catch", &[], None);
        self.declare_rt("zap_uncaught", &[ptr.into()], None);
        // REPL session-binding slots (I#159, design/0025-repl.md). Each binding
        // persists across entries in a 64-bit slot; the driver assigns indices
        // and emits these per the binding's representation (integer/bool -> i64,
        // GC pointer -> ptr). A pointer store roots the slot for the collector.
        self.declare_rt("zap_repl_slot_get_i64", &[i64t.into()], Some(i64t.into()));
        self.declare_rt("zap_repl_slot_set_i64", &[i64t.into(), i64t.into()], None);
        self.declare_rt("zap_repl_slot_get_ptr", &[i64t.into()], Some(ptr.into()));
        self.declare_rt("zap_repl_slot_set_ptr", &[i64t.into(), ptr.into()], None);
        // Float bindings reuse the same 64-bit slot cell, reinterpreted as a
        // `double` in the runtime (I#252); no GC rooting (floats are not ptrs).
        let f64t = self.ctx.f64_type();
        self.declare_rt("zap_repl_slot_get_f64", &[i64t.into()], Some(f64t.into()));
        self.declare_rt("zap_repl_slot_set_f64", &[i64t.into(), f64t.into()], None);
        // The C++ exception personality routine (libstdc++/libgcc). Landing pads
        // name it; it parses the unwind tables and drives phase-1/phase-2
        // unwinding. Its exact signature is immaterial — it is only referenced by
        // name — so it is declared variadic `i32 (...)`. The name is target-chosen
        // (`self.personality`, ticket #197/A3): `__gxx_personality_v0` on
        // Linux/macOS (unchanged), `__gxx_personality_seh0` on MinGW-w64 Windows.
        let personality = self
            .module
            .add_function(self.personality, i32t.fn_type(&[], true), None);
        self.fns.insert(self.personality.to_string(), personality);

        // Collection entry points (ticket #21, see design/RUNTIME-ABI.md). Lists
        // and dicts store uniform 8-byte element words; the lowering picks the
        // `_i64` / `_ptr` / `_str` variant per element kind, so this step stays
        // a mechanical declaration of fixed C signatures.
        let p = ptr;
        let i = i64t;
        // list
        self.declare_rt("zap_list_new", &[], Some(p.into()));
        self.declare_rt("zap_list_push_i64", &[p.into(), i.into()], None);
        self.declare_rt("zap_list_push_ptr", &[p.into(), p.into()], None);
        // `list[float]` (I#569): a `double` element shares the uniform 8-byte
        // word, so the runtime reinterprets the bits around the same storage.
        self.declare_rt("zap_list_push_f64", &[p.into(), f64t.into()], None);
        self.declare_rt("zap_list_get_i64", &[p.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_list_get_ptr", &[p.into(), i.into()], Some(p.into()));
        self.declare_rt("zap_list_get_f64", &[p.into(), i.into()], Some(f64t.into()));
        self.declare_rt("zap_list_set_i64", &[p.into(), i.into(), i.into()], None);
        self.declare_rt("zap_list_set_ptr", &[p.into(), i.into(), p.into()], None);
        self.declare_rt("zap_list_set_f64", &[p.into(), i.into(), f64t.into()], None);
        self.declare_rt("zap_list_len", &[p.into()], Some(i.into()));
        // list slice `xs[lo:hi:step]` -> a NEW list (a copy, D-0030). Same bound
        // triple + present-flags as the str/bytes slice; element-type-agnostic.
        self.declare_rt(
            "zap_list_slice",
            &[
                p.into(),
                i.into(),
                i.into(),
                i.into(),
                i.into(),
                i.into(),
                i.into(),
            ],
            Some(p.into()),
        );
        // Sliced assignment `xs[lo:hi:step] = ys` (#1005): (list, rhs-list, lo,
        // hi, step, has_lo, has_hi, has_step) -> void. May raise ValueError.
        self.declare_rt(
            "zap_list_set_slice",
            &[
                p.into(),
                p.into(),
                i.into(),
                i.into(),
                i.into(),
                i.into(),
                i.into(),
                i.into(),
            ],
            None,
        );
        // list sort (ticket #336): `sorted()` copies first; `zap_list_sort_by`
        // sorts a list in place by a parallel key list (self for the no-key case).
        self.declare_rt("zap_list_copy", &[p.into()], Some(p.into()));
        self.declare_rt(
            "zap_list_sort_by",
            &[p.into(), p.into(), i.into(), i.into()],
            None,
        );
        // list element removal `del xs[i]` (#595): normalize + bounds-check the
        // index (same `IndexError` as a read) and shift the tail down one slot.
        self.declare_rt("zap_list_del_at", &[p.into(), i.into()], None);
        // list mutation methods (#977, D-0220): insert shifts a word in at an
        // index (typed like push_*); clear drops every element.
        self.declare_rt("zap_list_insert_i64", &[p.into(), i.into(), i.into()], None);
        self.declare_rt("zap_list_insert_ptr", &[p.into(), i.into(), p.into()], None);
        self.declare_rt(
            "zap_list_insert_f64",
            &[p.into(), i.into(), f64t.into()],
            None,
        );
        self.declare_rt("zap_list_clear", &[p.into()], None);
        // list.pop([i]) (#1095): remove+return the element at `i` (last when
        // `has_index == 0`), typed like `get_*`; may raise a catchable
        // IndexError, so the caller `invoke`s it. reverse() flips in place.
        self.declare_rt(
            "zap_list_pop_i64",
            &[p.into(), i.into(), i.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_list_pop_ptr",
            &[p.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_list_pop_f64",
            &[p.into(), i.into(), i.into()],
            Some(f64t.into()),
        );
        self.declare_rt("zap_list_reverse", &[p.into()], None);
        // dict
        self.declare_rt("zap_dict_new", &[], Some(p.into()));
        self.declare_rt("zap_dict_len", &[p.into()], Some(i.into()));
        self.declare_rt(
            "zap_dict_set_i64_i64",
            &[p.into(), i.into(), i.into()],
            None,
        );
        self.declare_rt(
            "zap_dict_set_i64_ptr",
            &[p.into(), i.into(), p.into()],
            None,
        );
        self.declare_rt(
            "zap_dict_set_str_i64",
            &[p.into(), p.into(), i.into()],
            None,
        );
        self.declare_rt(
            "zap_dict_set_str_ptr",
            &[p.into(), p.into(), p.into()],
            None,
        );
        self.declare_rt(
            "zap_dict_get_i64_i64",
            &[p.into(), i.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_dict_get_i64_ptr",
            &[p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_dict_get_str_i64",
            &[p.into(), p.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_dict_get_str_ptr",
            &[p.into(), p.into()],
            Some(p.into()),
        );
        // dict with struct keys (operator overloading, D-0047): the key is a
        // struct pointer hashed/compared through the `__hash__`/`__eq__` function
        // pointers passed as the trailing two arguments.
        self.declare_rt(
            "zap_dict_set_fn_i64",
            &[p.into(), p.into(), i.into(), p.into(), p.into()],
            None,
        );
        self.declare_rt(
            "zap_dict_set_fn_ptr",
            &[p.into(), p.into(), p.into(), p.into(), p.into()],
            None,
        );
        self.declare_rt(
            "zap_dict_get_fn_i64",
            &[p.into(), p.into(), p.into(), p.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_dict_get_fn_ptr",
            &[p.into(), p.into(), p.into(), p.into()],
            Some(p.into()),
        );
        // dict with `float64` VALUES (#800): the value shares the uniform word via
        // a bit reinterpret, so these push/return a `double`. One pair per key
        // kind (i64 / str / struct-`fn`); a float *key* is a separate follow-up.
        self.declare_rt(
            "zap_dict_set_i64_f64",
            &[p.into(), i.into(), f64t.into()],
            None,
        );
        self.declare_rt(
            "zap_dict_set_str_f64",
            &[p.into(), p.into(), f64t.into()],
            None,
        );
        self.declare_rt(
            "zap_dict_set_fn_f64",
            &[p.into(), p.into(), f64t.into(), p.into(), p.into()],
            None,
        );
        self.declare_rt(
            "zap_dict_get_i64_f64",
            &[p.into(), i.into()],
            Some(f64t.into()),
        );
        self.declare_rt(
            "zap_dict_get_str_f64",
            &[p.into(), p.into()],
            Some(f64t.into()),
        );
        // Borrowed-bytes probes (#1082): look up a str-keyed dict with a view
        // `(backing, byte_off, byte_len)` — no transient key alloc, no root.
        self.declare_rt(
            "zap_dict_get_strview_i64",
            &[p.into(), p.into(), i.into(), i.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_dict_get_strview_ptr",
            &[p.into(), p.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_dict_get_strview_f64",
            &[p.into(), p.into(), i.into(), i.into()],
            Some(f64t.into()),
        );
        self.declare_rt(
            "zap_dict_contains_strview",
            &[p.into(), p.into(), i.into(), i.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_dict_get_fn_f64",
            &[p.into(), p.into(), p.into(), p.into()],
            Some(f64t.into()),
        );
        // dict with `float64` KEYS (#803): the key travels as a `double`, which
        // the runtime canonicalizes (`-0.0 -> +0.0`) and NaN-rejects before
        // storing it as an i64 word (D-0138). One setter/getter per value kind
        // (i64 / ptr / f64).
        self.declare_rt(
            "zap_dict_set_f64_i64",
            &[p.into(), f64t.into(), i.into()],
            None,
        );
        self.declare_rt(
            "zap_dict_set_f64_ptr",
            &[p.into(), f64t.into(), p.into()],
            None,
        );
        self.declare_rt(
            "zap_dict_set_f64_f64",
            &[p.into(), f64t.into(), f64t.into()],
            None,
        );
        self.declare_rt(
            "zap_dict_get_f64_i64",
            &[p.into(), f64t.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_dict_get_f64_ptr",
            &[p.into(), f64t.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_dict_get_f64_f64",
            &[p.into(), f64t.into()],
            Some(f64t.into()),
        );
        // dict key removal `del d[k]` (#595): probe for the key and tombstone the
        // slot (reusing the open-addressing probe of set/get). A missing key is a
        // no-op, matching the current non-raising dict read (`KeyError` is #22).
        // The `_fn` form carries the struct key's `__hash__`/`__eq__` pointers.
        self.declare_rt("zap_dict_del_i64", &[p.into(), i.into()], None);
        self.declare_rt("zap_dict_del_str", &[p.into(), p.into()], None);
        self.declare_rt(
            "zap_dict_del_fn",
            &[p.into(), p.into(), p.into(), p.into()],
            None,
        );
        // `del d[k]` for a float-keyed dict (#803): the key is canonicalized and
        // NaN-rejected before the probe.
        self.declare_rt("zap_dict_del_f64", &[p.into(), f64t.into()], None);
        // dict enumeration / membership / shallow copy (ticket #299). Keys and
        // values are snapshotted into a fresh list (word kind picked by codegen);
        // `contains` probes for a live key; `copy` shares referenced objects.
        self.declare_rt("zap_dict_keys_i64", &[p.into()], Some(p.into()));
        self.declare_rt("zap_dict_keys_ptr", &[p.into()], Some(p.into()));
        // `.keys()` of a `dict[float64, V]` (#803) -> a `list[float64]`.
        self.declare_rt("zap_dict_keys_f64", &[p.into()], Some(p.into()));
        self.declare_rt("zap_dict_values_i64", &[p.into()], Some(p.into()));
        self.declare_rt("zap_dict_values_ptr", &[p.into()], Some(p.into()));
        // `.values()` of a `dict[K, float64]` (#800) -> a `list[float64]`.
        self.declare_rt("zap_dict_values_f64", &[p.into()], Some(p.into()));
        self.declare_rt(
            "zap_dict_contains_i64",
            &[p.into(), i.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_dict_contains_str",
            &[p.into(), p.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_dict_contains_fn",
            &[p.into(), p.into(), p.into(), p.into()],
            Some(i.into()),
        );
        // `k in d` for a float-keyed dict (#803): key canonicalized + NaN-rejected.
        self.declare_rt(
            "zap_dict_contains_f64",
            &[p.into(), f64t.into()],
            Some(i.into()),
        );
        self.declare_rt("zap_dict_copy", &[p.into()], Some(p.into()));
        // Pre-size the SoA table for ~n entries without a regrow (#1083): the
        // `dict[K,V](capacity=n)` constructor keyword and the `d.reserve(n)`
        // method both lower to this; `__ht_resize_count()` reads the cumulative
        // regrow counter the capacity goldens assert against.
        self.declare_rt("zap_dict_reserve", &[p.into(), i.into()], None);
        self.declare_rt("zap_ht_resize_count", &[], Some(i.into()));
        // set
        self.declare_rt("zap_set_new", &[], Some(p.into()));
        self.declare_rt("zap_set_reserve", &[p.into(), i.into()], None);
        self.declare_rt("zap_set_len", &[p.into()], Some(i.into()));
        self.declare_rt("zap_set_add_i64", &[p.into(), i.into()], None);
        self.declare_rt("zap_set_add_str", &[p.into(), p.into()], None);
        // `set[float64]` member (#803): the member travels as a `double`, which the
        // runtime canonicalizes (`-0.0 -> +0.0`) and NaN-rejects (D-0138).
        self.declare_rt("zap_set_add_f64", &[p.into(), f64t.into()], None);
        self.declare_rt(
            "zap_set_contains_i64",
            &[p.into(), i.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_set_contains_str",
            &[p.into(), p.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_set_contains_f64",
            &[p.into(), f64t.into()],
            Some(i.into()),
        );
        // set with struct elements (operator overloading, D-0047): the element is
        // a struct pointer hashed/compared through the `__hash__`/`__eq__`
        // function pointers passed as the trailing two arguments.
        self.declare_rt(
            "zap_set_add_fn",
            &[p.into(), p.into(), p.into(), p.into()],
            None,
        );
        self.declare_rt(
            "zap_set_contains_fn",
            &[p.into(), p.into(), p.into(), p.into()],
            Some(i.into()),
        );
        self.declare_rt("zap_set_items", &[p.into()], Some(p.into()));
        // heap (ticket #76, D-0057): a binary min-heap priority queue over a
        // growable GC array. `zap_heap_new(kind, dir)` picks the element word
        // representation (0 = i64, 1 = str, selecting the typed push/pop/peek
        // entry point) and the ordering (0 = min, 1 = max). `zap_heap_items`
        // snapshots the elements into a list in priority (root-first) order for
        // non-destructive iteration.
        self.declare_rt("zap_heap_new", &[i.into(), i.into()], Some(p.into()));
        self.declare_rt("zap_heap_len", &[p.into()], Some(i.into()));
        self.declare_rt("zap_heap_push_i64", &[p.into(), i.into()], None);
        self.declare_rt("zap_heap_push_ptr", &[p.into(), p.into()], None);
        self.declare_rt("zap_heap_pop_i64", &[p.into()], Some(i.into()));
        self.declare_rt("zap_heap_pop_ptr", &[p.into()], Some(p.into()));
        self.declare_rt("zap_heap_peek_i64", &[p.into()], Some(i.into()));
        self.declare_rt("zap_heap_peek_ptr", &[p.into()], Some(p.into()));
        // `heap[float64]` (#800): float elements ordered by a total-order
        // comparator (NaN greatest); push/pop/peek carry a reinterpreted `double`.
        self.declare_rt("zap_heap_push_f64", &[p.into(), f64t.into()], None);
        self.declare_rt("zap_heap_pop_f64", &[p.into()], Some(f64t.into()));
        self.declare_rt("zap_heap_peek_f64", &[p.into()], Some(f64t.into()));
        self.declare_rt("zap_heap_items", &[p.into()], Some(p.into()));
        // heap ergonomics (#978, D-0236): O(n) heapify from a list,
        // heapreplace/heappushpop (pop-then-push / push-then-pop), and the
        // top-/bottom-K helpers behind `heap.nlargest`/`heap.nsmallest`. The
        // keyed top-K takes parallel element + key lists (lowering evaluates the
        // `key=` closure per element) plus the key-word and element-word kinds.
        self.declare_rt(
            "zap_heap_from_list",
            &[p.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_heap_replace_i64",
            &[p.into(), i.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_heap_replace_ptr",
            &[p.into(), p.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_heap_replace_f64",
            &[p.into(), f64t.into()],
            Some(f64t.into()),
        );
        self.declare_rt(
            "zap_heap_pushpop_i64",
            &[p.into(), i.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_heap_pushpop_ptr",
            &[p.into(), p.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_heap_pushpop_f64",
            &[p.into(), f64t.into()],
            Some(f64t.into()),
        );
        self.declare_rt(
            "zap_heap_topk",
            &[p.into(), i.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_heap_topk_keyed",
            &[p.into(), p.into(), i.into(), i.into(), i.into(), i.into()],
            Some(p.into()),
        );
        // Struct-key hashing helpers (operator overloading, D-0047).
        self.declare_rt("zap_str_cmp", &[p.into(), p.into()], Some(i.into()));
        self.declare_rt("zap_str_hash", &[p.into()], Some(i.into()));
        self.declare_rt("zap_i64_hash", &[i.into()], Some(i.into()));
        self.declare_rt("zap_hash_combine", &[i.into(), i.into()], Some(i.into()));
        // Float field of a value aggregate used as a key (D-0139/#808): canonicalize
        // `-0.0`, reject `NaN`, return the canonical IEEE bits (#803 semantics).
        self.declare_rt("zap_f64_value_key_word", &[f64t.into()], Some(i.into()));

        // Numeric-array entry points (ticket #24, see design/0002-arrays.md and
        // design/RUNTIME-ABI.md §"Collection layout"). Construction, element
        // access, length, and reductions are runtime calls; the element-wise
        // ops (`Inst::ArrayBinOp`) are vectorized inline in codegen instead.
        let f = self.ctx.f64_type();
        self.declare_rt("zap_array_new", &[i.into(), i.into()], Some(p.into()));
        self.declare_rt("zap_array_len", &[p.into()], Some(i.into()));
        self.declare_rt("zap_array_check_shape", &[p.into(), p.into()], None);
        self.declare_rt("zap_array_data", &[p.into()], Some(p.into()));
        // N-D shape + multi-axis indexing (#500, D-0164). Construction builds a
        // rank-`ndim` array (`new_nd` + per-axis `set_dim`); `ndim`/`dim`/`count`
        // back `a.ndim`/`a.shape`/`a.size`; `row` is a partial index (rank-reduced
        // view); `new_like` allocates an element-wise result sharing the shape.
        self.declare_rt(
            "zap_array_new_nd",
            &[i.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt("zap_array_set_dim", &[p.into(), i.into(), i.into()], None);
        self.declare_rt("zap_array_new_like", &[p.into(), i.into()], Some(p.into()));
        self.declare_rt("zap_array_ndim", &[p.into()], Some(i.into()));
        self.declare_rt("zap_array_count", &[p.into()], Some(i.into()));
        self.declare_rt("zap_array_dim", &[p.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_array_row", &[p.into(), i.into()], Some(p.into()));
        // Broadcasting (#501, D-0165): `broadcast_check` raises ValueError on
        // incompatible shapes (emitted as a throwing call in lowering);
        // `broadcast_new` allocates the broadcast-shaped result; `same_shape`
        // selects the flat vs strided kernel; `bcast_offsets` maps a result flat
        // index to each operand's window-relative element offset (last two args
        // are `int64*` out-params, so `p`).
        self.declare_rt("zap_array_broadcast_check", &[p.into(), p.into()], None);
        self.declare_rt(
            "zap_array_broadcast_new",
            &[p.into(), p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_array_same_shape",
            &[p.into(), p.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_array_bcast_offsets",
            &[p.into(), p.into(), p.into(), i.into(), p.into(), p.into()],
            None,
        );
        // #896: operand's element step along the result's innermost axis — 0 if
        // stretched there (splat), else its contiguous stride. The vectorized
        // broadcast kernel precomputes this per operand.
        self.declare_rt(
            "zap_array_bcast_inner_stride",
            &[p.into(), p.into()],
            Some(i.into()),
        );
        // array slice `a[lo:hi]` -> a VIEW sharing the buffer (D-0030, contiguous
        // only). Bounds `(lo, hi)` + their present-flags; no step (strided views
        // are a follow-up, so codegen rejects a non-unit step before here).
        self.declare_rt(
            "zap_array_slice",
            &[p.into(), i.into(), i.into(), i.into(), i.into()],
            Some(p.into()),
        );
        // Strided views (#502, D-0171): `contig` compacts a strided view to a
        // contiguous copy (no-op when already contiguous) for the SIMD kernels;
        // `index_axis` integer-indexes one axis (dropping it) and `slice_axis`
        // slices one axis (keeping it, with a signed step) — the lowering folds a
        // multi-axis index/slice into a sequence of these.
        self.declare_rt("zap_array_contig", &[p.into()], Some(p.into()));
        self.declare_rt(
            "zap_array_index_axis",
            &[p.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_array_slice_axis",
            &[
                p.into(),
                i.into(),
                i.into(),
                i.into(),
                i.into(),
                i.into(),
                i.into(),
                i.into(),
            ],
            Some(p.into()),
        );
        // Per-axis bounds check for the full-index fast path (#894): validates
        // one axis index against that axis's length and returns it, so a full
        // integer index `m[i, j, ...]` can compute its flat offset inline
        // (Horner over the dims) without folding through an allocated row view.
        self.declare_rt(
            "zap_array_ckidx",
            &[p.into(), i.into(), i.into()],
            Some(i.into()),
        );
        self.declare_rt("zap_array_get_f64", &[p.into(), i.into()], Some(f.into()));
        self.declare_rt("zap_array_set_f64", &[p.into(), i.into(), f.into()], None);
        self.declare_rt("zap_array_get_i64", &[p.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_array_set_i64", &[p.into(), i.into(), i.into()], None);
        self.declare_rt("zap_array_sum_f64", &[p.into()], Some(f.into()));
        self.declare_rt("zap_array_sum_i64", &[p.into()], Some(i.into()));
        // uint8 / bool dtypes (ticket #49): elements cross the ABI as `i64`
        // words (the runtime truncates/normalizes on set, zero-extends on get);
        // the compact 1-byte storage is handled by the SIMD kernel directly.
        self.declare_rt("zap_array_get_u8", &[p.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_array_set_u8", &[p.into(), i.into(), i.into()], None);
        self.declare_rt("zap_array_sum_u8", &[p.into()], Some(i.into()));
        self.declare_rt("zap_array_get_bool", &[p.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_array_set_bool", &[p.into(), i.into(), i.into()], None);
        self.declare_rt("zap_array_sum_bool", &[p.into()], Some(i.into()));
        // The rest of the dtype matrix (#503 f32/i32, #890 i8/i16 + u16/u32/u64):
        // every integer dtype crosses the ABI as an `i64` word (runtime
        // sign/zero-extends on get, wraps on set), every float as an `f64` word
        // (widened on get, narrowed on set) — same prototypes as i64 / f64.
        for s in ["i32", "i16", "i8", "u16", "u32", "u64"] {
            self.declare_rt(
                &format!("zap_array_get_{s}"),
                &[p.into(), i.into()],
                Some(i.into()),
            );
            self.declare_rt(
                &format!("zap_array_set_{s}"),
                &[p.into(), i.into(), i.into()],
                None,
            );
            self.declare_rt(&format!("zap_array_sum_{s}"), &[p.into()], Some(i.into()));
        }
        self.declare_rt("zap_array_get_f32", &[p.into(), i.into()], Some(f.into()));
        self.declare_rt("zap_array_set_f32", &[p.into(), i.into(), f.into()], None);
        self.declare_rt("zap_array_sum_f32", &[p.into()], Some(f.into()));
        // complex128 arrays (#938): get/set a `complex` element (a ZapComplex
        // pointer), the element-wise helpers (array⊗array and array⊗scalar, op
        // code 0/1/2/3 = +/-/×/÷), conj/abs, and the whole-array complex sum. The
        // lowering routes every complex-array op to one of these (a complex
        // element is a 16-byte 2×f64 slot, not a scalar SIMD lane).
        self.declare_rt(
            "zap_array_get_complex",
            &[p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_array_set_complex",
            &[p.into(), i.into(), p.into()],
            None,
        );
        self.declare_rt("zap_array_sum_complex", &[p.into()], Some(p.into()));
        self.declare_rt(
            "zap_array_cbinop",
            &[p.into(), p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_array_cbinop_cs",
            &[p.into(), p.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt("zap_array_conj", &[p.into()], Some(p.into()));
        self.declare_rt("zap_array_cabs", &[p.into()], Some(p.into()));
        self.declare_rt("zap_array_cneg", &[p.into()], Some(p.into()));
        // Reductions (#507, D-0170): min/max/prod keep the dtype (an int word
        // for integer/bool dtypes, an f64 word for float dtypes); mean always
        // returns an f64 word (the lowering narrows to f32 for an f32 source).
        // The `axis=` forms return a new rank-(n-1) array pointer.
        for s in ["i64", "i32", "i16", "i8", "u8", "u16", "u32", "u64", "bool"] {
            self.declare_rt(&format!("zap_array_min_{s}"), &[p.into()], Some(i.into()));
            self.declare_rt(&format!("zap_array_max_{s}"), &[p.into()], Some(i.into()));
            self.declare_rt(&format!("zap_array_prod_{s}"), &[p.into()], Some(i.into()));
            self.declare_rt(&format!("zap_array_mean_{s}"), &[p.into()], Some(f.into()));
        }
        for s in ["f64", "f32"] {
            self.declare_rt(&format!("zap_array_min_{s}"), &[p.into()], Some(f.into()));
            self.declare_rt(&format!("zap_array_max_{s}"), &[p.into()], Some(f.into()));
            self.declare_rt(&format!("zap_array_prod_{s}"), &[p.into()], Some(f.into()));
            self.declare_rt(&format!("zap_array_mean_{s}"), &[p.into()], Some(f.into()));
        }
        self.declare_rt(
            "zap_array_reduce_axis",
            &[p.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt("zap_array_mean_axis", &[p.into(), i.into()], Some(p.into()));
        self.declare_rt("zap_array_axis_check", &[p.into(), i.into()], None);
        // Matrix multiply / dot product (#509, D-0172). `dot{i,f}` return the
        // 1-D·1-D scalar (int/float word); `matmul` returns a new rank-1 or
        // rank-2 array of the promoted `out_dtype`. All raise ValueError on a
        // shape mismatch.
        self.declare_rt("zap_array_doti", &[p.into(), p.into()], Some(i.into()));
        self.declare_rt("zap_array_dotf", &[p.into(), p.into()], Some(f.into()));
        self.declare_rt(
            "zap_array_matmul",
            &[p.into(), p.into(), i.into()],
            Some(p.into()),
        );
        // linalg (#926, D-0183): `import linalg` solve/inv/det, LAPACK-backed.
        // solve(a, b) -> x array; inv(a) -> array; det(a) -> f64. Each raises a
        // catchable ValueError on a bad shape / singular matrix, so the lowering
        // routes them through `throwing_call`.
        self.declare_rt("zap_linalg_solve", &[p.into(), p.into()], Some(p.into()));
        self.declare_rt("zap_linalg_inv", &[p.into()], Some(p.into()));
        self.declare_rt("zap_linalg_det", &[p.into()], Some(f.into()));
        // Decompositions (#927): cholesky -> L; qr -> (Q, R); svd -> (U, S, Vt).
        // Each output has its own entry point (the lowering assembles the tuple).
        self.declare_rt("zap_linalg_cholesky", &[p.into()], Some(p.into()));
        self.declare_rt("zap_linalg_qr_q", &[p.into()], Some(p.into()));
        self.declare_rt("zap_linalg_qr_r", &[p.into()], Some(p.into()));
        self.declare_rt("zap_linalg_svd_u", &[p.into()], Some(p.into()));
        self.declare_rt("zap_linalg_svd_s", &[p.into()], Some(p.into()));
        self.declare_rt("zap_linalg_svd_vt", &[p.into()], Some(p.into()));
        // Eigensolvers / least squares / norm / pinv / matrix_power (#928).
        // eigvals -> list[complex]; eigh -> (w, v); eigvalsh reuses eigh_w; lstsq
        // -> (x, residuals, rank:i64, s); norm(a, ord) -> f64; matrix_power(a, p).
        self.declare_rt("zap_linalg_eigvals", &[p.into()], Some(p.into()));
        // eig -> (w, v): eigenvalues (array[complex]) + the complex eigenvector
        // matrix, via dgeev JOBVR='V' (#928-eig).
        self.declare_rt("zap_linalg_eig_w", &[p.into()], Some(p.into()));
        self.declare_rt("zap_linalg_eig_v", &[p.into()], Some(p.into()));
        self.declare_rt("zap_linalg_eigh_w", &[p.into()], Some(p.into()));
        self.declare_rt("zap_linalg_eigh_v", &[p.into()], Some(p.into()));
        self.declare_rt("zap_linalg_lstsq_x", &[p.into(), p.into()], Some(p.into()));
        self.declare_rt(
            "zap_linalg_lstsq_resid",
            &[p.into(), p.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_linalg_lstsq_rank",
            &[p.into(), p.into()],
            Some(i.into()),
        );
        self.declare_rt("zap_linalg_lstsq_s", &[p.into(), p.into()], Some(p.into()));
        self.declare_rt("zap_linalg_norm", &[p.into(), f.into()], Some(f.into()));
        self.declare_rt("zap_linalg_pinv", &[p.into()], Some(p.into()));
        self.declare_rt(
            "zap_linalg_matrix_power",
            &[p.into(), i.into()],
            Some(p.into()),
        );
        // FFT (#929), PocketFFT-backed. The c2c transforms and rfft take
        // (array, norm); irfft takes (array, n, norm); the freq helpers take
        // (n, d). All return an array pointer (complex, except irfft/freq -> f64).
        for name in [
            "zap_fft_fft",
            "zap_fft_ifft",
            "zap_fft_fft2",
            "zap_fft_ifft2",
            "zap_fft_fftn",
            "zap_fft_ifftn",
            "zap_fft_rfft",
        ] {
            self.declare_rt(name, &[p.into(), i.into()], Some(p.into()));
        }
        self.declare_rt(
            "zap_fft_irfft",
            &[p.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt("zap_fft_fftfreq", &[i.into(), f.into()], Some(p.into()));
        self.declare_rt("zap_fft_rfftfreq", &[i.into(), f.into()], Some(p.into()));
        // Shape ops (#511, D-0174): transpose/flatten/reshape (views or copies)
        // and concatenate/stack (over a `list` of arrays). `i64buf`/`i64set`
        // build the reshape target-dims buffer.
        self.declare_rt("zap_array_transpose", &[p.into()], Some(p.into()));
        self.declare_rt("zap_array_flatten", &[p.into()], Some(p.into()));
        // Array sort / argsort (#932): in-place sort, sorted copy, argsort indices.
        self.declare_rt(
            "zap_array_sort_inplace",
            &[p.into(), i.into(), i.into()],
            None,
        );
        self.declare_rt(
            "zap_array_sorted",
            &[p.into(), i.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_array_argsort",
            &[p.into(), i.into(), i.into(), i.into()],
            Some(p.into()),
        );
        // Searching / selection (#934): searchsorted, argmin/argmax, unique,
        // partition/argpartition.
        self.declare_rt(
            "zap_array_searchsorted",
            &[p.into(), p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt("zap_array_argminmax", &[p.into(), i.into()], Some(i.into()));
        self.declare_rt(
            "zap_array_argminmax_axis",
            &[p.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt("zap_array_unique_values", &[p.into()], Some(p.into()));
        self.declare_rt("zap_array_unique_index", &[p.into()], Some(p.into()));
        self.declare_rt("zap_array_unique_inverse", &[p.into()], Some(p.into()));
        self.declare_rt("zap_array_unique_counts", &[p.into()], Some(p.into()));
        self.declare_rt(
            "zap_array_partition_inplace",
            &[p.into(), p.into(), i.into()],
            None,
        );
        self.declare_rt(
            "zap_array_partitioned",
            &[p.into(), p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_array_argpartition",
            &[p.into(), p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt("zap_i64buf", &[i.into()], Some(p.into()));
        self.declare_rt("zap_i64set", &[p.into(), i.into(), i.into()], None);
        self.declare_rt(
            "zap_array_reshape",
            &[p.into(), i.into(), p.into()],
            Some(p.into()),
        );
        self.declare_rt("zap_array_concat", &[p.into(), i.into()], Some(p.into()));
        self.declare_rt("zap_array_stack", &[p.into(), i.into()], Some(p.into()));
        // Comparison & mask combination (#510, D-0175): element-wise compare ->
        // array[bool] (array-array with broadcast, and array-scalar), the logical
        // `&`/`|`/`^` combiners and `~` over bool masks.
        self.declare_rt(
            "zap_array_cmp",
            &[p.into(), p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_array_cmp_scalar",
            &[p.into(), i.into(), f.into(), i.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_array_logical",
            &[p.into(), p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt("zap_array_lognot", &[p.into()], Some(p.into()));
        self.declare_rt("zap_array_truth_ambiguous", &[], None);
        // Boolean-mask indexing (#510, D-0175): select -> a fresh copy;
        // assignment writes back into the target's storage.
        self.declare_rt(
            "zap_array_mask_select",
            &[p.into(), p.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_array_mask_assign_scalar",
            &[p.into(), p.into(), i.into(), f.into(), i.into()],
            None,
        );
        self.declare_rt(
            "zap_array_mask_assign_array",
            &[p.into(), p.into(), p.into()],
            None,
        );
        // Fancy integer-array indexing (#510, D-0175): gather along axis 0 and
        // the matching scatter assignments.
        self.declare_rt("zap_array_take", &[p.into(), p.into()], Some(p.into()));
        self.declare_rt(
            "zap_array_scatter_scalar",
            &[p.into(), p.into(), i.into(), f.into(), i.into()],
            None,
        );
        self.declare_rt(
            "zap_array_scatter_array",
            &[p.into(), p.into(), p.into()],
            None,
        );
        // where(cond, x, y) + nonzero (#510, D-0175): 3-way broadcast select,
        // scalar->1-elem-array wrappers, and the per-axis nonzero gather that the
        // 1-arg where(cond) packs into a tuple.
        self.declare_rt("zap_array_from_scalar_i", &[i.into()], Some(p.into()));
        self.declare_rt("zap_array_from_scalar_f", &[f.into()], Some(p.into()));
        self.declare_rt(
            "zap_array_where",
            &[p.into(), p.into(), p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_array_nonzero_axis",
            &[p.into(), i.into()],
            Some(p.into()),
        );
        // General advanced indexing (#510, D-0175): multi-axis fancy/mixed
        // subscripts via a per-axis spec buffer + the list of index arrays.
        self.declare_rt(
            "zap_array_fancy",
            &[p.into(), p.into(), i.into(), p.into()],
            Some(p.into()),
        );
        // Constructors (#504, D-0167): zeros/ones/full fill a `new_nd` result via
        // `fill_i`/`fill_f` (value as an i64 or f64 ABI word); arange/linspace
        // allocate and fill a runtime-length 1-D array themselves.
        self.declare_rt("zap_array_fill_i", &[p.into(), i.into()], None);
        self.declare_rt("zap_array_fill_f", &[p.into(), f.into()], None);
        self.declare_rt(
            "zap_array_arange_i",
            &[i.into(), i.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_array_arange_f",
            &[i.into(), f.into(), f.into(), f.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_array_linspace",
            &[i.into(), f.into(), f.into(), i.into()],
            Some(p.into()),
        );

        // Scalar numeric helpers (ticket #19): integer/float power (`**`) and
        // Python floor division (`//`). The lowering emits plain `Inst::Call`s
        // to these, so codegen only declares their fixed C signatures.
        self.declare_rt("zap_ipow", &[i.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_powf", &[f.into(), f.into()], Some(f.into()));
        // Python built-in `round()` / 3-arg `pow()` helpers (#1381): modular
        // exponentiation, and banker's (ties-to-even) rounding to an int64 / a
        // float64 / an int64-with-power-of-ten.
        self.declare_rt(
            "zap_imodpow",
            &[i.into(), i.into(), i.into()],
            Some(i.into()),
        );
        self.declare_rt("zap_round_i64", &[f.into()], Some(i.into()));
        self.declare_rt("zap_round_nd", &[f.into(), i.into()], Some(f.into()));
        self.declare_rt("zap_iround_nd", &[i.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_ifloordiv", &[i.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_ffloordiv", &[f.into(), f.into()], Some(f.into()));
        self.declare_rt("zap_imod", &[i.into(), i.into()], Some(i.into()));
        // Guarded float division and shifts (#374): divide-by-zero ->
        // ZeroDivisionError, negative/out-of-range shift count -> ValueError /
        // defined "shifted out". Lowering calls these only for the non-constant
        // case (a safe literal operand stays a plain inline op).
        self.declare_rt("zap_fdiv", &[f.into(), f.into()], Some(f.into()));
        self.declare_rt("zap_ffmod", &[f.into(), f.into()], Some(f.into()));
        self.declare_rt("zap_ishl", &[i.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_ishr", &[i.into(), i.into()], Some(i.into()));
        // Unsigned `//`, `%`, `**`, and logical `>>` (#552). Same i64-word ABI
        // (operands/result are reinterpreted as uint64 inside), so a divide/
        // modulo by zero raises the same catchable ZeroDivisionError and a
        // negative `>>` count the same ValueError. `<<` reuses `zap_ishl` (the
        // bit result is sign-independent).
        self.declare_rt("zap_udiv", &[i.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_umod", &[i.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_upow", &[i.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_ushr", &[i.into(), i.into()], Some(i.into()));
        // An unsigned subscript index clamps `> INT64_MAX` to `INT64_MAX` so an
        // overflowing `uint64`/`usize` raises IndexError instead of wrapping
        // from the end (#552).
        self.declare_rt("zap_uindex", &[i.into()], Some(i.into()));

        // Complex scalars are immutable GC objects with a pointer ABI. The
        // runtime owns construction/arithmetic so both AOT and the REPL JIT use
        // the same representation and semantics.
        self.declare_rt("zap_complex_new", &[f.into(), f.into()], Some(p.into()));
        self.declare_rt("zap_complex_from_f64", &[f.into()], Some(p.into()));
        for name in [
            "zap_complex_add",
            "zap_complex_sub",
            "zap_complex_mul",
            "zap_complex_div",
            "zap_complex_pow",
        ] {
            self.declare_rt(name, &[p.into(), p.into()], Some(p.into()));
        }
        self.declare_rt("zap_complex_eq", &[p.into(), p.into()], Some(i.into()));
        self.declare_rt("zap_complex_truthy", &[p.into()], Some(i.into()));
        self.declare_rt("zap_complex_neg", &[p.into()], Some(p.into()));
        self.declare_rt("zap_complex_to_str", &[p.into()], Some(p.into()));

        self.declare_rt("zap_bigint_from_str", &[p.into()], Some(p.into()));
        self.declare_rt("zap_bigint_from_i64", &[i.into()], Some(p.into()));
        self.declare_rt("zap_bigint_from_f64", &[f.into()], Some(p.into()));
        self.declare_rt("zap_bigint_to_i64", &[p.into()], Some(i.into()));
        self.declare_rt(
            "zap_bigint_to_i32",
            &[p.into()],
            Some(self.ctx.i32_type().into()),
        );
        self.declare_rt("zap_bigint_to_f64", &[p.into()], Some(f.into()));
        self.declare_rt("zap_bigint_cmp", &[p.into(), p.into()], Some(i.into()));
        self.declare_rt("zap_bigint_neg", &[p.into()], Some(p.into()));
        for name in [
            "zap_bigint_add",
            "zap_bigint_sub",
            "zap_bigint_mul",
            "zap_bigint_floordiv",
            "zap_bigint_mod",
            "zap_bigint_pow",
        ] {
            self.declare_rt(name, &[p.into(), p.into()], Some(p.into()));
        }
        self.declare_rt("zap_bigint_truthy", &[p.into()], Some(i.into()));
        self.declare_rt("zap_bigint_to_str", &[p.into()], Some(p.into()));

        // Decorators & reflection (ticket #16, D-0036). `zap_i64_to_str` and
        // `zap_str_eq` back the `@derive(Repr)` / `@derive(Eq)` generated methods;
        // the `zap_reflect_*` entry points back the runtime-reflection surface
        // (metadata registered at startup, queried by `reflect_has`/`reflect_names`).
        self.declare_rt("zap_i64_to_str", &[i.into()], Some(p.into()));
        // Unsigned decimal rendering (#552): prints a 64-bit *unsigned* word, so
        // a `uint64` above `INT64_MAX` shows its true magnitude and no value
        // carries a phantom minus sign. Narrower unsigned values are zero-extended
        // into the `i64` word by lowering before the call.
        self.declare_rt("zap_u64_to_str", &[i.into()], Some(p.into()));
        // Representation / base-formatting built-ins (#1384). `zap_ascii_escape`
        // post-processes a `repr` string; the three base kernels render an
        // integer in the `0b`/`0o`/`0x` sign-magnitude form (base passed as a
        // compile-time i32): signed i64, unsigned u64, and a `bigint` (by limb
        // short-division).
        self.declare_rt("zap_ascii_escape", &[p.into()], Some(p.into()));
        self.declare_rt(
            "zap_int_to_base",
            &[i.into(), self.ctx.i32_type().into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_uint_to_base",
            &[i.into(), self.ctx.i32_type().into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_bigint_to_base",
            &[p.into(), self.ctx.i32_type().into()],
            Some(p.into()),
        );
        self.declare_rt("zap_str_eq", &[p.into(), p.into()], Some(i.into()));
        // tuple[T] element-wise equality (#1474): (a, b, kind) -> 0/1.
        self.declare_rt(
            "zap_vartuple_eq",
            &[p.into(), p.into(), self.ctx.i32_type().into()],
            Some(i.into()),
        );
        // tuple[T] structural hash (#1474, dict/set key): (buf, kind) -> i64.
        self.declare_rt(
            "zap_vartuple_hash",
            &[p.into(), self.ctx.i32_type().into()],
            Some(i.into()),
        );
        // tuple[T] concatenation (#1475): (a, b, kind) -> new tuple[T].
        self.declare_rt(
            "zap_vartuple_concat",
            &[p.into(), p.into(), self.ctx.i32_type().into()],
            Some(p.into()),
        );
        // Power-assert value rendering (ticket #73, D-0054).
        self.declare_rt("zap_f64_to_str", &[f.into()], Some(p.into()));
        self.declare_rt("zap_bool_to_str", &[i.into()], Some(p.into()));
        // @derive(Csv) record codec (ticket #132, D-0059): field quoting/join,
        // record splitting, and string->scalar parsing for the decode direction.
        self.declare_rt("zap_csv_quote", &[p.into()], Some(p.into()));
        self.declare_rt("zap_csv_join", &[p.into()], Some(p.into()));
        self.declare_rt("zap_csv_split", &[p.into()], Some(p.into()));
        self.declare_rt("zap_str_to_i64", &[p.into()], Some(i.into()));
        self.declare_rt("zap_str_to_f64", &[p.into()], Some(f.into()));
        self.declare_rt("zap_str_to_bool", &[p.into()], Some(i.into()));
        self.declare_rt("zap_reflect_register", &[p.into(), p.into()], None);
        self.declare_rt("zap_reflect_has", &[p.into(), p.into()], Some(i.into()));
        self.declare_rt("zap_reflect_names", &[p.into()], Some(p.into()));
        // Function/callable signature reflection (I#458, D-0089): a parallel
        // registry keyed by function name / `"Type.method"`, populated at startup
        // and queried by `reflect_params`/`reflect_sig`/`reflect_param_*`.
        self.declare_rt(
            "zap_reflect_param_register",
            &[p.into(), p.into(), p.into(), i.into(), p.into()],
            None,
        );
        self.declare_rt("zap_reflect_sig_register", &[p.into(), p.into()], None);
        self.declare_rt("zap_reflect_params", &[p.into()], Some(p.into()));
        self.declare_rt("zap_reflect_sig", &[p.into()], Some(p.into()));
        self.declare_rt(
            "zap_reflect_param_type",
            &[p.into(), p.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_reflect_param_annotations",
            &[p.into(), p.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_reflect_param_has_default",
            &[p.into(), p.into()],
            Some(i.into()),
        );

        // Regex entry points (ticket #23, D-0010), backed by PCRE2 (linked with
        // `-lpcre2-8`). `m/.../` → `zap_regex_match` (match object, null = no
        // match); `s/.../.../` → `zap_regex_subst` (new string); match-object
        // captures via `zap_regex_group{,_name}`; `zap_ptr_nonnull` backs
        // pointer truthiness (`if m:` / `!~`). Mechanical fixed C signatures.
        self.declare_rt("zap_ptr_nonnull", &[p.into()], Some(i.into()));
        self.declare_rt(
            "zap_regex_match",
            &[p.into(), p.into(), i.into()],
            Some(p.into()),
        );
        // `m/.../g` (global match) → `zap_regex_match_all` (a `list[Match]` of
        // all ordered non-overlapping matches, #1078). Same C signature as
        // `zap_regex_match`.
        self.declare_rt(
            "zap_regex_match_all",
            &[p.into(), p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_regex_subst",
            &[p.into(), p.into(), p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt("zap_regex_group", &[p.into(), i.into()], Some(p.into()));
        self.declare_rt(
            "zap_regex_group_name",
            &[p.into(), p.into()],
            Some(p.into()),
        );

        // Python interop entry points (ticket #27, D-0012,
        // design/0003-python-interop.md), backed by the embedded CPython C-API
        // (linked via `pkg-config --libs python3-embed`; see build_executable).
        // A Python value is an opaque GC-managed handle (`ptr`): `zap_py_import`
        // loads a module, `zap_py_getattr` reads an attribute, `zap_py_call`
        // calls a callable with a ZapList of argument handles, the `zap_py_from_*`
        // helpers marshal Zap scalars/str into Python, and `zap_py_to_*` /
        // `zap_py_str` marshal back. Mechanical fixed C signatures.
        self.declare_rt("zap_py_import", &[p.into()], Some(p.into()));
        self.declare_rt("zap_py_getattr", &[p.into(), p.into()], Some(p.into()));
        self.declare_rt("zap_py_call", &[p.into(), p.into()], Some(p.into()));
        self.declare_rt("zap_py_from_i64", &[i.into()], Some(p.into()));
        self.declare_rt("zap_py_from_f64", &[f.into()], Some(p.into()));
        self.declare_rt("zap_py_from_bool", &[i.into()], Some(p.into()));
        self.declare_rt("zap_py_from_str", &[p.into()], Some(p.into()));
        self.declare_rt("zap_py_to_i64", &[p.into()], Some(i.into()));
        self.declare_rt("zap_py_to_f64", &[p.into()], Some(f.into()));
        self.declare_rt("zap_py_str", &[p.into()], Some(p.into()));

        // Concurrency entry points (ticket #25, D-0013/D-0031,
        // design/0005-concurrency.md). Coroutines are backed by GC-registered OS
        // threads (`zap_spawn` takes a trampoline `ptr(ptr)` and its closure
        // `ptr`), joined by the implicit root scope at program exit
        // (`zap_rt_join_all`, emitted at `main`'s return). Channels store uniform
        // 8-byte element words; send/recv pick the `_i64` / `_ptr` variant per
        // element kind, exactly like the collection entry points. `zap_chan_new`
        // takes the buffer capacity (0 = unbuffered rendezvous). See `zaprt.c`.
        self.declare_rt("zap_spawn", &[p.into(), p.into()], None);
        self.declare_rt("zap_rt_join_all", &[], None);
        // Structured concurrency (ticket #50, D-0044/D-0045,
        // design/0005-concurrency.md): `scope:` blocks and `spawn detached`.
        // `zap_scope_enter(on_error)` opens a scope (0 = Cancel, 1 = Wait) and
        // returns its handle; `zap_spawn_scoped` enqueues a coroutine owned by a
        // scope; `zap_spawn_detached` enqueues one owned by nothing. The join
        // barrier `zap_scope_exit` awaits owned coroutines and re-raises their
        // failure (an aggregate `ExceptionGroup` for several concurrent failures,
        // so it may unwind); `zap_scope_exit_cancel` cancels + awaits them without
        // raising (the parent's own in-flight exception wins). `zap_checkpoint` is
        // an explicit cancellation point (may raise `Cancelled`).
        self.declare_rt("zap_scope_enter", &[i.into()], Some(p.into()));
        self.declare_rt("zap_scope_set_parent", &[p.into(), p.into()], None);
        // `zap_scope_timeout(scope, duration_ms, grace_ms)`: integer-millisecond
        // deadline (D-0094). Negative/overflow raises `ValueError` at runtime.
        self.declare_rt("zap_scope_timeout", &[p.into(), i.into(), i.into()], None);
        self.declare_rt("zap_scope_exit", &[p.into()], None);
        self.declare_rt("zap_scope_exit_cancel", &[p.into()], None);
        // `zap_scope_exit_cancel_exc(scope, pending)`: the early-exit/failure join
        // that also decides which exception surfaces (child failure > block-level
        // `ScopeTimeout`/`Cancelled`); returns the exception to re-raise.
        self.declare_rt(
            "zap_scope_exit_cancel_exc",
            &[p.into(), p.into()],
            Some(p.into()),
        );
        // `zap_spawn_scoped(thunk, env, scope, name)`: `name` is a static C-string
        // of the spawn target, used to identify the coroutine in `ScopeTimeout`
        // reports and the coroutine dump (D-0058).
        self.declare_rt(
            "zap_spawn_scoped",
            &[p.into(), p.into(), p.into(), p.into()],
            None,
        );
        self.declare_rt("zap_spawn_detached", &[p.into(), p.into()], None);
        self.declare_rt("zap_checkpoint", &[], None);
        self.declare_rt("zap_g_uncaught", &[p.into()], None);
        // Netpoller integration-test hook (ticket #80): runs the runtime's
        // netpoller self-test and returns the number of sub-checks passed. No
        // language I/O surface yet (sockets #79, signals #77). See `zaprt.c`.
        self.declare_rt("zap_netpoll_selftest", &[], Some(i.into()));
        // Built-in stdlib modules' runtime entry points (ticket #121): the
        // `signal`/`gc`/`runtime` `zap_*` declarations now live in per-module
        // files under `modules/`, so a new module's runtime surface is added in
        // its own file rather than here. See `modules` for the seam contract.
        modules::declare_runtime(self);
        self.declare_rt("zap_chan_new", &[i.into()], Some(p.into()));
        self.declare_rt("zap_chan_send_i64", &[p.into(), i.into()], None);
        self.declare_rt("zap_chan_send_ptr", &[p.into(), p.into()], None);
        self.declare_rt("zap_chan_recv_i64", &[p.into()], Some(i.into()));
        self.declare_rt("zap_chan_recv_ptr", &[p.into()], Some(p.into()));
        // AtomicArray[T] (#844): a fixed-capacity array of seq_cst CAS-able word
        // slots. `_new(cap, elem_is_ptr)`; reference elements use the `_ptr`
        // entry points, `int64` elements the `_i64` ones (typed like channels).
        self.declare_rt("zap_atomicarray_new", &[i.into(), i.into()], Some(p.into()));
        self.declare_rt("zap_atomicarray_len", &[p.into()], Some(i.into()));
        self.declare_rt(
            "zap_atomicarray_load_i64",
            &[p.into(), i.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_atomicarray_load_ptr",
            &[p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_atomicarray_store_i64",
            &[p.into(), i.into(), i.into()],
            None,
        );
        self.declare_rt(
            "zap_atomicarray_store_ptr",
            &[p.into(), i.into(), p.into()],
            None,
        );
        self.declare_rt(
            "zap_atomicarray_cas_i64",
            &[p.into(), i.into(), i.into(), i.into()],
            Some(i.into()),
        );
        self.declare_rt(
            "zap_atomicarray_cas_ptr",
            &[p.into(), i.into(), p.into(), p.into()],
            Some(i.into()),
        );
        // ContextVar[T] coroutine-local execution context (#1237). `_new(name,
        // default, has_default)` allocates the key object; reference values use
        // the `_ptr` entry points, `int64` values the `_i64` ones. `get` may
        // raise LookupError (routed as an `invoke` by lowering). `set` returns a
        // ContextToken; `reset` consumes one.
        self.declare_rt(
            "zap_ctxvar_new_ptr",
            &[p.into(), p.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt(
            "zap_ctxvar_new_i64",
            &[p.into(), i.into(), i.into()],
            Some(p.into()),
        );
        self.declare_rt("zap_ctxvar_get_ptr", &[p.into()], Some(p.into()));
        self.declare_rt("zap_ctxvar_get_i64", &[p.into()], Some(i.into()));
        self.declare_rt("zap_ctxvar_set_ptr", &[p.into(), p.into()], Some(p.into()));
        self.declare_rt("zap_ctxvar_set_i64", &[p.into(), i.into()], Some(p.into()));
        self.declare_rt("zap_ctxvar_reset", &[p.into()], None);
        // `channel[float64]` (#800): the element travels as a reinterpreted
        // `double` word.
        self.declare_rt("zap_chan_send_f64", &[p.into(), f64t.into()], None);
        self.declare_rt("zap_chan_recv_f64", &[p.into()], Some(f64t.into()));
        self.declare_rt("zap_chan_close", &[p.into()], None);
        // `select` (D-0031, #1002): variadic. Fixed: (uint64* outval, int64* outok,
        // int64 n, int64 has_default); then n per-case triples (void* ch, int64
        // dir, <value>) passed at each call site (fixed arity per site), so a
        // pointer send value is a direct pointer argument and stays rooted across
        // the park. Returns the chosen case index, or -1 for `default`.
        {
            let sel_ty = i.fn_type(&[p.into(), p.into(), i.into(), i.into()], true);
            let fv = self.module.add_function("zap_select", sel_ty, None);
            self.fns.insert("zap_select".to_string(), fv);
        }

        // Precise GC (ticket #48): allocation that carries a type descriptor, a
        // safepoint poll, and the per-thread shadow-stack head. Declared on both
        // builds (harmless under Boehm, which never emits calls to them) so the
        // helpers below can look them up unconditionally.
        self.declare_rt("zap_alloc_typed", &[p.into()], Some(p.into()));
        self.declare_rt("zap_safepoint", &[], None);
        if Self::GC_IMMIX {
            // __thread void *zap_shadow_head; — the shadow-stack chain head the
            // compiled frames push/pop. Thread-local so each M/coroutine and the
            // main thread have their own.
            let g = self.module.add_global(p, None, "zap_shadow_head");
            g.set_thread_local(true);
            // The shadow-head slot address is always fetched via this runtime
            // helper (`void **zap_shadow_head_addr(void)`), never by referencing
            // the `__thread` global directly: compiled coroutine code may migrate
            // OS threads at a safepoint, and a cached TLS base would then point at
            // the previous M's slot (I#223). Required by the REPL JIT too (I#159:
            // MCJIT cannot emit a dynamic-TLS access to the dlopen'd runtime's
            // `__thread` global). See `gc_head`.
            self.declare_rt("zap_shadow_head_addr", &[], Some(p.into()));
            // Inline safepoint poll (#568, D-0144; coalesced #860, D-0155). The hot
            // path no longer calls the non-inlinable `zap_safepoint` on every
            // block; it loads ONE exported runtime word and branches to the
            // out-of-line slow path only when any lane is set (see `gc_safepoint`).
            // It is a PLAIN (not thread-local) global, so both the AOT backend and
            // the REPL JIT (MCJIT) resolve it as an ordinary external symbol —
            // unlike the `__thread zap_shadow_head`, which needs the `*_addr`
            // helper. The word packs the three advisory flags the poll used to read
            // as three separate globals into three bit lanes (zaprt.c):
            //   * GC_WANTED / COLLECTING — the GC flags the slow path already
            //     checks; a collection is due / in progress.
            //   * PREEMPT — set by the monitor when any M has a pending fairness
            //     preemption, so a tight call-free loop still yields.
            // Coalescing them means the back-edge poll is a single `load volatile
            // i32`, not three (the #860 win once #857 register-promotes the loop).
            let i32t = self.ctx.i32_type();
            let g = self.module.add_global(i32t, None, "zap_safepoint_flags");
            g.set_linkage(inkwell::module::Linkage::External);
        }

        // Build LLVM struct types (fields are scalars or pointers, so no
        // recursive-by-value ordering concerns).
        for sl in &m.structs {
            let st = if let Some(pl) = &sl.packed {
                // C-style bitfield struct (I#173): the whole aggregate is a single
                // backing integer; fields are shift/mask slices of it, not LLVM
                // struct members. Wrapping the integer in a one-element struct
                // keeps it pointer-addressed like every other struct (StructNew,
                // copies, arrays, generics) while `size_of` reports the unit size.
                self.ctx.struct_type(&[self.basic(pl.backing)], false)
            } else {
                let field_tys: Vec<BasicTypeEnum> =
                    sl.fields.iter().map(|t| self.basic(*t)).collect();
                self.ctx.struct_type(&field_tys, false)
            };
            self.structs.push(st);
            self.struct_inline.push(sl.inline);
            // A packed value struct (I#173) is a single backing integer with no
            // pointer fields; any other struct is pointer-bearing iff a logical
            // field is a GC pointer (#1081).
            self.struct_has_ptr
                .push(sl.packed.is_none() && sl.fields.iter().any(|t| t.is_ptr()));
            self.struct_packed.push(sl.packed.clone());
            self.struct_names.push(sl.name.clone());
        }
        // Interface metadata (D-0016) for fat-pointer construction and dynamic
        // dispatch (`Inst::IfaceMake` / `IfaceCall` / `IfaceInvoke`).
        self.interfaces = m.interfaces.clone();
        // Per-concrete-type Display renderers for interface-value rendering
        // (#707, D-0124): `Inst::IfaceRender` dispatches through the itable's
        // renderer slot, which `itable_global` fills from this map.
        self.iface_renderers = m.iface_renderers.clone();

        // Emit a GC type descriptor per struct (ticket #48): kind PRECISE with
        // the byte offsets of the pointer fields, computed from the struct's
        // LLVM layout. Only under the precise-GC build.
        if Self::GC_IMMIX {
            for sid in 0..m.structs.len() {
                let d = self.emit_struct_desc(sid as u32, &m.structs[sid]);
                self.struct_descs.push(d);
            }
        }

        // Module-level global slots (D-0112, #658 Phase 1): one LLVM global per
        // ZIR global, zero/null initialized, internal linkage (whole-program AOT
        // module). A `GlobalAddr` yields the slot's address; `@__zap_init_globals`
        // fills it once before `main`. A pointer slot in the data/BSS segment is
        // scanned by the conservative Boehm collector as-is; under the precise
        // Immix collector `@__zap_init_globals` also registers it via
        // `zap_gc_add_root`, so the referent stays live either way.
        if !m.globals.is_empty() {
            // `void zap_gc_add_root(void **slot)` — registers a permanent root
            // (a no-op macro under the conservative Boehm build; a real
            // registration under precise Immix). `@__zap_init_globals` calls it
            // for each pointer-typed global.
            let ptr = self.ctx.ptr_type(AddressSpace::default());
            self.declare_rt("zap_gc_add_root", &[ptr.into()], None);
            // `void zap_gc_write_barrier(void **slot)` — a post-write barrier on
            // a mutable module-global pointer slot (D-0112 #663). Reassigning a
            // permanent-root slot installs a new old→new edge; a future
            // generational/incremental Immix cycle records it here. A linkable
            // runtime no-op today (stop-the-world Immix re-scans every root; the
            // conservative Boehm build is an exported no-op). `@__zap_init_globals`
            // never calls it — the one-time init store needs no barrier.
            self.declare_rt("zap_gc_write_barrier", &[ptr.into()], None);
        }
        // Runtime data-race detector hooks (`--race`, #677, epic #665, D-0114):
        // `void __zap_race_read/__zap_race_write(void *slot, i64 name_id, i64 line,
        // i64 col)`. Lowering emits a call at every instrumented shared-global
        // access (the ZAP5125 set) only under `--race`, so the externs are
        // declared ONLY when the ZIR actually contains such a call. A non-`--race`
        // build emits no race calls, so this adds nothing and the module stays
        // byte-identical (golden-safe).
        // The happens-before engine (#678) adds two more generated-code entry
        // points to slice 1's read/write hooks: `__zap_race_enable` (arm the
        // detector) and `__zap_race_register_global` (record a slot's name for
        // reporting), both emitted into `main` under `--race`. The spawn and
        // `Mutex` release/acquire edges are wired entirely inside the runtime, so
        // they need no LLVM declaration here. All race entry points carry the
        // `__zap_race_` prefix, so a single prefix test gates the declarations.
        let needs_race = m.funcs.iter().flat_map(|f| &f.blocks).any(|b| {
            b.insts.iter().any(|i| {
                matches!(
                    i,
                    Inst::Call { func, .. } if func.as_str().starts_with("__zap_race_")
                )
            })
        });
        if needs_race {
            let ptr = self.ctx.ptr_type(AddressSpace::default());
            let i64t = self.ctx.i64_type();
            self.declare_rt(
                "__zap_race_read",
                &[ptr.into(), i64t.into(), i64t.into(), i64t.into()],
                None,
            );
            self.declare_rt(
                "__zap_race_write",
                &[ptr.into(), i64t.into(), i64t.into(), i64t.into()],
                None,
            );
            self.declare_rt("__zap_race_enable", &[], None);
            self.declare_rt(
                "__zap_race_register_global",
                &[i64t.into(), ptr.into()],
                None,
            );
            // Slice 4 (#684): heap field/element access hooks,
            // `void __zap_race_{read,write}_field(void *base, i64 sub,
            // ZapStr *name, i64 line, i64 col)` — `base` is the stable heap
            // object header, `sub` the field/element index, `name` a static
            // descriptor string. Emitted by lowering at struct-field and
            // list-element access sites under `--race` only, so (like the
            // module-global hooks) these externs appear only when the ZIR
            // contains such a call, keeping a non-`--race` build byte-identical.
            self.declare_rt(
                "__zap_race_read_field",
                &[
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                    i64t.into(),
                ],
                None,
            );
            self.declare_rt(
                "__zap_race_write_field",
                &[
                    ptr.into(),
                    i64t.into(),
                    ptr.into(),
                    i64t.into(),
                    i64t.into(),
                ],
                None,
            );
        }
        for (i, g) in m.globals.iter().enumerate() {
            let _ = &g.name;
            let llty = self.basic(g.ty);
            let gv = self
                .module
                .add_global(llty, None, &format!("zap_global_{i}"));
            gv.set_initializer(&llty.const_zero());
            gv.set_linkage(inkwell::module::Linkage::Internal);
            self.globals.push(gv.as_pointer_value());
        }

        // Functions whose address is taken (`Inst::FuncAddr`) are handed to the
        // collection runtime as struct-key `__hash__`/`__eq__` pointers (D-0047)
        // and called through a fixed C pointer ABI (`i64(ptr)` / `i1(ptr, ptr)`).
        // So such a function must keep the plain pointer ABI for every parameter
        // and its return — never the by-value aggregate convention (D-0051) —
        // or the indirect call would not match. Direct ZIR calls stay consistent
        // because codegen keys argument materialization off the callee signature.
        let addr_taken: std::collections::HashSet<&str> = m
            .funcs
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                Inst::FuncAddr { func, .. } => Some(func.as_str()),
                _ => None,
            })
            .collect();

        // Declare all functions first (so calls resolve).
        for f in &m.funcs {
            // Inline value structs / tuples are passed and returned by value
            // (register/sret, D-0051); everything else is a scalar or a pointer.
            // A method receiver (`self`) stays by-reference even when its type is
            // inline (`Func::self_param`), so a mutating method sees the caller's
            // instance (Swift-style).
            let ptr_abi = addr_taken.contains(f.name.as_str());
            let params: Vec<BasicMetadataTypeEnum> = f
                .params
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    if ptr_abi || (f.self_param && i == 0) {
                        self.basic(*t).into()
                    } else {
                        self.abi_ty(*t).into()
                    }
                })
                .collect();
            let fnty = if f.is_main {
                i32t.fn_type(&[], false)
            } else {
                match f.ret {
                    Ty::Void => self.ctx.void_type().fn_type(&params, false),
                    rt if ptr_abi => self.basic(rt).fn_type(&params, false),
                    rt => self.abi_ty(rt).fn_type(&params, false),
                }
            };
            let fv = self.module.add_function(&f.name, fnty, None);
            // REPL JIT (I#252): a session re-emits its accumulated `def`/`type`
            // declarations into every entry module so each entry type-checks and
            // links against them. Those re-emitted definitions would collide as
            // duplicate external symbols across the MCJIT's accumulated modules,
            // so give every function internal linkage except the symbols the
            // engine looks up by name — `main` (whole-program smoke tests) and
            // the per-entry `__repl_entry_N` thunk. Each module then carries its
            // own private copy, callable within that module. No effect on the
            // AOT path (`jit` is false there).
            if self.jit && !f.is_main && !f.name.starts_with("__repl_entry_") {
                fv.set_linkage(inkwell::module::Linkage::Internal);
            }
            // `@multiversion` clone (#944, epic #139): stamp the tier's LLVM
            // `target-features` so the backend may use the wider ISA in this
            // clone, and mark it `noinline` so the dispatched call survives to
            // runtime (rule #3: a multiversioned fn is an inlining barrier). Only
            // for a tier of the TARGET arch — a foreign-arch tier (an `avx2`
            // clone in an aarch64 build) can never be selected, so it stays a
            // plain baseline copy rather than carrying an invalid feature. The
            // resolver/dispatcher themselves never get features (rule #4).
            if let Some(tier) = &f.target_feature {
                self.apply_tier_features(fv, tier);
            }
            self.fns.insert(f.name.clone(), fv);
        }

        // Declare the dispatcher symbol for every `@multiversion` group BEFORE
        // translating bodies, so a caller's `Inst::Call{func: public_name}`
        // resolves to it (lowering emits no `Func` under the public name — codegen
        // owns the dispatcher). Bodies are filled in after the clones translate.
        self.declare_multiversion_dispatchers(m);

        for f in &m.funcs {
            self.translate_fn(f)?;
        }
        self.emit_multiversion_dispatchers(m)?;
        Ok(())
    }

    /// Stamp a `@multiversion` clone's LLVM `target-features` + `noinline` for
    /// tier `tier`, when `tier` belongs to the target arch (#944, epic #139). A
    /// baseline tier (`sse2`/`neon`/`default`) adds no feature but still gets
    /// `noinline`; a foreign-arch tier is left completely baseline.
    fn apply_tier_features(&self, fv: FunctionValue<'ctx>, tier: &str) {
        let noinline = Attribute::get_named_enum_kind_id("noinline");
        if noinline != 0 {
            fv.add_attribute(
                AttributeLoc::Function,
                self.ctx.create_enum_attribute(noinline, 0),
            );
        }
        if tier == "default" {
            return;
        }
        let triple = self.module.get_triple();
        let target_arch = cpu_features::arch_of_triple(&triple.as_str().to_string_lossy());
        if let Some(spec) = cpu_features::tier_spec(tier) {
            if Some(spec.arch) == target_arch && !spec.llvm_features.is_empty() {
                let attr = self
                    .ctx
                    .create_string_attribute("target-features", spec.llvm_features);
                fv.add_attribute(AttributeLoc::Function, attr);
                // Mark this function so the LTO `target-features` strip keeps it
                // (#963, D-0200). A `@multiversion` clone is kept by its `$mv$`
                // name, but a standalone `@target_feature` user function has an
                // ordinary name; without this marker its features would be
                // stripped and the function silently demoted to baseline (the
                // #845 broken-LTO failure mode). The strip removes the marker.
                let keep = self
                    .ctx
                    .create_string_attribute("zap-keep-target-features", "");
                fv.add_attribute(AttributeLoc::Function, keep);
            }
        }
    }

    /// Declare the public dispatcher symbol + the cached-pointer global for every
    /// `@multiversion` group (#944, epic #139), BEFORE bodies translate, so a
    /// caller's `Inst::Call{func: public_name}` resolves to the dispatcher
    /// (lowering emits no `Func` under that name). The signature is copied from a
    /// clone — they all share it. Also declares the `zap_cpu_has` runtime query
    /// the resolver uses.
    fn declare_multiversion_dispatchers(&mut self, m: &zir::Module) {
        if m.multiversion_groups.is_empty() {
            return;
        }
        let ptr = self.ctx.ptr_type(AddressSpace::default());
        if !self.fns.contains_key("zap_cpu_has") {
            // int zap_cpu_has(unsigned idx) — the #943 index-based query (cached,
            // malloc-free). i32(i32) matches the C ABI.
            let i32t = self.ctx.i32_type();
            let f =
                self.module
                    .add_function("zap_cpu_has", i32t.fn_type(&[i32t.into()], false), None);
            self.fns.insert("zap_cpu_has".to_string(), f);
        }
        for g in &m.multiversion_groups {
            let Some(sig_fn) = g
                .tiers
                .iter()
                .find_map(|t| self.fns.get(&t.clone_name).copied())
            else {
                continue; // defensive: a group with no emitted clone
            };
            let disp = self
                .module
                .add_function(&g.public_name, sig_fn.get_type(), None);
            // The dispatcher is a plain forwarding stub, but a clone can raise a
            // Zap exception that must unwind THROUGH it; give it an unwind table
            // like every ordinary function (the #845 / LTO lesson — a frame
            // without CFI drops the landing pad and terminates). It carries no
            // `target-features` (rule #4: resolver/dispatcher stay baseline).
            let uwtable = Attribute::get_named_enum_kind_id("uwtable");
            if uwtable != 0 {
                disp.add_attribute(
                    AttributeLoc::Function,
                    self.ctx.create_enum_attribute(uwtable, 2),
                );
            }
            self.fns.insert(g.public_name.clone(), disp);
            let gv = self
                .module
                .add_global(ptr, None, &mv_selected_global(&g.public_name));
            gv.set_initializer(&ptr.const_null());
            gv.set_linkage(inkwell::module::Linkage::Internal);
        }
    }

    /// Fill in each `@multiversion` dispatcher body (#944, epic #139): the
    /// pointer-with-once-init pattern (#920 §6a). On the first call the inline
    /// resolver reads `cpu_features` (`zap_cpu_has`) and `select`s the
    /// highest-priority clone the host supports, falling back to the mandatory
    /// `default`; it caches the choice in the group's global so every later call
    /// is one load + one indirect call. The resolver/dispatcher stay baseline —
    /// only the clone bodies carry `target-features` (rule #4), and the clones
    /// are `noinline` so the dispatched call survives to runtime (rule #3).
    fn emit_multiversion_dispatchers(&mut self, m: &zir::Module) -> Result<(), String> {
        if m.multiversion_groups.is_empty() {
            return Ok(());
        }
        let ptr = self.ctx.ptr_type(AddressSpace::default());
        let i32t = self.ctx.i32_type();
        let triple = self.module.get_triple();
        let target_arch = cpu_features::arch_of_triple(&triple.as_str().to_string_lossy());
        let zchf = self.fns["zap_cpu_has"];
        let b = self.builder;
        for g in &m.multiversion_groups {
            let Some(&disp) = self.fns.get(&g.public_name) else {
                continue;
            };
            let selected = self
                .module
                .get_global(&mv_selected_global(&g.public_name))
                .ok_or_else(|| format!("missing @multiversion global for {}", g.public_name))?;
            let sel_slot = selected.as_pointer_value();
            let default_tier =
                g.tiers.iter().find(|t| t.is_default).ok_or_else(|| {
                    format!("@multiversion group {} has no default", g.public_name)
                })?;
            let default_addr = self.fns[&default_tier.clone_name]
                .as_global_value()
                .as_pointer_value();

            let entry = self.ctx.append_basic_block(disp, "entry");
            let resolve = self.ctx.append_basic_block(disp, "resolve");
            let call = self.ctx.append_basic_block(disp, "call");

            // entry: load the cached pointer; branch to resolve on first call.
            b.position_at_end(entry);
            let cur = b
                .build_load(ptr, sel_slot, "mv.cur")
                .map_err(|e| e.to_string())?
                .into_pointer_value();
            let is_null = b
                .build_is_null(cur, "mv.unset")
                .map_err(|e| e.to_string())?;
            b.build_conditional_branch(is_null, resolve, call)
                .map_err(|e| e.to_string())?;

            // resolve: select the best supported clone, then cache it. Tiers are
            // in priority order (highest first); applying them in REVERSE makes
            // the highest-priority match the last `select`, so it wins. Foreign-
            // arch tiers can never be selected (their names are not this arch's),
            // so they are skipped entirely.
            b.position_at_end(resolve);
            let mut sel = default_addr;
            let applicable: Vec<&zir::MvTier> = g
                .tiers
                .iter()
                .filter(|t| !t.is_default)
                .filter(|t| {
                    cpu_features::tier_spec(&t.tier).map(|s| Some(s.arch) == target_arch)
                        == Some(true)
                })
                .collect();
            for t in applicable.iter().rev() {
                let spec = cpu_features::tier_spec(&t.tier).expect("applicable tier has a spec");
                let idx = i32t.const_int(spec.idx as u64, false);
                let has = b
                    .build_call(zchf, &[idx.into()], "mv.has")
                    .map_err(|e| e.to_string())?
                    .try_as_basic_value()
                    .left()
                    .ok_or("zap_cpu_has returned void")?
                    .into_int_value();
                let hit = b
                    .build_int_compare(IntPredicate::NE, has, i32t.const_zero(), "mv.hit")
                    .map_err(|e| e.to_string())?;
                let clone_addr = self.fns[&t.clone_name].as_global_value().as_pointer_value();
                sel = b
                    .build_select(hit, clone_addr, sel, "mv.sel")
                    .map_err(|e| e.to_string())?
                    .into_pointer_value();
            }
            b.build_store(sel_slot, sel).map_err(|e| e.to_string())?;
            b.build_unconditional_branch(call)
                .map_err(|e| e.to_string())?;

            // call: indirect-call the chosen clone with the forwarded args.
            b.position_at_end(call);
            let fp = b.build_phi(ptr, "mv.fp").map_err(|e| e.to_string())?;
            fp.add_incoming(&[
                (&cur as &dyn BasicValue, entry),
                (&sel as &dyn BasicValue, resolve),
            ]);
            let fp = fp.as_basic_value().into_pointer_value();
            let args: Vec<BasicMetadataValueEnum> =
                disp.get_param_iter().map(|p| p.into()).collect();
            let cs = b
                .build_indirect_call(disp.get_type(), fp, &args, "mv.dispatch")
                .map_err(|e| e.to_string())?;
            match cs.try_as_basic_value().left() {
                Some(v) => b.build_return(Some(&v)).map_err(|e| e.to_string())?,
                None => b.build_return(None).map_err(|e| e.to_string())?,
            };
        }
        Ok(())
    }

    /// Declare a runtime entry point (`ret = None` means `void`) and remember it
    /// under `name` for `Inst::Call` resolution.
    fn declare_rt(
        &mut self,
        name: &str,
        params: &[BasicMetadataTypeEnum<'ctx>],
        ret: Option<BasicTypeEnum<'ctx>>,
    ) {
        let fnty = match ret {
            Some(t) => t.fn_type(params, false),
            None => self.ctx.void_type().fn_type(params, false),
        };
        let fv = self.module.add_function(name, fnty, None);
        self.fns.insert(name.to_string(), fv);
    }

    // --- precise GC (ticket #48) codegen helpers (gc-immix only) ---

    /// The LLVM layout of the runtime `ZapTypeDesc`:
    /// `{ i64 size; i32 kind; i32 n_ptrs; ptr ptr_offsets }`.
    #[cfg(feature = "gc-immix")]
    fn td_type(&self) -> inkwell::types::StructType<'ctx> {
        let i64t = self.ctx.i64_type();
        let i32t = self.ctx.i32_type();
        let p = self.ctx.ptr_type(AddressSpace::default());
        self.ctx
            .struct_type(&[i64t.into(), i32t.into(), i32t.into(), p.into()], false)
    }

    /// Emit a PRECISE `ZapTypeDesc` global for the LLVM struct `st`: payload size
    /// `td.get_abi_size(st)`, and the byte offsets of the fields flagged pointer
    /// in `field_is_ptr` (computed from the struct's real LLVM layout). Returns a
    /// pointer to the descriptor. Used for user structs and spawn closures.
    #[cfg(feature = "gc-immix")]
    fn emit_precise_desc(
        &self,
        st: inkwell::types::StructType<'ctx>,
        field_is_ptr: &[bool],
        name: &str,
    ) -> PointerValue<'ctx> {
        let td = self.target_data.as_ref().expect("target data set");
        let i32t = self.ctx.i32_type();
        let i64t = self.ctx.i64_type();
        let p = self.ctx.ptr_type(AddressSpace::default());
        let offsets: Vec<inkwell::values::IntValue> = field_is_ptr
            .iter()
            .enumerate()
            .filter(|(_, &is_ptr)| is_ptr)
            .map(|(i, _)| {
                let off = td.offset_of_element(&st, i as u32).unwrap_or(0);
                i32t.const_int(off, false)
            })
            .collect();
        let n = offsets.len();
        let offsets_ptr = if n == 0 {
            p.const_null()
        } else {
            let arr = i32t.const_array(&offsets);
            let g = self
                .module
                .add_global(arr.get_type(), None, &format!("{name}.offs"));
            g.set_initializer(&arr);
            g.set_constant(true);
            g.set_linkage(inkwell::module::Linkage::Internal);
            g.as_pointer_value()
        };
        let size = td.get_abi_size(&st);
        let desc = self.td_type().const_named_struct(&[
            i64t.const_int(size, false).into(),
            i32t.const_int(0, false).into(), // ZAP_TD_PRECISE
            i32t.const_int(n as u64, false).into(),
            offsets_ptr.into(),
        ]);
        let g = self.module.add_global(self.td_type(), None, name);
        g.set_initializer(&desc);
        g.set_constant(true);
        g.set_linkage(inkwell::module::Linkage::Internal);
        g.as_pointer_value()
    }

    /// Emit the descriptor for struct id `sid`.
    #[cfg(feature = "gc-immix")]
    fn emit_struct_desc(&self, sid: u32, sl: &zir::ir::StructLayout) -> PointerValue<'ctx> {
        let flags: Vec<bool> = sl.fields.iter().map(|t| t.is_ptr()).collect();
        self.emit_precise_desc(
            self.structs[sid as usize],
            &flags,
            &format!("zap.desc.struct.{sid}"),
        )
    }

    #[cfg(not(feature = "gc-immix"))]
    fn emit_struct_desc(&self, _sid: u32, _sl: &zir::ir::StructLayout) -> PointerValue<'ctx> {
        unreachable!("emit_struct_desc is only called under the gc-immix build")
    }

    /// Emit a GC/preemption safepoint poll — the only place a collection can run
    /// and the landing point for async preemption (#568, D-0144).
    ///
    /// The previous emission (FORK 4a) placed an unconditional, non-inlinable
    /// `call zap_safepoint` at the top of *every* basic block. An external
    /// language review (#568) measured that this dominates even GC-free
    /// integer-only functions: recursive `fib` paid three such calls per call,
    /// each re-reading thread-local state inside the runtime. The poll is now an
    /// INLINED flag check — load the three exported runtime flags and branch to
    /// the out-of-line `zap_safepoint` slow path only when one is set. The three
    /// advisory flags are three bit lanes of ONE exported word (#860, D-0155), so
    /// the poll is a SINGLE volatile load, not three:
    ///
    /// ```text
    ///   %f = load volatile i32, ptr @zap_safepoint_flags
    ///   %need = icmp ne i32 %f, 0                 ; hinted false
    ///   br %need, label %sp.slow, label %sp.cont
    /// sp.slow:                                    ; cold
    ///   call void @zap_safepoint()
    ///   br label %sp.cont
    /// sp.cont:
    /// ```
    ///
    /// The load is `volatile` so LICM (and #857's `-O2` pipeline) cannot hoist it
    /// out of a loop — a back-edge poll must re-read the word every iteration. Any
    /// lane set (GC due/in-progress, or a pending preemption) takes the slow path,
    /// which re-checks the precise per-M / per-collection state exactly as before;
    /// the coalesce changed only the flag encoding, not when a safepoint is taken.
    /// The slow path keeps
    /// the exact semantics of the old call: it re-checks precisely, drives the
    /// collection or the preemption yield, and parks. Splitting the current block
    /// in two is why this takes `fv`: it appends the `sp.slow`/`sp.cont` blocks
    /// and leaves the builder positioned at `sp.cont`, so the caller's remaining
    /// instructions land after the poll. No-op under Boehm (never emitted).
    #[cfg(feature = "gc-immix")]
    fn gc_safepoint(&self, fv: FunctionValue<'ctx>) -> Result<(), String> {
        let b = self.builder;
        let i32t = self.ctx.i32_type();
        // ONE volatile load of the coalesced flag word (#860, D-0155) — down from
        // three per-flag loads. `volatile` so the back-edge poll re-reads it every
        // iteration (neither LICM nor #857's `-O2` can hoist it out of the loop).
        let name = "zap_safepoint_flags";
        let g = self
            .module
            .get_global(name)
            .ok_or_else(|| format!("safepoint poll flag `{name}` not declared"))?;
        let v = b
            .build_load(i32t, g.as_pointer_value(), name)
            .map_err(|e| e.to_string())?;
        v.as_instruction_value()
            .expect("load is an instruction")
            .set_volatile(true)
            .map_err(|_| "set volatile on poll load".to_string())?;
        let flags = v.into_int_value();
        let need = b
            .build_int_compare(IntPredicate::NE, flags, i32t.const_zero(), "sp.need")
            .map_err(|e| e.to_string())?;
        // Hint "no safepoint" as the common case so the backend lays the call out
        // cold (predicted not-taken), keeping the hot path a load + branch.
        let need = self.expect_false(need)?;
        let slow = self.ctx.append_basic_block(fv, "sp.slow");
        let cont = self.ctx.append_basic_block(fv, "sp.cont");
        b.build_conditional_branch(need, slow, cont)
            .map_err(|e| e.to_string())?;
        b.position_at_end(slow);
        let sp = self.fns["zap_safepoint"];
        b.build_call(sp, &[], "").map_err(|e| e.to_string())?;
        b.build_unconditional_branch(cont)
            .map_err(|e| e.to_string())?;
        b.position_at_end(cont);
        Ok(())
    }

    /// Wrap `cond` in `@llvm.expect.i1(cond, false)` so the backend treats the
    /// taken branch as unlikely. Returns `cond` unchanged if the intrinsic is
    /// unavailable (keeps codegen correct, only loses the layout hint).
    #[cfg(feature = "gc-immix")]
    fn expect_false(
        &self,
        cond: inkwell::values::IntValue<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        let Some(expect) = inkwell::intrinsics::Intrinsic::find("llvm.expect") else {
            return Ok(cond);
        };
        let i1 = self.ctx.bool_type();
        let Some(decl) = expect.get_declaration(self.module, &[i1.into()]) else {
            return Ok(cond);
        };
        let hinted = self
            .builder
            .build_call(decl, &[cond.into(), i1.const_zero().into()], "sp.expect")
            .map_err(|e| e.to_string())?;
        Ok(hinted
            .try_as_basic_value()
            .left()
            .expect("llvm.expect returns i1")
            .into_int_value())
    }

    /// The basic blocks that must emit a safepoint poll (#568, gc-immix): the
    /// function entry (so every call chain polls — recursion has no loop
    /// back-edge of its own) plus every loop header, found as the target of a DFS
    /// back-edge. Every cycle in the CFG contains at least one DFS back-edge whose
    /// target is on the cycle, so placing a poll at each such target guarantees a
    /// poll at least once per iteration of every loop — including irreducible
    /// control flow — which preserves cooperative stop-the-world liveness and
    /// preemption for call-free loops. Straight-line blocks that are neither the
    /// entry nor a loop header no longer poll (the old FORK-4a emission polled
    /// every block), which is the redundancy #568 removes.
    #[cfg(feature = "gc-immix")]
    fn safepoint_blocks(f: &Func) -> Vec<bool> {
        let n = f.blocks.len();
        let mut is_sp = vec![false; n];
        if n == 0 {
            return is_sp;
        }
        is_sp[0] = true;
        let succs = |i: usize| -> Vec<usize> {
            match &f.blocks[i].term {
                Term::Br(t) => vec![*t as usize],
                Term::BrIf {
                    then_bb, else_bb, ..
                } => vec![*then_bb as usize, *else_bb as usize],
                Term::Ret(_) | Term::Unreachable => Vec::new(),
            }
        };
        // Iterative DFS (0 = white, 1 = gray/on-stack, 2 = black). A successor
        // edge to a gray node is a back-edge; its target is a loop header. Visit
        // from every node so disconnected/unreachable blocks are classified too.
        let mut color = vec![0u8; n];
        let mut stack: Vec<(usize, usize)> = Vec::new();
        for start in 0..n {
            if color[start] != 0 {
                continue;
            }
            color[start] = 1;
            stack.push((start, 0));
            while let Some(&(node, idx)) = stack.last() {
                let ss = succs(node);
                if idx < ss.len() {
                    stack.last_mut().unwrap().1 += 1;
                    let w = ss[idx];
                    if w < n {
                        match color[w] {
                            0 => {
                                color[w] = 1;
                                stack.push((w, 0));
                            }
                            1 => is_sp[w] = true,
                            _ => {}
                        }
                    }
                } else {
                    color[node] = 2;
                    stack.pop();
                }
            }
        }
        is_sp
    }

    /// Address of the per-thread `zap_shadow_head` TLS slot, always obtained by
    /// calling the runtime helper `zap_shadow_head_addr()` so the thread-local
    /// base is re-resolved on the CURRENT OS thread at every push/pop.
    ///
    /// A coroutine runs on an M:N green stack and may MIGRATE to a different OS
    /// thread (M) at any safepoint/yield; the shadow head is thread-local (one
    /// slot per M). Referencing the `zap_shadow_head` TLS global directly lets
    /// LLVM materialize the thread-pointer base once and keep it in a
    /// callee-saved register for the whole function. `swapcontext` preserves that
    /// register across a migration, so after the coroutine resumes on another M
    /// its push/pop still target the PREVIOUS M's slot — desyncing
    /// `g->shadow_head` from the real frames and freeing live objects (I#223,
    /// aarch64-only in practice: the x86 `%fs`-relative access is re-materialized
    /// per use). Routing through the non-inlinable call forces re-resolution on
    /// whichever M is current. (REPL JIT / I#159 already required this call,
    /// because MCJIT cannot emit a dynamic-TLS access to the dlopen'd runtime's
    /// `__thread` global; the AOT path now shares it for correctness under
    /// migration.)
    #[cfg(feature = "gc-immix")]
    fn gc_head(&self) -> PointerValue<'ctx> {
        let f = self.fns["zap_shadow_head_addr"];
        self.builder
            .build_call(f, &[], "shadow_head_addr")
            .expect("call zap_shadow_head_addr")
            .try_as_basic_value()
            .left()
            .expect("zap_shadow_head_addr returns a pointer")
            .into_pointer_value()
    }

    /// Build and link this function's shadow frame in the entry block and
    /// null-init its roots. Sets `self.gc_frame`. The GC-pointer-typed parameters
    /// are rooted by the caller afterwards (from `vals`, so an aggregate param's
    /// spilled pointer is the root — see `translate_fn`). A no-op (leaves
    /// `gc_frame = None`) for functions with no GC pointers.
    #[cfg(feature = "gc-immix")]
    fn gc_setup_frame(&mut self, f: &zir::Func) -> Result<(), String> {
        self.gc_frame = None;
        // An `Alloca`'s dest is a STACK-slot address, never a heap reference: the
        // collector never relocates it, and any GC pointer the slot *holds* is
        // tracked through the reference-local cell below (`local_slot_of`), whose
        // contents the collector traces. So an alloca dest needs no DIRECT root
        // slot. Rooting it anyway (it is `Ptr`-typed) was harmless — the collector
        // validates each root against the heap block registry and ignores a stack
        // pointer — but it forced a shadow frame onto functions whose only pointer
        // values are scalar-local addresses, e.g. `fib`, whose `int64` parameter
        // lowers to an `alloca i64`. Excluding alloca dests is what lets a provably
        // GC-free function skip the frame entirely (#568, D-0144): with no direct
        // root, no reference-local cell, and no aggregate, `n == 0` below and the
        // frame is elided. Reload is unaffected — an alloca address is stable, so
        // reading it from a root slot or straight from the register is the same
        // pointer.
        let mut is_alloca_dest = vec![false; f.value_types.len()];
        // A stack-promoted `StackStruct` (#839) is a stack-slot address: it is
        // never a heap reference the collector relocates, so it needs no DIRECT
        // root slot. A leaf (pointer-free) one needs no root at all; a
        // pointer-bearing one (#1081) is instead given an aggregate-root
        // `(base, desc)` pair below, which traces its interior fields in place.
        // Excluding every `StackStruct` here keeps the #568/D-0144 frame-skip (a
        // function whose only "allocations" are leaf stack structs stays
        // frameless) and is sound — the collector ignores a stack pointer anyway.
        //
        // A stack `StrBuild` is excluded ONLY when `bounded` (#1081): the join's
        // compile-time max length provably fits the inline buffer, so the runtime
        // spill path is unreachable and the result is always the stack buffer (no
        // root needed), removing the per-iteration `gc_store_root`. A NON-bounded
        // stack `StrBuild` may spill to a *heap* string at run time, and that
        // spill must be a precise GC root, so it keeps its ordinary direct-root
        // slot (rooting the stack-buffer case is harmless — the collector
        // validates each root against the heap-block registry).
        let mut is_stack_dest = vec![false; f.value_types.len()];
        for inst in f.blocks.iter().flat_map(|block| &block.insts) {
            match inst {
                Inst::Alloca { dest, .. } => is_alloca_dest[*dest as usize] = true,
                Inst::StackStruct { dest, .. } => is_stack_dest[*dest as usize] = true,
                Inst::StrBuild {
                    dest,
                    stack: true,
                    bounded: true,
                    ..
                } => is_stack_dest[*dest as usize] = true,
                _ => {}
            }
        }
        let mut slot_of = vec![-1i32; f.value_types.len()];
        let mut n: u32 = 0;
        for (vid, ty) in f.value_types.iter().enumerate() {
            if ty.is_ptr() && !is_alloca_dest[vid] && !is_stack_dest[vid] {
                slot_of[vid] = n as i32;
                n += 1;
            }
        }
        let mut local_slot_of = vec![-1i32; f.value_types.len()];
        for inst in f.blocks.iter().flat_map(|block| &block.insts) {
            if let Inst::Alloca { dest, pointee } = inst {
                if self.basic(*pointee).is_pointer_type() {
                    local_slot_of[*dest as usize] = n as i32;
                    n += 1;
                }
            }
        }
        let n_direct = n;
        // Aggregate roots (#586 / #1081): a stack spill whose interior reference
        // fields the direct roots above never cover (its own SSA pointer addresses
        // the stack, which the marker ignores). Reserve a `(base, desc)` pair for
        // each so the collector traces those fields IN PLACE through the struct's
        // precise descriptor. Two sources:
        //  - a by-value `value struct` / tuple SSA value (`struct_inline`), the
        //    D-0051 param/return spill (#586); and
        //  - a stack-promoted *pointer-bearing* reference struct (`is_stack_dest`
        //    + `struct_has_ptr`), the #1081 extension. A `StackStruct` is never
        //    `struct_inline` (the escape pass promotes only reference structs), so
        //    the two cases are disjoint.
        // A leaf (pointer-free) `StackStruct` has no interior reference fields and
        // so is never an aggregate root; a heap reference struct is covered by its
        // direct slot.
        let mut aggr_of = vec![-1i32; f.value_types.len()];
        let mut aggr_sids: Vec<u32> = Vec::new();
        for (vid, ty) in f.value_types.iter().enumerate() {
            if let Ty::Struct(sid) = ty {
                let s = *sid as usize;
                let is_aggr = if is_stack_dest[vid] {
                    self.struct_has_ptr[s]
                } else {
                    self.struct_inline[s]
                };
                if is_aggr {
                    aggr_of[vid] = aggr_sids.len() as i32;
                    aggr_sids.push(*sid);
                }
            }
        }
        let n_aggr = aggr_sids.len() as u32;
        n = n_direct + 2 * n_aggr;
        if n == 0 {
            return Ok(()); // nothing to root
        }
        let p = self.ctx.ptr_type(AddressSpace::default());
        let i64t = self.ctx.i64_type();
        let roots_ty = p.array_type(n);
        let frame_ty = self.ctx.struct_type(
            &[p.into(), i64t.into(), i64t.into(), roots_ty.into()],
            false,
        );
        let b = self.builder;
        let frame = b
            .build_alloca(frame_ty, "gcframe")
            .map_err(|e| e.to_string())?;
        // frame.prev = *head; frame.nroots = n_direct; frame.naggr = n_aggr;
        // roots[*] = null; store each aggregate pair's descriptor; *head = &frame.
        let head = self.gc_head();
        let prev = b
            .build_load(p, head, "gc.prev")
            .map_err(|e| e.to_string())?;
        let prev_slot = b
            .build_struct_gep(frame_ty, frame, 0, "gc.prevslot")
            .map_err(|e| e.to_string())?;
        b.build_store(prev_slot, prev).map_err(|e| e.to_string())?;
        let nslot = b
            .build_struct_gep(frame_ty, frame, 1, "gc.nslot")
            .map_err(|e| e.to_string())?;
        b.build_store(nslot, i64t.const_int(n_direct as u64, false))
            .map_err(|e| e.to_string())?;
        let naggr_slot = b
            .build_struct_gep(frame_ty, frame, 2, "gc.naggrslot")
            .map_err(|e| e.to_string())?;
        b.build_store(naggr_slot, i64t.const_int(n_aggr as u64, false))
            .map_err(|e| e.to_string())?;
        let roots = b
            .build_struct_gep(frame_ty, frame, 3, "gc.roots")
            .map_err(|e| e.to_string())?;
        // Zero the whole roots array (direct slots AND aggregate pairs) in one
        // memset: every direct slot and every aggregate `base` starts null, so a
        // safepoint reached before a value is defined sees no stale pointer.
        b.build_memset(
            roots,
            8,
            self.ctx.i8_type().const_zero(),
            i64t.const_int((n as u64) * 8, false),
        )
        .map_err(|e| e.to_string())?;
        // An aggregate pair's descriptor is constant for the function, so store it
        // once here; its `base` is filled in at the spill's definition. The
        // collector reads `desc` only when `base` is a non-null stack pointer.
        for (j, sid) in aggr_sids.iter().enumerate() {
            let desc_idx = n_direct + 2 * (j as u32) + 1;
            let elem = unsafe {
                b.build_in_bounds_gep(
                    roots_ty,
                    roots,
                    &[
                        self.ctx.i32_type().const_zero(),
                        self.ctx.i32_type().const_int(desc_idx as u64, false),
                    ],
                    "gc.aggr.desc",
                )
                .map_err(|e| e.to_string())?
            };
            b.build_store(elem, self.struct_descs[*sid as usize])
                .map_err(|e| e.to_string())?;
        }
        b.build_store(head, frame).map_err(|e| e.to_string())?;
        self.gc_frame = Some(GcFrame {
            frame_ptr: frame,
            frame_ty,
            slot_of,
            local_slot_of,
            aggr_of,
            n_direct,
            n,
        });
        // Parameters are rooted by the caller (translate_fn) from `vals`, not from
        // the raw LLVM params: a by-value aggregate parameter (an inline value
        // struct / tuple, D-0051) arrives as an aggregate and is spilled to a stack
        // slot whose POINTER lives in `vals`; that pointer — not the non-pointer
        // aggregate — is the root, and is also what load-at-use reloads (#120).
        Ok(())
    }

    /// Reference locals live IN the shadow frame, rather than in a separate
    /// untraced stack alloca. Stores and collector relocation then update the
    /// same pointer cell that a later ZIR Load reads (#583).
    #[cfg(feature = "gc-immix")]
    fn gc_local_slot(&self, vid: u32) -> Result<Option<PointerValue<'ctx>>, String> {
        let Some(frame) = &self.gc_frame else {
            return Ok(None);
        };
        let slot = frame.local_slot_of[vid as usize];
        if slot < 0 {
            return Ok(None);
        }
        // Materialize the slot address in the ENTRY block, exactly as
        // `entry_alloca` hoists a plain stack slot (ticket #91). This pointer is
        // cached as the reference local's storage and reused by every `Load` /
        // `Store` of it across all blocks, so it must dominate them. The `Alloca`
        // inst is codegen'd wherever it appears in the ZIR; when that is a block
        // that does not dominate all uses — e.g. an invoke-continuation block
        // introduced by a #1493 null-dereference guard on a `len`/index/`for` —
        // a GEP built at the current position would not dominate its uses and the
        // module fails verification ("Instruction does not dominate all uses").
        // Building it in the entry block (where the frame pointer is live) makes
        // it dominate unconditionally. Restore the insertion point afterward.
        let saved = self.builder.get_insert_block();
        if let Some(entry) = self.entry_block {
            match entry.get_terminator() {
                Some(term) => self.builder.position_before(&term),
                None => self.builder.position_at_end(entry),
            }
        }
        let roots = self
            .builder
            .build_struct_gep(frame.frame_ty, frame.frame_ptr, 3, "gc.local.roots")
            .map_err(|e| e.to_string())?;
        let p = self.ctx.ptr_type(AddressSpace::default());
        let local = unsafe {
            self.builder.build_in_bounds_gep(
                p.array_type(frame.n),
                roots,
                &[
                    self.ctx.i32_type().const_zero(),
                    self.ctx.i32_type().const_int(slot as u64, false),
                ],
                "gc.local.slot",
            )
        }
        .map_err(|e| e.to_string())?;
        if let Some(bb) = saved {
            self.builder.position_at_end(bb);
        }
        Ok(Some(local))
    }

    /// Store SSA value `vid` into its root slot, if it has one. `val` must be a
    /// pointer value. Called at the end of the value's defining block.
    ///
    /// For a by-value `value struct` / tuple value (`aggr_of >= 0`) the same
    /// pointer is also written into its aggregate-root pair's `base` word, so the
    /// collector can trace the spill's interior pointer fields in place (#586). A
    /// heap-resident `Struct` value (e.g. a `StructNew` instance) is rooted by its
    /// direct slot as usual; its `base` word is ignored by the collector because
    /// the base is a heap pointer, not a stack spill.
    #[cfg(feature = "gc-immix")]
    fn gc_store_root(&self, vid: u32, val: BasicValueEnum<'ctx>) -> Result<(), String> {
        let Some(frame) = &self.gc_frame else {
            return Ok(());
        };
        if !val.is_pointer_value() {
            return Ok(());
        }
        let slot = frame.slot_of[vid as usize];
        let aggr = frame.aggr_of[vid as usize];
        if slot < 0 && aggr < 0 {
            return Ok(());
        }
        let b = self.builder;
        let p = self.ctx.ptr_type(AddressSpace::default());
        let roots = b
            .build_struct_gep(frame.frame_ty, frame.frame_ptr, 3, "gc.roots")
            .map_err(|e| e.to_string())?;
        let store_at = |idx: u32| -> Result<(), String> {
            let elem = unsafe {
                b.build_in_bounds_gep(
                    p.array_type(frame.n),
                    roots,
                    &[
                        self.ctx.i32_type().const_zero(),
                        self.ctx.i32_type().const_int(idx as u64, false),
                    ],
                    "gc.slot",
                )
                .map_err(|e| e.to_string())?
            };
            b.build_store(elem, val.into_pointer_value())
                .map_err(|e| e.to_string())?;
            Ok(())
        };
        if slot >= 0 {
            store_at(slot as u32)?;
        }
        if aggr >= 0 {
            store_at(frame.n_direct + 2 * (aggr as u32))?;
        }
        Ok(())
    }

    /// Load-at-use reload for the MOVING collector (ticket #120, gc-immix only).
    ///
    /// The precise collector may now RELOCATE a live object during a collection
    /// (evacuation/defrag) and rewrite every reference to it — including the
    /// GC-pointer SSA value's shadow-frame slot (`gc_store_root` writes it at the
    /// def site, I#167). But compiled code keeps the value in an SSA register and
    /// never re-reads the slot, so after a safepoint that moved the object the
    /// register would hold the stale OLD address → use-after-free. To let compiled
    /// code observe relocations, every USE of a GC-pointer operand re-reads it from
    /// its (collector-updated) slot instead of trusting the register.
    ///
    /// The slot is the canonical home: `gc_store_root` stores at the def site, the
    /// collector updates it, and this load sits at the use site (in the using
    /// block, dominated by the entry-block frame alloca) — so no liveness or
    /// cross-block dominance analysis is needed. The gcframe alloca's address
    /// escapes (`*zap_shadow_head = &frame`), so LLVM cannot promote it to a
    /// register; it does, however, CSE redundant loads between two safepoints (a
    /// safepoint is an opaque call that may clobber the frame), collapsing the cost
    /// to roughly one reload per value per safepoint-interval. Non-GC pointers
    /// (allocas, function addresses, inline value-struct pointers) are never
    /// relocated by the collector, so reloading their slot returns the same
    /// pointer — correct and uniform.
    ///
    /// Under the Boehm (non-moving) build this is a no-op returning the register
    /// value unchanged, so that codegen is byte-for-byte identical and pays no
    /// reload cost.
    #[cfg_attr(not(feature = "gc-immix"), allow(unused_variables))]
    fn gc_reload_operand(&self, vid: u32, raw: BasicValueEnum<'ctx>) -> BasicValueEnum<'ctx> {
        #[cfg(feature = "gc-immix")]
        {
            if raw.is_pointer_value() {
                if let Some(frame) = &self.gc_frame {
                    let slot = frame.slot_of[vid as usize];
                    if slot >= 0 {
                        let b = self.builder;
                        let p = self.ctx.ptr_type(AddressSpace::default());
                        if let Ok(roots) =
                            b.build_struct_gep(frame.frame_ty, frame.frame_ptr, 3, "gc.roots")
                        {
                            let elem = unsafe {
                                b.build_in_bounds_gep(
                                    p.array_type(frame.n),
                                    roots,
                                    &[
                                        self.ctx.i32_type().const_zero(),
                                        self.ctx.i32_type().const_int(slot as u64, false),
                                    ],
                                    "gc.slot",
                                )
                            };
                            if let Ok(elem) = elem {
                                if let Ok(v) = b.build_load(p, elem, "gc.reload") {
                                    return v;
                                }
                            }
                        }
                    }
                }
            }
        }
        raw
    }

    /// Unlink the shadow frame (`*head = frame.prev`) before a `ret`.
    #[cfg(feature = "gc-immix")]
    fn gc_unlink(&self) -> Result<(), String> {
        let Some(frame) = &self.gc_frame else {
            return Ok(());
        };
        let b = self.builder;
        let p = self.ctx.ptr_type(AddressSpace::default());
        let prev_slot = b
            .build_struct_gep(frame.frame_ty, frame.frame_ptr, 0, "gc.prevslot")
            .map_err(|e| e.to_string())?;
        let prev = b
            .build_load(p, prev_slot, "gc.prev")
            .map_err(|e| e.to_string())?;
        b.build_store(self.gc_head(), prev)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Re-establish this frame as the shadow head on entry to a landing pad: the
    /// deeper frames unwinding tore down are abandoned, and this (still-valid)
    /// frame's prev chain reaches the live caller frames.
    #[cfg(feature = "gc-immix")]
    fn gc_restore_head(&self) -> Result<(), String> {
        let Some(frame) = &self.gc_frame else {
            return Ok(());
        };
        self.builder
            .build_store(self.gc_head(), frame.frame_ptr)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn translate_fn(&mut self, f: &Func) -> Result<(), String> {
        let fv = self.fns[&f.name];

        // Force asynchronous unwind tables (.eh_frame / CFI) on every function so
        // the unwinder can walk through frames that have no landing pad of their
        // own (ticket #22). A function that installs a landing pad also needs a
        // personality routine.
        let uwtable = Attribute::get_named_enum_kind_id("uwtable");
        if uwtable != 0 {
            fv.add_attribute(
                AttributeLoc::Function,
                self.ctx.create_enum_attribute(uwtable, 2),
            );
        }
        let has_landing_pad = f
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .any(|i| matches!(i, Inst::LandingPad { .. }));
        if has_landing_pad {
            fv.set_personality_function(self.fns[self.personality]);
        }

        let blocks: Vec<BasicBlock> = f
            .blocks
            .iter()
            .map(|b| self.ctx.append_basic_block(fv, &format!("b{}", b.id)))
            .collect();

        // Hoist target for fixed-size allocas (ticket #91): block 0 is the entry
        // block, executed once per call and dominating every other block.
        self.entry_block = Some(blocks[0]);

        let mut vals: Vec<Option<BasicValueEnum>> = vec![None; f.value_types.len()];
        // Position at the entry block so a by-value aggregate parameter can be
        // spilled into a stack slot there (the body addresses it through a
        // pointer). Later instructions for the entry block append after these.
        self.builder.position_at_end(blocks[0]);
        for (i, slot) in vals.iter_mut().take(f.params.len()).enumerate() {
            let p = fv.get_nth_param(i as u32).unwrap();
            if p.is_struct_value() {
                // An inline value struct / tuple parameter arrives by value
                // (D-0051); spill it so FieldGet/FieldSet can address it.
                let spill = self
                    .builder
                    .build_alloca(p.get_type(), "param.spill")
                    .map_err(|e| e.to_string())?;
                self.builder
                    .build_store(spill, p)
                    .map_err(|e| e.to_string())?;
                *slot = Some(spill.into());
            } else {
                *slot = Some(p);
            }
        }

        // Precise GC (ticket #48): build and link this function's shadow-stack
        // frame in the entry block before any safepoint can observe it, then root
        // the GC-pointer parameters from `vals` — which holds the spilled POINTER
        // for a by-value aggregate parameter, so the shadow slot mirrors the value
        // the body (and load-at-use reloads) actually uses (#120). Rooting the raw
        // aggregate param instead would leave the slot null and the reload would
        // read null.
        #[cfg(feature = "gc-immix")]
        {
            self.gc_setup_frame(f)?;
            for (i, slot) in vals.iter().take(f.params.len()).enumerate() {
                if let Some(v) = slot {
                    self.gc_store_root(i as u32, *v)?;
                }
            }
        }

        // A GC safepoint poll at the function entry and every loop header (#568,
        // D-0144): the only places a collection runs and the preemption landing
        // points. The old FORK-4a emission polled the top of *every* block; the
        // inlined flag-check poll (see `gc_safepoint`) plus this reduced placement
        // removes the per-call overhead #568 measured while preserving STW/
        // preemption liveness (every call chain polls at a callee entry; every
        // loop iteration passes through its header).
        #[cfg(feature = "gc-immix")]
        let sp_blocks = Self::safepoint_blocks(f);
        for (bi, b) in f.blocks.iter().enumerate() {
            self.builder.position_at_end(blocks[bi]);
            // A landing-pad block starts with the mandatory `landingpad`
            // instruction, so its safepoint + head-restore are emitted in the
            // LandingPad handler instead.
            #[cfg(feature = "gc-immix")]
            if sp_blocks[bi] && !block_starts_with_landing_pad(b) {
                self.gc_safepoint(fv)?;
            }
            for inst in &b.insts {
                self.inst(f, inst, &mut vals, fv, &blocks)?;
                // Root a GC pointer the instant it is defined — NOT deferred to
                // the end of the block. A block may contain several calls, each an
                // interior GC safepoint; a heap pointer defined by one call must
                // already be in its shadow-frame slot before a *later* same-block
                // call can trigger a cross-coroutine collection, or that object is
                // unrooted and can be reclaimed out from under us (I#167).
                #[cfg(feature = "gc-immix")]
                if let Some(d) = inst_dest(inst) {
                    if let Some(Some(v)) = vals.get(d as usize) {
                        self.gc_store_root(d, *v)?;
                    }
                }
            }
            self.term(f, &b.term, &blocks, &vals, fv)?;
        }
        self.gc_frame = None;
        self.entry_block = None;
        Ok(())
    }

    // --- helpers ---

    fn basic(&self, t: Ty) -> BasicTypeEnum<'ctx> {
        match t {
            Ty::I1 => self.ctx.bool_type().into(),
            // Unsigned integers share the sign-agnostic LLVM integer type of the
            // same width as their signed counterparts (#552); signedness lives in
            // the ops (udiv/urem/lshr, unsigned icmp, zext/uitofp), not the type.
            Ty::I8 | Ty::U8 => self.ctx.i8_type().into(),
            Ty::I16 | Ty::U16 => self.ctx.i16_type().into(),
            // `char` is physically an `i32` codepoint (#709).
            Ty::I32 | Ty::Char | Ty::U32 => self.ctx.i32_type().into(),
            Ty::I64 | Ty::U64 => self.ctx.i64_type().into(),
            Ty::Enum(_) => self.ctx.i64_type().into(),
            Ty::F32 => self.ctx.f32_type().into(),
            Ty::F64 => self.ctx.f64_type().into(),
            Ty::Ptr
            | Ty::Void
            | Ty::Struct(_)
            | Ty::List(_)
            | Ty::Dict(..)
            | Ty::Set(_)
            | Ty::Heap(_)
            | Ty::Deque(_)
            | Ty::OSet(_)
            | Ty::VarTuple(_)
            | Ty::Array(..)
            | Ty::Chan(_)
            | Ty::AtomicArray(_)
            | Ty::ContextVar(_)
            | Ty::CtxToken
            | Ty::Bytes
            | Ty::Match
            | Ty::Func(_)
            | Ty::Py
            | Ty::Bigint
            | Ty::Complex => self.ctx.ptr_type(AddressSpace::default()).into(),
        }
    }

    /// Read a C-style bitfield (I#173, D-0068): load the struct's backing
    /// integer, bring the field's bit slice down to bit 0, and extend it to the
    /// field's logical type. Signed integer fields are **sign-extended**; `bool`
    /// and tier-1 enums are zero-extended.
    fn packed_field_get(
        &self,
        b: &Builder<'ctx>,
        base: PointerValue<'ctx>,
        st: inkwell::types::StructType<'ctx>,
        backing: Ty,
        bf: zir::ir::BitField,
        dest_ty: BasicTypeEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let int_ty = self.basic(backing).into_int_type();
        let nbits = int_ty.get_bit_width();
        let unit_ptr = b
            .build_struct_gep(st, base, 0, "bf.unit")
            .map_err(|_| "bitfield backing unit".to_string())?;
        let unit = b
            .build_load(int_ty, unit_ptr, "bf.load")
            .map_err(|e| e.to_string())?
            .into_int_value();
        let extracted = if bf.signed {
            // Shift the field to the top of the word, then arithmetic-shift it
            // back down: the top bit of the slice becomes the sign bit.
            let up = (nbits - bf.width - bf.offset) as u64;
            let shl = b
                .build_left_shift(unit, int_ty.const_int(up, false), "bf.shl")
                .map_err(|e| e.to_string())?;
            b.build_right_shift(
                shl,
                int_ty.const_int((nbits - bf.width) as u64, false),
                true,
                "bf.ashr",
            )
            .map_err(|e| e.to_string())?
        } else {
            let down = b
                .build_right_shift(
                    unit,
                    int_ty.const_int(bf.offset as u64, false),
                    false,
                    "bf.lshr",
                )
                .map_err(|e| e.to_string())?;
            b.build_and(
                down,
                int_ty.const_int(width_mask(bf.width), false),
                "bf.mask",
            )
            .map_err(|e| e.to_string())?
        };
        let v = b
            .build_int_cast_sign_flag(extracted, dest_ty.into_int_type(), bf.signed, "bf.cast")
            .map_err(|e| e.to_string())?;
        Ok(v.into())
    }

    /// Write a C-style bitfield (I#173, D-0068): load the backing integer, clear
    /// the field's bits, and OR in the low `width` bits of `val` shifted into
    /// place. Only the field's bits change — neighbours are preserved.
    fn packed_field_set(
        &self,
        b: &Builder<'ctx>,
        base: PointerValue<'ctx>,
        st: inkwell::types::StructType<'ctx>,
        backing: Ty,
        bf: zir::ir::BitField,
        val: BasicValueEnum<'ctx>,
    ) -> Result<(), String> {
        let int_ty = self.basic(backing).into_int_type();
        let unit_ptr = b
            .build_struct_gep(st, base, 0, "bf.unit")
            .map_err(|_| "bitfield backing unit".to_string())?;
        let unit = b
            .build_load(int_ty, unit_ptr, "bf.load")
            .map_err(|e| e.to_string())?
            .into_int_value();
        // Bring the incoming value to the backing width; masking below drops any
        // high bits, so a plain (zero-extending) cast is correct for either sign.
        let v = b
            .build_int_cast_sign_flag(val.into_int_value(), int_ty, false, "bf.vin")
            .map_err(|e| e.to_string())?;
        let masked = b
            .build_and(v, int_ty.const_int(width_mask(bf.width), false), "bf.vmask")
            .map_err(|e| e.to_string())?;
        let shifted = b
            .build_left_shift(masked, int_ty.const_int(bf.offset as u64, false), "bf.vshl")
            .map_err(|e| e.to_string())?;
        let clear = int_ty.const_int(!(width_mask(bf.width) << bf.offset), false);
        let cleared = b
            .build_and(unit, clear, "bf.clear")
            .map_err(|e| e.to_string())?;
        let new_unit = b
            .build_or(cleared, shifted, "bf.or")
            .map_err(|e| e.to_string())?;
        b.build_store(unit_ptr, new_unit)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The **ABI** (calling-convention) type for `t`. Identical to [`basic`]
    /// except that an inline value struct / tuple (D-0051) is its LLVM aggregate
    /// type *by value* — so the backend passes small ones in registers and
    /// returns larger ones via an `sret` pointer — instead of a heap pointer.
    /// Used only at call boundaries (function signatures, call arguments,
    /// returns); the function *body* still addresses every aggregate through a
    /// pointer, which is what [`basic`] yields.
    fn abi_ty(&self, t: Ty) -> BasicTypeEnum<'ctx> {
        if let Ty::Struct(sid) = t {
            if self.struct_inline[sid as usize] {
                return self.structs[sid as usize].into();
            }
        }
        self.basic(t)
    }

    /// Materialize a call argument for the ABI: when the callee expects an
    /// inline aggregate *by value* (`param_ty` is a struct type), load it from
    /// the pointer the body holds; otherwise pass the value through.
    fn abi_arg(
        &self,
        param_ty: BasicTypeEnum<'ctx>,
        v: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if param_ty.is_struct_type() {
            self.builder
                .build_load(param_ty, v.into_pointer_value(), "arg.agg")
                .map_err(|e| e.to_string())
        } else {
            Ok(v)
        }
    }

    /// Spill a by-value aggregate *result* into a stack slot, yielding the
    /// pointer the rest of the body uses to address it (FieldGet/FieldSet,
    /// D-0051). Non-aggregate results pass through unchanged.
    fn abi_result(&self, v: BasicValueEnum<'ctx>) -> Result<BasicValueEnum<'ctx>, String> {
        if v.is_struct_value() {
            // Hoisted to the entry block (ticket #91): a call that returns a
            // by-value aggregate inside a loop would otherwise spill a fresh
            // slot every iteration.
            let slot = self.entry_alloca(v.get_type(), "ret.spill")?;
            self.builder
                .build_store(slot, v)
                .map_err(|e| e.to_string())?;
            Ok(slot.into())
        } else {
            Ok(v)
        }
    }

    fn int_ty(&self, t: Ty) -> IntType<'ctx> {
        match t {
            Ty::I1 => self.ctx.bool_type(),
            Ty::I8 | Ty::U8 => self.ctx.i8_type(),
            Ty::I16 | Ty::U16 => self.ctx.i16_type(),
            Ty::I32 | Ty::Char | Ty::U32 => self.ctx.i32_type(),
            _ => self.ctx.i64_type(),
        }
    }

    fn float_ty(&self, t: Ty) -> FloatType<'ctx> {
        match t {
            Ty::F32 => self.ctx.f32_type(),
            _ => self.ctx.f64_type(),
        }
    }

    // --- Atomic[T] codegen helpers (I#176) ----------------------------------

    /// The integer **access type** an atomic op uses for a cell of type `elem`.
    /// `bool` (`i1`) is accessed as `i8` (LLVM atomics require a byte-sized,
    /// power-of-two integer); a reference payload is accessed as a pointer-wide
    /// machine word. The other integer widths map to themselves.
    fn atom_int_ty(&self, elem: Ty) -> IntType<'ctx> {
        match elem {
            Ty::I1 | Ty::I8 => self.ctx.i8_type(),
            Ty::I16 => self.ctx.i16_type(),
            Ty::I32 | Ty::Char => self.ctx.i32_type(),
            Ty::I64 => self.ctx.i64_type(),
            // A reference payload (struct/list/str/...) is a machine-word pointer.
            _ => self.ctx.i64_type(),
        }
    }

    /// Map a ZIR [`AtomicOrd`] to the inkwell ordering. Only `seq_cst` is emitted
    /// today (the default); the mapping is centralized so weaker orderings slot in
    /// here when they are added.
    fn atom_ord(o: AtomicOrd) -> AtomicOrdering {
        match o {
            AtomicOrd::SeqCst => AtomicOrdering::SequentiallyConsistent,
        }
    }

    /// GEP the cell field of an `Atomic[T]` box (field `idx` of struct `sid`),
    /// yielding the pointer the atomic op operates on.
    fn atomic_cell_ptr(
        &self,
        b: &Builder<'ctx>,
        vals: &[Option<BasicValueEnum<'ctx>>],
        get: &dyn Fn(&[Option<BasicValueEnum<'ctx>>], u32) -> BasicValueEnum<'ctx>,
        base: u32,
        sid: u32,
        idx: u32,
    ) -> Result<PointerValue<'ctx>, String> {
        let st = self.structs[sid as usize];
        let p = get(vals, base).into_pointer_value();
        b.build_struct_gep(st, p, idx, "atomic.cell")
            .map_err(|_| "invalid atomic cell field".to_string())
    }

    /// Convert a cell value to its integer access representation for an atomic
    /// store / rmw / cas input: a reference becomes `ptrtoint` to the word, a
    /// `bool` (`i1`) is zero-extended to `i8`, other integers pass through.
    fn atom_to_int(
        &self,
        b: &Builder<'ctx>,
        v: BasicValueEnum<'ctx>,
        elem: Ty,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if elem.is_ptr() {
            let i64t = self.ctx.i64_type();
            return b
                .build_ptr_to_int(v.into_pointer_value(), i64t, "atomic.p2i")
                .map(Into::into)
                .map_err(|e| e.to_string());
        }
        if elem == Ty::I1 {
            return b
                .build_int_z_extend(v.into_int_value(), self.ctx.i8_type(), "atomic.zext")
                .map(Into::into)
                .map_err(|e| e.to_string());
        }
        Ok(v)
    }

    /// Convert an integer access value back to the cell type `elem` for an atomic
    /// load / swap result: `inttoptr` for a reference, truncate `i8`→`i1` for a
    /// `bool`, other integers pass through.
    fn atom_from_int(
        &self,
        b: &Builder<'ctx>,
        v: inkwell::values::IntValue<'ctx>,
        elem: Ty,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if elem.is_ptr() {
            let pt = self.ctx.ptr_type(AddressSpace::default());
            return b
                .build_int_to_ptr(v, pt, "atomic.i2p")
                .map(Into::into)
                .map_err(|e| e.to_string());
        }
        if elem == Ty::I1 {
            return b
                .build_int_truncate(v, self.ctx.bool_type(), "atomic.trunc")
                .map(Into::into)
                .map_err(|e| e.to_string());
        }
        Ok(v.into())
    }

    fn inst(
        &self,
        f: &Func,
        inst: &Inst,
        vals: &mut [Option<BasicValueEnum<'ctx>>],
        fv: FunctionValue<'ctx>,
        blocks: &[BasicBlock<'ctx>],
    ) -> Result<(), String> {
        // Every operand use goes through `get`, which (under the moving gc-immix
        // collector) re-reads a GC pointer from its shadow-frame slot so compiled
        // code observes any relocation done at an intervening safepoint (#120). A
        // no-op passthrough under Boehm.
        let get = |vals: &[Option<BasicValueEnum<'ctx>>], v: u32| -> BasicValueEnum<'ctx> {
            let raw = vals[v as usize].expect("ZIR value used before definition");
            self.gc_reload_operand(v, raw)
        };
        let b = self.builder;
        match inst {
            Inst::IConst { dest, val } => {
                let it = self.int_ty(f.value_ty(*dest));
                vals[*dest as usize] = Some(it.const_int(*val as u64, false).into());
            }
            Inst::FConst { dest, val } => {
                let ft = self.float_ty(f.value_ty(*dest));
                vals[*dest as usize] = Some(ft.const_float(*val).into());
            }
            Inst::BConst { dest, val } => {
                vals[*dest as usize] =
                    Some(self.ctx.bool_type().const_int(*val as u64, false).into());
            }
            // Niche-optional support (I#154, D-0064 #4). `NullConst` is the null
            // pointer that a niche `Optional`'s `None` lowers to; `IsNull` backs its
            // tag-free `match` test. Both are ordinary pointer operations — the
            // precise collector already skips a null pointer slot
            // (`zap_is_heap_ptr` in `zapgc.inc.c`), so a niche optional needs no
            // GC change.
            Inst::NullConst { dest } => {
                let null = self.ctx.ptr_type(AddressSpace::default()).const_null();
                vals[*dest as usize] = Some(null.into());
            }
            Inst::IsNull { dest, val } => {
                let p = get(vals, *val).into_pointer_value();
                let c = b.build_is_null(p, "isnull").map_err(|e| e.to_string())?;
                vals[*dest as usize] = Some(c.into());
            }
            Inst::Bin { dest, op, lhs, rhs } => {
                let ty = f.value_ty(*dest);
                let l = get(vals, *lhs);
                let r = get(vals, *rhs);
                let res: BasicValueEnum = if ty.is_float() {
                    let (lf, rf) = (l.into_float_value(), r.into_float_value());
                    use zir::BinOp::*;
                    let fv = match op {
                        Add => b.build_float_add(lf, rf, "fadd"),
                        Sub => b.build_float_sub(lf, rf, "fsub"),
                        Mul => b.build_float_mul(lf, rf, "fmul"),
                        Div => b.build_float_div(lf, rf, "fdiv"),
                        Rem => b.build_float_rem(lf, rf, "frem"),
                        _ => return Err("float op not supported".into()),
                    }
                    .map_err(|e| e.to_string())?;
                    // FP determinism policy (#946, D-0196): stamp contraction /
                    // reassociation per `--fp` so a source `a*b+c` may fuse to an
                    // FMA under `default`/`fast` and stays two rounded ops under
                    // `strict`.
                    self.stamp_fp(fv).into()
                } else {
                    let (li, ri) = (l.into_int_value(), r.into_int_value());
                    use zir::BinOp::*;
                    // Select signed vs unsigned integer ops from the result type
                    // (#552). Add/Sub/Mul and the bitwise And/Or/Xor/Shl are
                    // sign-agnostic (two's complement); only Div/Rem and the
                    // right shift differ: unsigned uses `udiv`/`urem` and a
                    // logical shift (`lshr`), signed uses `sdiv`/`srem`/`ashr`.
                    let unsigned = ty.is_unsigned();
                    match op {
                        Add => b.build_int_add(li, ri, "add"),
                        Sub => b.build_int_sub(li, ri, "sub"),
                        Mul => b.build_int_mul(li, ri, "mul"),
                        Div if unsigned => b.build_int_unsigned_div(li, ri, "udiv"),
                        Div => b.build_int_signed_div(li, ri, "sdiv"),
                        Rem if unsigned => b.build_int_unsigned_rem(li, ri, "urem"),
                        Rem => b.build_int_signed_rem(li, ri, "srem"),
                        And => b.build_and(li, ri, "and"),
                        Or => b.build_or(li, ri, "or"),
                        Xor => b.build_xor(li, ri, "xor"),
                        Shl => b.build_left_shift(li, ri, "shl"),
                        // `false` => logical right shift (`lshr`) for unsigned;
                        // `true` => arithmetic (`ashr`) for signed.
                        Shr => b.build_right_shift(
                            li,
                            ri,
                            !unsigned,
                            if unsigned { "lshr" } else { "ashr" },
                        ),
                    }
                    .map_err(|e| e.to_string())?
                    .into()
                };
                vals[*dest as usize] = Some(res);
            }
            Inst::ICmp {
                dest,
                pred,
                lhs,
                rhs,
            } => {
                // `is` / `is not` (#611, D18) compare two reference pointers for
                // same-allocation identity. LLVM `icmp` needs integer operands,
                // so pointer operands are `ptrtoint`-ed to a word first; integer
                // operands (every other `ICmp` site) pass through unchanged.
                let lv = get(vals, *lhs);
                let rv = get(vals, *rhs);
                let (li, ri) = if lv.is_pointer_value() || rv.is_pointer_value() {
                    let i64t = self.ctx.i64_type();
                    let li = if lv.is_pointer_value() {
                        b.build_ptr_to_int(lv.into_pointer_value(), i64t, "is.p2i")
                            .map_err(|e| e.to_string())?
                    } else {
                        lv.into_int_value()
                    };
                    let ri = if rv.is_pointer_value() {
                        b.build_ptr_to_int(rv.into_pointer_value(), i64t, "is.p2i")
                            .map_err(|e| e.to_string())?
                    } else {
                        rv.into_int_value()
                    };
                    (li, ri)
                } else {
                    (lv.into_int_value(), rv.into_int_value())
                };
                // Pick the unsigned ordering predicate (ULT/ULE/UGT/UGE) when the
                // operands are an unsigned integer type (#552); EQ/NE are the
                // same for both. Signedness comes from the operand type, not the
                // `i1` result. The `is`/`is not` pointer path only uses EQ/NE.
                let unsigned = f.value_ty(*lhs).is_unsigned() || f.value_ty(*rhs).is_unsigned();
                let c = b
                    .build_int_compare(int_pred(*pred, unsigned), li, ri, "icmp")
                    .map_err(|e| e.to_string())?;
                vals[*dest as usize] = Some(c.into());
            }
            Inst::FCmp {
                dest,
                pred,
                lhs,
                rhs,
            } => {
                let c = b
                    .build_float_compare(
                        float_pred(*pred),
                        get(vals, *lhs).into_float_value(),
                        get(vals, *rhs).into_float_value(),
                        "fcmp",
                    )
                    .map_err(|e| e.to_string())?;
                vals[*dest as usize] = Some(c.into());
            }
            Inst::Un { dest, op, val } => {
                let v = get(vals, *val);
                let res: BasicValueEnum = match op {
                    zir::UnOp::Neg => {
                        if v.is_float_value() {
                            b.build_float_neg(v.into_float_value(), "fneg")
                                .map_err(|e| e.to_string())?
                                .into()
                        } else {
                            b.build_int_neg(v.into_int_value(), "neg")
                                .map_err(|e| e.to_string())?
                                .into()
                        }
                    }
                    zir::UnOp::Not | zir::UnOp::BitNot => b
                        .build_not(v.into_int_value(), "not")
                        .map_err(|e| e.to_string())?
                        .into(),
                };
                vals[*dest as usize] = Some(res);
            }
            Inst::Cast { dest, kind, val } => {
                let v = get(vals, *val);
                let to = f.value_ty(*dest);
                let res: BasicValueEnum = match kind {
                    CastKind::SExt => b
                        .build_int_s_extend(v.into_int_value(), self.int_ty(to), "sext")
                        .map_err(|e| e.to_string())?
                        .into(),
                    // Zero-extend an unsigned source so its value is preserved
                    // (#552): `uint8(200)` widened stays `200`, not `-56` worth
                    // of sign bits.
                    CastKind::ZExt => b
                        .build_int_z_extend(v.into_int_value(), self.int_ty(to), "zext")
                        .map_err(|e| e.to_string())?
                        .into(),
                    CastKind::Trunc => b
                        .build_int_truncate(v.into_int_value(), self.int_ty(to), "trunc")
                        .map_err(|e| e.to_string())?
                        .into(),
                    CastKind::SIToFP => b
                        .build_signed_int_to_float(v.into_int_value(), self.float_ty(to), "sitofp")
                        .map_err(|e| e.to_string())?
                        .into(),
                    // Unsigned int -> float (#552): `uitofp`, so a `uint64` above
                    // `INT64_MAX` converts to its true magnitude.
                    CastKind::UIToFP => b
                        .build_unsigned_int_to_float(
                            v.into_int_value(),
                            self.float_ty(to),
                            "uitofp",
                        )
                        .map_err(|e| e.to_string())?
                        .into(),
                    CastKind::FPToSI => b
                        .build_float_to_signed_int(v.into_float_value(), self.int_ty(to), "fptosi")
                        .map_err(|e| e.to_string())?
                        .into(),
                    // Float -> unsigned int (#552): `fptoui`.
                    CastKind::FPToUI => b
                        .build_float_to_unsigned_int(
                            v.into_float_value(),
                            self.int_ty(to),
                            "fptoui",
                        )
                        .map_err(|e| e.to_string())?
                        .into(),
                    CastKind::FPExt | CastKind::FPTrunc => b
                        .build_float_cast(v.into_float_value(), self.float_ty(to), "fcast")
                        .map_err(|e| e.to_string())?
                        .into(),
                };
                vals[*dest as usize] = Some(res);
            }
            Inst::Alloca { dest, pointee } => {
                // Hoist the stack slot to the entry block (ticket #91) so a loop
                // body that declares a local does not re-`alloca` it each
                // iteration and overflow the stack on a long-running loop.
                #[cfg(feature = "gc-immix")]
                let rooted = self.gc_local_slot(*dest)?;
                #[cfg(not(feature = "gc-immix"))]
                let rooted: Option<PointerValue<'ctx>> = None;
                let p = match rooted {
                    Some(slot) => slot,
                    None => self.entry_alloca(self.basic(*pointee), "slot")?,
                };
                vals[*dest as usize] = Some(p.into());
            }
            Inst::Load { dest, ptr } => {
                let ty = self.basic(f.value_ty(*dest));
                let p = get(vals, *ptr).into_pointer_value();
                let v = b.build_load(ty, p, "load").map_err(|e| e.to_string())?;
                vals[*dest as usize] = Some(v);
            }
            Inst::Store { ptr, val } => {
                let p = get(vals, *ptr).into_pointer_value();
                b.build_store(p, get(vals, *val))
                    .map_err(|e| e.to_string())?;
            }
            Inst::Select {
                dest,
                cond,
                a,
                b: bb,
            } => {
                let sel = b
                    .build_select(
                        get(vals, *cond).into_int_value(),
                        get(vals, *a),
                        get(vals, *bb),
                        "sel",
                    )
                    .map_err(|e| e.to_string())?;
                vals[*dest as usize] = Some(sel);
            }
            Inst::Call { dest, func, args } => {
                let fv = *self
                    .fns
                    .get(func)
                    .ok_or_else(|| format!("call to unknown function '{func}'"))?;
                // By-value aggregate arguments (inline value structs / tuples,
                // D-0051) are loaded from their pointer per the callee signature;
                // a struct result is spilled back to a pointer for the body.
                let param_tys = fv.get_type().get_param_types();
                let mut argvals: Vec<inkwell::values::BasicMetadataValueEnum> =
                    Vec::with_capacity(args.len());
                for (i, a) in args.iter().enumerate() {
                    let v = get(vals, *a);
                    let v = match param_tys.get(i) {
                        Some(pt) => self.abi_arg(*pt, v)?,
                        None => v, // variadic tail (e.g. printf) — never aggregate
                    };
                    argvals.push(v.into());
                }
                let cs = b
                    .build_call(fv, &argvals, "call")
                    .map_err(|e| e.to_string())?;
                if let Some(d) = dest {
                    if let Some(v) = cs.try_as_basic_value().left() {
                        vals[*d as usize] = Some(self.abi_result(v)?);
                    }
                }
            }
            Inst::Print { args } => {
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        self.printf_lit(" ")?;
                    }
                    let ty = f.value_ty(*a);
                    let v = get(vals, *a);
                    // `bytes` prints its raw bytes like a `str` (both are ZapStr).
                    if matches!(ty, Ty::Ptr | Ty::Bytes) {
                        let print_str = self.fns["zap_print_str"];
                        b.build_call(print_str, &[v.into()], "")
                            .map_err(|e| e.to_string())?;
                    } else if ty.is_float() {
                        // `printf` is variadic: a float32 must be promoted to
                        // double (what `%f` reads) before the call.
                        let dv = if matches!(ty, Ty::F32) {
                            b.build_float_ext(v.into_float_value(), self.ctx.f64_type(), "fpext")
                                .map_err(|e| e.to_string())?
                                .into()
                        } else {
                            v
                        };
                        self.printf_val("%f", dv)?;
                    } else {
                        let iv = if ty.int_width() == Some(64) {
                            v
                        } else {
                            b.build_int_s_extend(v.into_int_value(), self.ctx.i64_type(), "sext")
                                .map_err(|e| e.to_string())?
                                .into()
                        };
                        self.printf_val("%lld", iv)?;
                    }
                }
                self.printf_lit("\n")?;
            }
            Inst::StrConst { dest, bytes } => {
                // Intern each distinct string literal as ONE never-freed constant
                // global `ZapStr` `{ i64 len, [N x i8] bytes }` (#837, D-0145),
                // matching the runtime's one-object layout (len @0, bytes @8). The
                // literal is no longer re-allocated via `zap_str_new` at each use,
                // so a constant prefix like the `"k"` of `f"k{i}"` costs zero heap
                // allocations per iteration. Interning is transparent — `str` is a
                // value type with no observable identity (`is` on `str` is a compile
                // error), so same bytes sharing one object changes nothing
                // semantically; the static-data pointer is non-heap, so the moving
                // collector leaves it alone and concat/format (which always allocate
                // a fresh result) never mutate or free it.
                let ptr = {
                    let mut cache = self.str_consts.borrow_mut();
                    if let Some(p) = cache.get(bytes.as_slice()) {
                        *p
                    } else {
                        let i8t = self.ctx.i8_type();
                        let arr_vals: Vec<_> = bytes
                            .iter()
                            .map(|byte| i8t.const_int(*byte as u64, false))
                            .collect();
                        let arr = i8t.const_array(&arr_vals);
                        let len = self.ctx.i64_type().const_int(bytes.len() as u64, false);
                        let init = self.ctx.const_struct(&[len.into(), arr.into()], false);
                        let global = self.module.add_global(init.get_type(), None, "str.const");
                        global.set_initializer(&init);
                        global.set_constant(true);
                        global.set_linkage(inkwell::module::Linkage::Internal);
                        let p = global.as_pointer_value();
                        cache.insert(bytes.clone(), p);
                        p
                    }
                };
                vals[*dest as usize] = Some(BasicValueEnum::PointerValue(ptr));
            }
            Inst::StructNew { dest, sid } => {
                let st = self.structs[*sid as usize];
                let cs = if Self::GC_IMMIX {
                    // Precise GC (ticket #48): allocate with the struct's type
                    // descriptor so the collector can trace its pointer fields.
                    #[cfg(feature = "gc-immix")]
                    {
                        let alloc = self.fns["zap_alloc_typed"];
                        b.build_call(
                            alloc,
                            &[self.struct_descs[*sid as usize].into()],
                            "structnew",
                        )
                        .map_err(|e| e.to_string())?
                    }
                    #[cfg(not(feature = "gc-immix"))]
                    unreachable!()
                } else {
                    let size = st
                        .size_of()
                        .ok_or_else(|| "struct has no size".to_string())?;
                    let alloc = self.fns["zap_alloc"];
                    b.build_call(alloc, &[size.into()], "structnew")
                        .map_err(|e| e.to_string())?
                };
                vals[*dest as usize] = cs.try_as_basic_value().left();
            }
            Inst::ArenaNew { dest, sid, arena } => {
                // Scope-bound arena allocation (#553, D-0158): bump `size_of(sid)`
                // leaf bytes out of the arena handle `arena`. Backend-uniform and
                // size-based (the object is leaf / pointer-free, so the atomic
                // descriptor the runtime uses carries no tracing). The returned
                // payload pointer is GC-rooted via `inst_dest`; the precise marker
                // marks it in place (the region is pinned) and never traces into it.
                let st = self.structs[*sid as usize];
                let size = st
                    .size_of()
                    .ok_or_else(|| "arena struct has no size".to_string())?;
                let arena_v = vals[*arena as usize]
                    .ok_or_else(|| "arena handle not materialized".to_string())?;
                let alloc = self.fns["zap_arena_alloc_atomic"];
                let cs = b
                    .build_call(alloc, &[arena_v.into(), size.into()], "arenanew")
                    .map_err(|e| e.to_string())?;
                vals[*dest as usize] = cs.try_as_basic_value().left();
            }
            Inst::StackStruct { dest, sid } => {
                // Escape analysis (#839): a non-escaping, leaf / pointer-free
                // struct lives in an entry-hoisted stack slot (#91) instead of the
                // GC heap, zero-initialized to match `StructNew`'s zeroed instance.
                // The slot holds no outgoing heap pointers, so it is never a GC
                // root (excluded in `gc_setup_frame`) and is never relocated by
                // moving Immix — see `crates/ir/src/escape.rs` / discussion #849.
                let st = self.structs[*sid as usize];
                let ptr = self.entry_alloca(st.into(), "stackstruct")?;
                b.build_store(ptr, st.const_zero())
                    .map_err(|e| e.to_string())?;
                vals[*dest as usize] = Some(BasicValueEnum::PointerValue(ptr));
            }
            Inst::StrBuild {
                dest,
                parts,
                stack,
                // `bounded` is consumed in `gc_setup_frame` (it decides whether
                // this stack build needs a GC root); the emission below is
                // identical either way — the runtime `zap_strbuild_into` simply
                // never takes its spill branch for a bounded join (#1081).
                bounded: _,
                cap,
            } => {
                // One-pass string join (#839 / #837 lever-3(b)). Spill the part
                // pointers into an entry-hoisted `[n x ptr]` array, then call the
                // runtime joiner. `stack` (set by escape analysis) builds into an
                // inline `{i64 len; [cap x i8]}` stack buffer with a heap spill
                // when the summed length exceeds `cap`; otherwise one heap alloc.
                let p = self.ctx.ptr_type(AddressSpace::default());
                let i64t = self.ctx.i64_type();
                let n = parts.len();
                let arr_ty = p.array_type(n as u32);
                let arr = self.entry_alloca(arr_ty.into(), "strparts")?;
                // Index the `[n x ptr]` array with i32 indices, matching the
                // proven GC-roots GEP (an array index wants the aggregate's own
                // index width, not the i64 pointer width).
                let i32t = self.ctx.i32_type();
                let zero = i32t.const_zero();
                for (i, pv) in parts.iter().enumerate() {
                    let idx = i32t.const_int(i as u64, false);
                    let ep = unsafe {
                        b.build_in_bounds_gep(arr_ty, arr, &[zero, idx], "strpart")
                            .map_err(|e| e.to_string())?
                    };
                    b.build_store(ep, get(vals, *pv))
                        .map_err(|e| e.to_string())?;
                }
                let nconst = i64t.const_int(n as u64, false);
                let cs = if *stack {
                    let i8t = self.ctx.i8_type();
                    let buf_ty = self
                        .ctx
                        .struct_type(&[i64t.into(), i8t.array_type(*cap).into()], false);
                    let buf = self.entry_alloca(buf_ty.into(), "strbuf")?;
                    let join = self.fns["zap_strbuild_into"];
                    b.build_call(
                        join,
                        &[
                            buf.into(),
                            i64t.const_int(*cap as u64, false).into(),
                            arr.into(),
                            nconst.into(),
                        ],
                        "strjoin",
                    )
                    .map_err(|e| e.to_string())?
                } else {
                    let join = self.fns["zap_strbuild"];
                    b.build_call(join, &[arr.into(), nconst.into()], "strjoin")
                        .map_err(|e| e.to_string())?
                };
                vals[*dest as usize] = cs.try_as_basic_value().left();
            }
            Inst::FieldGet {
                dest,
                base,
                sid,
                idx,
            } => {
                let st = self.structs[*sid as usize];
                let p = get(vals, *base).into_pointer_value();
                let ty = self.basic(f.value_ty(*dest));
                let v = if let Some(pl) = &self.struct_packed[*sid as usize] {
                    // C-style bitfield read (I#173): shift/mask the backing int.
                    self.packed_field_get(b, p, st, pl.backing, pl.fields[*idx as usize], ty)?
                } else {
                    let fp = b
                        .build_struct_gep(st, p, *idx, "fieldptr")
                        .map_err(|_| "invalid field index".to_string())?;
                    b.build_load(ty, fp, "field").map_err(|e| e.to_string())?
                };
                vals[*dest as usize] = Some(v);
            }
            Inst::FieldSet {
                base,
                sid,
                idx,
                val,
            } => {
                let st = self.structs[*sid as usize];
                let p = get(vals, *base).into_pointer_value();
                if let Some(pl) = &self.struct_packed[*sid as usize] {
                    // C-style bitfield write (I#173): read-clear-or the backing int.
                    self.packed_field_set(
                        b,
                        p,
                        st,
                        pl.backing,
                        pl.fields[*idx as usize],
                        get(vals, *val),
                    )?;
                } else {
                    let fp = b
                        .build_struct_gep(st, p, *idx, "fieldptr")
                        .map_err(|_| "invalid field index".to_string())?;
                    b.build_store(fp, get(vals, *val))
                        .map_err(|e| e.to_string())?;
                }
            }
            // Atomic[T] operations (I#176). Each GEPs the cell field — which sits
            // at offset 0, so the pointer is the box itself — then emits the LLVM
            // atomic instruction over an integer *access type* (bool→i8, int32→i32,
            // int64→i64, a reference payload→iN-word via ptrtoint/inttoptr). The
            // atomic load/store need an explicit alignment (natural for the access
            // type; the box payload is 16-aligned, so field 0 is over-aligned).
            Inst::AtomicLoad {
                dest,
                base,
                sid,
                idx,
                elem,
                order,
            } => {
                let fp = self.atomic_cell_ptr(b, vals, &get, *base, *sid, *idx)?;
                let ai = self.atom_int_ty(*elem);
                let load = b
                    .build_load(ai, fp, "atomic.load")
                    .map_err(|e| e.to_string())?;
                let li = load
                    .as_instruction_value()
                    .ok_or_else(|| "atomic load has no instruction".to_string())?;
                li.set_alignment(ai.get_bit_width() / 8)
                    .map_err(|e| e.to_string())?;
                li.set_atomic_ordering(Self::atom_ord(*order))
                    .map_err(|e| e.to_string())?;
                vals[*dest as usize] = Some(self.atom_from_int(b, load.into_int_value(), *elem)?);
            }
            Inst::AtomicStore {
                base,
                sid,
                idx,
                val,
                elem,
                order,
            } => {
                let fp = self.atomic_cell_ptr(b, vals, &get, *base, *sid, *idx)?;
                let ai = self.atom_int_ty(*elem);
                let iv = self.atom_to_int(b, get(vals, *val), *elem)?;
                let st = b.build_store(fp, iv).map_err(|e| e.to_string())?;
                st.set_alignment(ai.get_bit_width() / 8)
                    .map_err(|e| e.to_string())?;
                st.set_atomic_ordering(Self::atom_ord(*order))
                    .map_err(|e| e.to_string())?;
            }
            Inst::AtomicRmw {
                dest,
                base,
                sid,
                idx,
                op,
                val,
                elem,
                order,
            } => {
                let fp = self.atomic_cell_ptr(b, vals, &get, *base, *sid, *idx)?;
                let iv = self
                    .atom_to_int(b, get(vals, *val), *elem)?
                    .into_int_value();
                let bin = match op {
                    AtomicRmwOp::Add => AtomicRMWBinOp::Add,
                    AtomicRmwOp::Sub => AtomicRMWBinOp::Sub,
                    AtomicRmwOp::Xchg => AtomicRMWBinOp::Xchg,
                };
                let old = b
                    .build_atomicrmw(bin, fp, iv, Self::atom_ord(*order))
                    .map_err(|e| e.to_string())?;
                // `Add`/`Sub` yield the NEW value (Go semantics); `Xchg` the old.
                let result = match op {
                    AtomicRmwOp::Add => b
                        .build_int_add(old, iv, "atomic.new")
                        .map_err(|e| e.to_string())?,
                    AtomicRmwOp::Sub => b
                        .build_int_sub(old, iv, "atomic.new")
                        .map_err(|e| e.to_string())?,
                    AtomicRmwOp::Xchg => old,
                };
                vals[*dest as usize] = Some(self.atom_from_int(b, result, *elem)?);
            }
            Inst::AtomicCas {
                dest,
                base,
                sid,
                idx,
                expected,
                desired,
                elem,
                success,
                failure,
            } => {
                let fp = self.atomic_cell_ptr(b, vals, &get, *base, *sid, *idx)?;
                let exp = self.atom_to_int(b, get(vals, *expected), *elem)?;
                let des = self.atom_to_int(b, get(vals, *desired), *elem)?;
                let cx = b
                    .build_cmpxchg(
                        fp,
                        exp,
                        des,
                        Self::atom_ord(*success),
                        Self::atom_ord(*failure),
                    )
                    .map_err(|e| e.to_string())?;
                // cmpxchg yields `{ iN old, i1 success }`; the bool result is slot 1.
                let ok = b
                    .build_extract_value(cx, 1, "atomic.cas.ok")
                    .map_err(|e| e.to_string())?;
                vals[*dest as usize] = Some(ok);
            }
            Inst::Invoke {
                dest,
                func,
                args,
                unwind,
            } => {
                // A call that may unwind -> LLVM `invoke`. The normal path is a
                // fresh block the builder continues into (so the rest of the ZIR
                // block's instructions and its terminator land there); the unwind
                // edge goes to the landing-pad block.
                let callee = *self
                    .fns
                    .get(func)
                    .ok_or_else(|| format!("invoke of unknown function '{func}'"))?;
                // Same by-value aggregate ABI as `Inst::Call` (D-0051).
                let param_tys = callee.get_type().get_param_types();
                let mut argvals: Vec<BasicValueEnum> = Vec::with_capacity(args.len());
                for (i, a) in args.iter().enumerate() {
                    let v = get(vals, *a);
                    let v = match param_tys.get(i) {
                        Some(pt) => self.abi_arg(*pt, v)?,
                        None => v,
                    };
                    argvals.push(v);
                }
                let cont = self.ctx.append_basic_block(fv, "invoke.cont");
                let cs = b
                    .build_invoke(callee, &argvals, cont, blocks[*unwind as usize], "invoke")
                    .map_err(|e| e.to_string())?;
                b.position_at_end(cont);
                if let Some(d) = dest {
                    if let Some(v) = cs.try_as_basic_value().left() {
                        vals[*d as usize] = Some(self.abi_result(v)?);
                    }
                }
            }
            Inst::LandingPad { dest } => {
                // The landing-pad result is `{ ptr (exception), i32 (selector) }`.
                // The catch-all C++ clause accepts Zap's single transport type;
                // language-level dispatch checks the carried Zap type id in IR.
                // `zap_eh_begin_catch` turns the in-flight `_Unwind_Exception*`
                // into the typed Zap exception object.
                let ptr_ty = self.ctx.ptr_type(AddressSpace::default());
                let lp_ty = self
                    .ctx
                    .struct_type(&[ptr_ty.into(), self.ctx.i32_type().into()], false);
                let null = ptr_ty.const_null();
                let pf = self.fns[self.personality];
                let lp = b
                    .build_landing_pad(lp_ty, pf, &[null.into()], false, "lp")
                    .map_err(|e| e.to_string())?;
                let exc_ptr = b
                    .build_extract_value(lp.into_struct_value(), 0, "excptr")
                    .map_err(|e| e.to_string())?;
                // Precise GC (ticket #48): unwinding tore down the deeper frames
                // without running their ret-unlinks, so the shadow head is stale.
                // Re-establish THIS frame as the head (its prev chain reaches the
                // live caller frames) before any handler/cleanup code — which
                // allocates and hits safepoints — runs. This must come right
                // after the mandatory `landingpad` instruction. Then a safepoint,
                // since the block's top safepoint was skipped.
                #[cfg(feature = "gc-immix")]
                {
                    self.gc_restore_head()?;
                    self.gc_safepoint(fv)?;
                }
                let begin = self.fns["zap_eh_begin_catch"];
                let cs = b
                    .build_call(begin, &[exc_ptr.into()], "begincatch")
                    .map_err(|e| e.to_string())?;
                vals[*dest as usize] = cs.try_as_basic_value().left();
            }
            Inst::ArrayBinOp {
                dest,
                op,
                out,
                lhs,
                lhs_dtype,
                rhs,
                rhs_dtype,
                scalar,
                scalar_lhs,
            } => {
                // Strided-view operands are compacted to a contiguous copy
                // before the SIMD kernel reads them (#502); a scalar `rhs` is a
                // plain value, not an array, so it is left untouched.
                let lv = self.call1("zap_array_contig", &[get(vals, *lhs).into()], "lc")?;
                let rv = if *scalar {
                    get(vals, *rhs)
                } else {
                    self.call1("zap_array_contig", &[get(vals, *rhs).into()], "rc")?
                };
                let res = self.array_binop(
                    *op,
                    *out,
                    *lhs_dtype,
                    *rhs_dtype,
                    lv,
                    rv,
                    *scalar,
                    *scalar_lhs,
                    fv,
                )?;
                vals[*dest as usize] = Some(res);
            }
            Inst::ArrayUnaryOp {
                dest,
                op,
                out,
                src,
                src_dtype,
            } => {
                // Compact a strided view before the unary SIMD kernel (#502).
                let sv = self.call1("zap_array_contig", &[get(vals, *src).into()], "sc")?;
                let res = self.array_unaryop(*op, *out, *src_dtype, sv, fv)?;
                vals[*dest as usize] = Some(res);
            }
            Inst::Spawn {
                func,
                args,
                owner,
                name,
            } => {
                // Launch a coroutine running `func(args...)` (ticket #25,
                // D-0013). The evaluated arguments are copied into a
                // GC-allocated closure whose layout is the callee's parameter
                // tuple; a per-target trampoline `ptr(ptr)` unpacks that closure
                // and calls the function. `zap_spawn` starts a GC-registered OS
                // thread running the trampoline on the closure. This is the one
                // non-mechanical arm (it synthesizes the trampoline), mirroring
                // the ArrayBinOp SIMD lowering above.
                let target = *self
                    .fns
                    .get(func)
                    .ok_or_else(|| format!("spawn of unknown function '{func}'"))?;
                let param_tys = target.get_type().get_param_types();
                let env_struct = self.ctx.struct_type(&param_tys, false);
                let env_cs = if Self::GC_IMMIX {
                    // Precise GC (ticket #48): the closure is kept alive via the
                    // coroutine's G (zap_desc_gobj traces G.arg), and its captured
                    // pointer args are traced via this descriptor. (By-value
                    // aggregate args that themselves embed pointers are a
                    // documented v1 limitation — their inner pointers are not
                    // traced through the closure.)
                    #[cfg(feature = "gc-immix")]
                    {
                        let flags: Vec<bool> =
                            param_tys.iter().map(|t| t.is_pointer_type()).collect();
                        let desc = self.emit_precise_desc(
                            env_struct,
                            &flags,
                            &format!("zap.desc.env.{func}"),
                        );
                        let alloc = self.fns["zap_alloc_typed"];
                        b.build_call(alloc, &[desc.into()], "spawnenv")
                            .map_err(|e| e.to_string())?
                    }
                    #[cfg(not(feature = "gc-immix"))]
                    unreachable!()
                } else {
                    let size = env_struct
                        .size_of()
                        .ok_or_else(|| "spawn closure has no size".to_string())?;
                    let alloc = self.fns["zap_alloc"];
                    b.build_call(alloc, &[size.into()], "spawnenv")
                        .map_err(|e| e.to_string())?
                };
                let env = env_cs
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| "zap_alloc returned no value".to_string())?
                    .into_pointer_value();
                for (i, a) in args.iter().enumerate() {
                    let fp = b
                        .build_struct_gep(env_struct, env, i as u32, "argptr")
                        .map_err(|_| "invalid spawn argument index".to_string())?;
                    // A by-value aggregate argument (inline value struct / tuple,
                    // D-0051) is stored into the GC-allocated closure by value —
                    // an independent copy that outlives the spawning frame — so
                    // load it from its pointer first. The trampoline loads it back
                    // and passes it by value to the target.
                    let v = get(vals, *a);
                    let v = match param_tys.get(i) {
                        Some(pt) => self.abi_arg(*pt, v)?,
                        None => v,
                    };
                    b.build_store(fp, v).map_err(|e| e.to_string())?;
                }
                let thunk = self.spawn_thunk(func, target, env_struct)?;
                let thunk_ptr = thunk.as_global_value().as_pointer_value();
                // Dispatch on ownership (D-0044): root spawns are joined at program
                // exit, scoped spawns are owned by their `scope:` handle, detached
                // spawns are owned by nothing.
                match owner {
                    Owner::Root => {
                        let spawn = self.fns["zap_spawn"];
                        b.build_call(spawn, &[thunk_ptr.into(), env.into()], "")
                            .map_err(|e| e.to_string())?;
                    }
                    Owner::Scope(s) => {
                        let scope_v = get(vals, *s);
                        let spawn = self.fns["zap_spawn_scoped"];
                        // A static C-string naming the spawn target, so a
                        // `ScopeTimeout` can report every outstanding coroutine by
                        // name (D-0058 diagnostics / I#475). A method or
                        // function-value spawn carries a readable `name` so the
                        // report names the user's coroutine rather than the
                        // mangled method symbol or the internal `$spawnthunk$…`.
                        let display = name.as_deref().unwrap_or(func.as_str());
                        let name = b
                            .build_global_string_ptr(display, "spawnname")
                            .map_err(|e| e.to_string())?
                            .as_pointer_value();
                        b.build_call(
                            spawn,
                            &[thunk_ptr.into(), env.into(), scope_v.into(), name.into()],
                            "",
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    Owner::Detached => {
                        let spawn = self.fns["zap_spawn_detached"];
                        b.build_call(spawn, &[thunk_ptr.into(), env.into()], "")
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
            Inst::FuncAddr { dest, func } => {
                // The address of a top-level function as a pointer value, handed
                // to the collection runtime as a struct key's `__hash__`/`__eq__`
                // (operator overloading, D-0047).
                let fv = *self
                    .fns
                    .get(func)
                    .ok_or_else(|| format!("address of unknown function '{func}'"))?;
                let p = fv.as_global_value().as_pointer_value();
                vals[*dest as usize] = Some(p.into());
            }
            Inst::GlobalAddr { dest, idx } => {
                // The address of a module-level global slot (D-0112, #658
                // Phase 1): the LLVM global emitted in `translate`. A module
                // `const` reads it with a following `Load`.
                let p = *self
                    .globals
                    .get(*idx as usize)
                    .ok_or_else(|| format!("address of unknown global #{idx}"))?;
                vals[*dest as usize] = Some(p.into());
            }
            Inst::ClosureMake {
                dest,
                func,
                env_sid,
                captures,
            } => {
                // Build a first-class function value (I#163): a GC-heap closure
                // record `{code_ptr, captures…}`. Field 0 holds the code pointer
                // (the lifted-lambda body or a bare-function wrapper); the rest
                // are the captured free variables. The precise collector scans
                // the record via its type descriptor (the code pointer is
                // non-heap, so it is never traced; captures are traced by their
                // ZIR pointer-kind).
                let code_fn = *self
                    .fns
                    .get(func)
                    .ok_or_else(|| format!("closure of unknown function '{func}'"))?;
                let code_ptr = code_fn.as_global_value().as_pointer_value();
                let env_struct = self.structs[*env_sid as usize];
                let env_cs = if Self::GC_IMMIX {
                    #[cfg(feature = "gc-immix")]
                    {
                        let mut flags = Vec::with_capacity(captures.len() + 1);
                        flags.push(false); // code pointer word: never traced
                        for c in captures {
                            flags.push(f.value_types[*c as usize].is_ptr());
                        }
                        let desc = self.emit_precise_desc(
                            env_struct,
                            &flags,
                            &format!("zap.desc.closure.{env_sid}"),
                        );
                        let alloc = self.fns["zap_alloc_typed"];
                        b.build_call(alloc, &[desc.into()], "closure")
                            .map_err(|e| e.to_string())?
                    }
                    #[cfg(not(feature = "gc-immix"))]
                    unreachable!()
                } else {
                    let size = env_struct
                        .size_of()
                        .ok_or_else(|| "closure record has no size".to_string())?;
                    let alloc = self.fns["zap_alloc"];
                    b.build_call(alloc, &[size.into()], "closure")
                        .map_err(|e| e.to_string())?
                };
                let env = env_cs
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| "closure alloc returned no value".to_string())?
                    .into_pointer_value();
                let code_slot = b
                    .build_struct_gep(env_struct, env, 0, "closure.code")
                    .map_err(|e| e.to_string())?;
                b.build_store(code_slot, code_ptr)
                    .map_err(|e| e.to_string())?;
                for (i, c) in captures.iter().enumerate() {
                    let fp = b
                        .build_struct_gep(env_struct, env, (i + 1) as u32, "closure.cap")
                        .map_err(|_| "invalid closure capture index".to_string())?;
                    let v = get(vals, *c);
                    // A captured by-value aggregate (value struct / tuple) is
                    // stored by value — an independent copy that outlives the
                    // capturing frame — so load it from its pointer first.
                    let v = match env_struct.get_field_type_at_index((i + 1) as u32) {
                        Some(ft) => self.abi_arg(ft, v)?,
                        None => v,
                    };
                    b.build_store(fp, v).map_err(|e| e.to_string())?;
                }
                vals[*dest as usize] = Some(env.into());
            }
            Inst::CallClosure {
                dest,
                callee,
                args,
                params,
                ret,
                unwind,
            } => {
                // Indirect call through a function value (I#163): load the code
                // pointer from the closure record (field 0, at offset 0) and call
                // it, passing the record itself as the implicit env followed by
                // the arguments. The callee type is built from the value's
                // signature.
                let p = self.ctx.ptr_type(AddressSpace::default());
                let closure = get(vals, *callee).into_pointer_value();
                let code = b
                    .build_load(p, closure, "closure.code")
                    .map_err(|e| e.to_string())?
                    .into_pointer_value();
                let mut param_tys: Vec<BasicMetadataTypeEnum> = vec![p.into()];
                for t in params {
                    param_tys.push(self.abi_ty(*t).into());
                }
                let fnty = if *ret == Ty::Void {
                    self.ctx.void_type().fn_type(&param_tys, false)
                } else {
                    self.abi_ty(*ret).fn_type(&param_tys, false)
                };
                let mut argvals: Vec<BasicValueEnum> = Vec::with_capacity(args.len() + 1);
                argvals.push(closure.into());
                for (i, a) in args.iter().enumerate() {
                    let v = get(vals, *a);
                    let v = match params.get(i) {
                        Some(t) => self.abi_arg(self.abi_ty(*t), v)?,
                        None => v,
                    };
                    argvals.push(v);
                }
                if let Some(uw) = unwind {
                    let cont = self.ctx.append_basic_block(fv, "callclosure.cont");
                    let cs = b
                        .build_indirect_invoke(
                            fnty,
                            code,
                            &argvals,
                            cont,
                            blocks[*uw as usize],
                            "callclosure",
                        )
                        .map_err(|e| e.to_string())?;
                    b.position_at_end(cont);
                    if let Some(d) = dest {
                        if let Some(v) = cs.try_as_basic_value().left() {
                            vals[*d as usize] = Some(self.abi_result(v)?);
                        }
                    }
                } else {
                    let md: Vec<BasicMetadataValueEnum> =
                        argvals.iter().map(|v| (*v).into()).collect();
                    let cs = b
                        .build_indirect_call(fnty, code, &md, "callclosure")
                        .map_err(|e| e.to_string())?;
                    if let Some(d) = dest {
                        if let Some(v) = cs.try_as_basic_value().left() {
                            vals[*d as usize] = Some(self.abi_result(v)?);
                        }
                    }
                }
            }
            Inst::IfaceMake {
                dest,
                data,
                iid,
                concrete_sid,
            } => {
                // Build the `{data, itable}` fat pointer on the stack (D-0016,
                // design/0006 §2.4). The ZIR value is a pointer to this inline
                // `{ptr, ptr}` aggregate — the same shape the inline-aggregate ABI
                // uses for a value struct — so it is passed/returned by value at
                // call boundaries and addressed by pointer in the body.
                let fat_sid = self.interfaces[*iid as usize].fat_sid;
                let fat_ty = self.structs[fat_sid as usize];
                let slot = self.entry_alloca(fat_ty.into(), "iface")?;
                let data_ptr = b
                    .build_struct_gep(fat_ty, slot, 0, "iface.data")
                    .map_err(|e| e.to_string())?;
                b.build_store(data_ptr, get(vals, *data))
                    .map_err(|e| e.to_string())?;
                let itable = self.itable_global(*concrete_sid, *iid)?;
                let itable_ptr = b
                    .build_struct_gep(fat_ty, slot, 1, "iface.itable")
                    .map_err(|e| e.to_string())?;
                b.build_store(itable_ptr, itable)
                    .map_err(|e| e.to_string())?;
                vals[*dest as usize] = Some(slot.into());
            }
            Inst::IfaceCall {
                dest,
                iface,
                iid,
                method,
                args,
            } => {
                let (callee, fnty, argvals) =
                    self.iface_dispatch(get(vals, *iface), *iid, *method, args, vals)?;
                let md: Vec<BasicMetadataValueEnum> = argvals.iter().map(|v| (*v).into()).collect();
                let cs = b
                    .build_indirect_call(fnty, callee, &md, "ifacecall")
                    .map_err(|e| e.to_string())?;
                if let Some(d) = dest {
                    if let Some(v) = cs.try_as_basic_value().left() {
                        vals[*d as usize] = Some(self.abi_result(v)?);
                    }
                }
            }
            Inst::IfaceInvoke {
                dest,
                iface,
                iid,
                method,
                args,
                unwind,
            } => {
                let (callee, fnty, argvals) =
                    self.iface_dispatch(get(vals, *iface), *iid, *method, args, vals)?;
                let cont = self.ctx.append_basic_block(fv, "ifaceinvoke.cont");
                let cs = b
                    .build_indirect_invoke(
                        fnty,
                        callee,
                        &argvals,
                        cont,
                        blocks[*unwind as usize],
                        "ifaceinvoke",
                    )
                    .map_err(|e| e.to_string())?;
                b.position_at_end(cont);
                if let Some(d) = dest {
                    if let Some(v) = cs.try_as_basic_value().left() {
                        vals[*d as usize] = Some(self.abi_result(v)?);
                    }
                }
            }
            Inst::IfaceRender { dest, iface, iid } => {
                // Render an interface value (#707, D-0124): load the renderer slot
                // from the itable and call the concrete type's renderer with `data`
                // as `self`, yielding a `str`.
                let (callee, fnty, selfp) = self.iface_render_dispatch(get(vals, *iface), *iid)?;
                let md: Vec<BasicMetadataValueEnum> = vec![selfp.into()];
                let cs = b
                    .build_indirect_call(fnty, callee, &md, "ifacerender")
                    .map_err(|e| e.to_string())?;
                if let Some(v) = cs.try_as_basic_value().left() {
                    vals[*dest as usize] = Some(v);
                }
            }
            Inst::IfaceRenderInvoke {
                dest,
                iface,
                iid,
                unwind,
            } => {
                // The exception-edge form of `IfaceRender`: the concrete renderer
                // may raise (a user `__str__`, a `bytes` field decode), so dispatch
                // through an `invoke` to the active landing pad.
                let (callee, fnty, selfp) = self.iface_render_dispatch(get(vals, *iface), *iid)?;
                let cont = self.ctx.append_basic_block(fv, "ifacerenderinvoke.cont");
                let cs = b
                    .build_indirect_invoke(
                        fnty,
                        callee,
                        &[selfp],
                        cont,
                        blocks[*unwind as usize],
                        "ifacerenderinvoke",
                    )
                    .map_err(|e| e.to_string())?;
                b.position_at_end(cont);
                if let Some(v) = cs.try_as_basic_value().left() {
                    vals[*dest as usize] = Some(v);
                }
            }
        }
        Ok(())
    }

    /// The static interface dispatch table for the `(concrete_sid, iid)` pair
    /// (D-0016, extended by #707): a constant `[N+1 x ptr]` global whose slot *k*
    /// (`k < N`) is the address of the implementor's `Struct__<method_k>` in
    /// interface declaration order, and whose final slot *N* is the concrete
    /// type's **Display renderer** (#707, D-0124) — the function
    /// `self.iface_renderers[concrete_sid]` that `Inst::IfaceRender` dispatches to
    /// when a type-erased interface value is rendered. A concrete type with no
    /// recorded renderer (not reachable for the field types D-0124 supports) gets
    /// a null renderer slot. Built once per pair and cached.
    fn itable_global(&self, concrete_sid: u32, iid: u32) -> Result<PointerValue<'ctx>, String> {
        if let Some(g) = self.itables.borrow().get(&(concrete_sid, iid)) {
            return Ok(*g);
        }
        let iface = &self.interfaces[iid as usize];
        let concrete = &self.struct_names[concrete_sid as usize];
        let p = self.ctx.ptr_type(AddressSpace::default());
        let mut slots: Vec<PointerValue<'ctx>> = Vec::with_capacity(iface.methods.len() + 1);
        for m in &iface.methods {
            let mangled = format!("{concrete}__{}", m.name);
            let fv = *self.fns.get(&mangled).ok_or_else(|| {
                format!(
                    "interface '{}' method '{}' not implemented by '{concrete}' (no '{mangled}')",
                    iface.name, m.name
                )
            })?;
            slots.push(fv.as_global_value().as_pointer_value());
        }
        // Renderer slot (#707, D-0124): the concrete type's Display renderer, or a
        // null pointer when none was synthesized (an unrenderable concrete type —
        // not reachable through the field types D-0124 supports; a render of such
        // an interface value would be the documented gap, never a miscompile of
        // method dispatch).
        let renderer = match self.iface_renderers.get(&concrete_sid) {
            Some(sym) => self
                .fns
                .get(sym)
                .map(|fv| fv.as_global_value().as_pointer_value())
                .unwrap_or_else(|| p.const_null()),
            None => p.const_null(),
        };
        slots.push(renderer);
        let arr = p.const_array(&slots);
        let g = self.module.add_global(
            arr.get_type(),
            None,
            &format!("itable.{concrete}.{}", iface.name),
        );
        g.set_initializer(&arr);
        g.set_constant(true);
        let ptr = g.as_pointer_value();
        self.itables.borrow_mut().insert((concrete_sid, iid), ptr);
        Ok(ptr)
    }

    /// Resolve a dynamic dispatch through an interface value (D-0016): load the
    /// method's function pointer from the fat pointer's itable and its `data`
    /// word (the `self` receiver), and build the callee's LLVM function type from
    /// the interface method's signature. Returns `(callee_ptr, fn_type, args)`
    /// ready for an indirect call/invoke; `args[0]` is `self` (`data`).
    fn iface_dispatch(
        &self,
        iface_val: BasicValueEnum<'ctx>,
        iid: u32,
        method: u32,
        args: &[u32],
        vals: &[Option<BasicValueEnum<'ctx>>],
    ) -> Result<
        (
            PointerValue<'ctx>,
            inkwell::types::FunctionType<'ctx>,
            Vec<BasicValueEnum<'ctx>>,
        ),
        String,
    > {
        let b = self.builder;
        let p = self.ctx.ptr_type(AddressSpace::default());
        let iface = &self.interfaces[iid as usize];
        let fat_ty = self.structs[iface.fat_sid as usize];
        let msig = &iface.methods[method as usize];
        let base = iface_val.into_pointer_value();
        // Load `data` (field 0) and `itable` (field 1) from the fat pointer.
        let data_slot = b
            .build_struct_gep(fat_ty, base, 0, "iface.data")
            .map_err(|e| e.to_string())?;
        let data = b
            .build_load(p, data_slot, "iface.self")
            .map_err(|e| e.to_string())?;
        let itable_slot = b
            .build_struct_gep(fat_ty, base, 1, "iface.itable")
            .map_err(|e| e.to_string())?;
        let itable = b
            .build_load(p, itable_slot, "iface.vtbl")
            .map_err(|e| e.to_string())?
            .into_pointer_value();
        // Load slot `method` of the itable (`[N+1 x ptr]`: N method slots + the
        // #707 renderer slot).
        let arr_ty = p.array_type(iface.methods.len() as u32 + 1);
        let fn_slot = unsafe {
            b.build_in_bounds_gep(
                arr_ty,
                itable,
                &[
                    self.ctx.i32_type().const_zero(),
                    self.ctx.i32_type().const_int(method as u64, false),
                ],
                "iface.slot",
            )
            .map_err(|e| e.to_string())?
        };
        let callee = b
            .build_load(p, fn_slot, "iface.fn")
            .map_err(|e| e.to_string())?
            .into_pointer_value();
        // Build the callee type `(ptr self, abi(params)...) -> abi(ret)` from the
        // interface method signature — every implementor matches it.
        let mut param_tys: Vec<BasicMetadataTypeEnum> = vec![p.into()];
        for pt in &msig.params {
            param_tys.push(self.abi_ty(*pt).into());
        }
        let fnty = match msig.ret {
            Ty::Void => self.ctx.void_type().fn_type(&param_tys, false),
            rt => self.abi_ty(rt).fn_type(&param_tys, false),
        };
        // Arguments: `self` (data) first, then the call arguments, each loaded by
        // value when the parameter is an inline aggregate (same ABI as `Call`).
        let mut argvals: Vec<BasicValueEnum> = Vec::with_capacity(args.len() + 1);
        argvals.push(data);
        for (i, a) in args.iter().enumerate() {
            let v = vals[*a as usize].expect("ZIR value used before definition");
            let v = self.gc_reload_operand(*a, v);
            let v = match msig.params.get(i) {
                Some(pt) => self.abi_arg(self.abi_ty(*pt), v)?,
                None => v,
            };
            argvals.push(v);
        }
        Ok((callee, fnty, argvals))
    }

    /// Resolve the Display-protocol render dispatch through an interface value
    /// (#707, D-0124): load the **renderer slot** — the slot past the `N` method
    /// slots — from the fat pointer's itable and its `data` word (the `self`
    /// receiver). Returns `(callee_ptr, fn_type, self)` ready for an indirect
    /// call/invoke; the renderer's LLVM type is `(ptr self) -> ptr`, matching the
    /// synthesized per-type renderer function (whose `self` is a reference-struct
    /// pointer, exactly as a dispatched method's `self` is). The counterpart of
    /// [`Self::iface_dispatch`] for the type-erased render path.
    fn iface_render_dispatch(
        &self,
        iface_val: BasicValueEnum<'ctx>,
        iid: u32,
    ) -> Result<
        (
            PointerValue<'ctx>,
            inkwell::types::FunctionType<'ctx>,
            BasicValueEnum<'ctx>,
        ),
        String,
    > {
        let b = self.builder;
        let p = self.ctx.ptr_type(AddressSpace::default());
        let iface = &self.interfaces[iid as usize];
        let fat_ty = self.structs[iface.fat_sid as usize];
        let base = iface_val.into_pointer_value();
        // Load `data` (field 0) and `itable` (field 1) from the fat pointer.
        let data_slot = b
            .build_struct_gep(fat_ty, base, 0, "iface.data")
            .map_err(|e| e.to_string())?;
        let data = b
            .build_load(p, data_slot, "iface.self")
            .map_err(|e| e.to_string())?;
        let itable_slot = b
            .build_struct_gep(fat_ty, base, 1, "iface.itable")
            .map_err(|e| e.to_string())?;
        let itable = b
            .build_load(p, itable_slot, "iface.vtbl")
            .map_err(|e| e.to_string())?
            .into_pointer_value();
        // The renderer slot is the last slot of the `[N+1 x ptr]` itable, index N.
        let arr_ty = p.array_type(iface.methods.len() as u32 + 1);
        let fn_slot = unsafe {
            b.build_in_bounds_gep(
                arr_ty,
                itable,
                &[
                    self.ctx.i32_type().const_zero(),
                    self.ctx
                        .i32_type()
                        .const_int(iface.methods.len() as u64, false),
                ],
                "iface.render.slot",
            )
            .map_err(|e| e.to_string())?
        };
        let callee = b
            .build_load(p, fn_slot, "iface.render.fn")
            .map_err(|e| e.to_string())?
            .into_pointer_value();
        // The renderer's type is `(ptr self) -> ptr str`.
        let fnty = p.fn_type(&[p.into()], false);
        Ok((callee, fnty, data))
    }

    /// Build (once per target function) the spawn trampoline `ptr @__zap_thunk_F(ptr env)`
    /// that unpacks the closure `env` (laid out as `F`'s parameter tuple) and
    /// calls `F`, returning null. Reused across spawn sites of the same target.
    /// Saves and restores the builder's insert position so it can be called
    /// mid-translation of another function.
    fn spawn_thunk(
        &self,
        func: &str,
        target: FunctionValue<'ctx>,
        env_struct: inkwell::types::StructType<'ctx>,
    ) -> Result<FunctionValue<'ctx>, String> {
        let name = format!("__zap_thunk_{func}");
        if let Some(existing) = self.module.get_function(&name) {
            return Ok(existing);
        }
        let b = self.builder;
        let saved = b.get_insert_block();
        let ptr = self.ctx.ptr_type(AddressSpace::default());
        let thunk = self
            .module
            .add_function(&name, ptr.fn_type(&[ptr.into()], false), None);
        // The thunk is each coroutine's base frame (ticket #47,
        // design/0005-concurrency.md "Interaction with exception unwinding"):
        // unwinding must stop here, not run off the green stack into the
        // makecontext bootstrap. So the call to the target is an `invoke` with a
        // catch-all landing pad that routes the uncaught exception to
        // `zap_g_uncaught`, which decides at runtime based on the coroutine's
        // owning scope (ticket #50, D-0044): a scoped coroutine records its
        // failure on the scope (which re-raises it — possibly as an aggregate
        // `ExceptionGroup` — at its dedent) and returns here so the coroutine ends
        // cleanly; a root/detached coroutine aborts the program with the message
        // (exit 1), exactly as `zap_uncaught` did before. In-coroutine `try/except`
        // is unaffected (it unwinds through normal CFI on the green stack); this
        // only catches what reaches the top. Requires the Itanium personality + an
        // unwind table.
        let uwtable = Attribute::get_named_enum_kind_id("uwtable");
        if uwtable != 0 {
            thunk.add_attribute(
                AttributeLoc::Function,
                self.ctx.create_enum_attribute(uwtable, 2),
            );
        }
        thunk.set_personality_function(self.fns[self.personality]);
        let entry = self.ctx.append_basic_block(thunk, "entry");
        b.position_at_end(entry);
        let env = thunk
            .get_nth_param(0)
            .ok_or_else(|| "thunk has no env parameter".to_string())?
            .into_pointer_value();
        let param_tys = target.get_type().get_param_types();
        let mut call_args: Vec<BasicValueEnum> = Vec::with_capacity(param_tys.len());
        for (i, pty) in param_tys.iter().enumerate() {
            let fp = b
                .build_struct_gep(env_struct, env, i as u32, "argptr")
                .map_err(|_| "invalid thunk argument index".to_string())?;
            let v = b.build_load(*pty, fp, "arg").map_err(|e| e.to_string())?;
            call_args.push(v);
        }
        let cont = self.ctx.append_basic_block(thunk, "cont");
        let pad = self.ctx.append_basic_block(thunk, "unwind");
        b.build_invoke(target, &call_args, cont, pad, "")
            .map_err(|e| e.to_string())?;
        // Normal path: the coroutine returned; hand `null` back to `zap_spawn`.
        b.position_at_end(cont);
        b.build_return(Some(&ptr.const_null()))
            .map_err(|e| e.to_string())?;
        // Unwind path: an exception reached the base frame -> report + exit.
        b.position_at_end(pad);
        let lp_ty = self
            .ctx
            .struct_type(&[ptr.into(), self.ctx.i32_type().into()], false);
        let lp = b
            .build_landing_pad(
                lp_ty,
                self.fns[self.personality],
                &[ptr.const_null().into()],
                false,
                "lp",
            )
            .map_err(|e| e.to_string())?;
        let exc_ptr = b
            .build_extract_value(lp.into_struct_value(), 0, "excptr")
            .map_err(|e| e.to_string())?;
        let exc = b
            .build_call(
                self.fns["zap_eh_begin_catch"],
                &[exc_ptr.into()],
                "begincatch",
            )
            .map_err(|e| e.to_string())?
            .try_as_basic_value()
            .left()
            .ok_or_else(|| "zap_eh_begin_catch returned no value".to_string())?;
        // Route to the scope-aware handler. For a scoped coroutine this records
        // the failure and returns; for a root/detached one it exits. Either way we
        // then balance the `__cxa_begin_catch` and end the coroutine by returning
        // null (the return is dead for the root/detached abort path, but required
        // for the scoped path where `zap_g_uncaught` returns).
        b.build_call(self.fns["zap_g_uncaught"], &[exc.into()], "")
            .map_err(|e| e.to_string())?;
        b.build_call(self.fns["zap_eh_end_catch"], &[], "")
            .map_err(|e| e.to_string())?;
        b.build_return(Some(&ptr.const_null()))
            .map_err(|e| e.to_string())?;
        if let Some(bb) = saved {
            b.position_at_end(bb);
        }
        Ok(thunk)
    }

    /// Lower an element-wise array op to a SIMD loop (tickets #24, #49): a
    /// vector body that processes `VW` lanes per iteration with explicit LLVM
    /// vector load/arith/store, followed by a scalar remainder loop for the
    /// final `len % VW` elements. `lhs` is the source array pointer; `rhs` is
    /// either a second array pointer (`!scalar`) or a scalar broadcast across
    /// all lanes (`scalar`). Returns the freshly allocated result array pointer.
    ///
    /// Mixed-dtype promotion (#49): each operand is loaded at its own storage
    /// type (`lhs_dtype` / `rhs_dtype`), converted to the result dtype `out`
    /// (widen/int-to-float for arrays, and a possible narrowing cast for a weak
    /// scalar), then the arithmetic and store happen in `out`. This is the
    /// deliberate exception to the "mechanical" rule — the whole point of the
    /// ticket is that the arithmetic vectorizes.
    ///
    /// `scalar_lhs` (#506): for a non-commutative scalar-on-left op (`s - a`,
    /// `s / a`) the scalar still arrives in `rhs`, but the per-lane arithmetic
    /// is computed with the operands swapped (`scalar <op> array[i]`). `Div` is
    /// NumPy true division, so `out` is always a float dtype and a zero element
    /// yields IEEE ±inf/nan (no raise).
    #[allow(clippy::too_many_arguments)]
    fn array_binop(
        &self,
        op: zir::BinOp,
        out: zir::Dtype,
        lhs_dtype: zir::Dtype,
        rhs_dtype: zir::Dtype,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        scalar: bool,
        scalar_lhs: bool,
        fv: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let b = self.builder;
        let i64t = self.ctx.i64_type();
        let i32t = self.ctx.i32_type();
        let la = lhs.into_pointer_value();

        // Result allocation. A scalar broadcast keeps the array operand's shape
        // (`new_like`); an array-array op uses the *broadcast* shape (#501,
        // D-0165) — rank = max rank, each dim the larger aligned dim. Broadcast
        // compatibility is validated by a throwing check the lowering emits
        // before this call, so `broadcast_new` may assume the shapes are
        // compatible. `n` is the result element count. The element buffers come
        // back as raw pointers via `zap_array_data` (already past each array's
        // `off`), so the kernels never touch the array header layout.
        let code = i64t.const_int(out.code() as u64, false);
        let res = if scalar {
            self.call1("zap_array_new_like", &[la.into(), code.into()], "arr")?
                .into_pointer_value()
        } else {
            let ra = rhs.into_pointer_value();
            self.call1(
                "zap_array_broadcast_new",
                &[la.into(), ra.into(), code.into()],
                "arr",
            )?
            .into_pointer_value()
        };
        let n = self
            .call1("zap_array_count", &[res.into()], "n")?
            .into_int_value();
        let lp = self
            .call1("zap_array_data", &[la.into()], "lp")?
            .into_pointer_value();
        let op_ptr = self
            .call1("zap_array_data", &[res.into()], "op")?
            .into_pointer_value();
        let rp = if scalar {
            None
        } else {
            Some(
                self.call1("zap_array_data", &[rhs.into_pointer_value().into()], "rp")?
                    .into_pointer_value(),
            )
        };

        let out_float = out.is_float();
        let out_elem = self.basic(out.storage_ty());
        let out_align = out.elem_size();
        let lhs_elem = self.basic(lhs_dtype.storage_ty());
        let lhs_align = lhs_dtype.elem_size();
        let rhs_elem = self.basic(rhs_dtype.storage_ty());
        let rhs_align = rhs_dtype.elem_size();
        const VW: u64 = 4;
        let out_vec = self.dtype_vec(out, VW as u32);
        let lhs_vec = self.dtype_vec(lhs_dtype, VW as u32);
        let rhs_vec = self.dtype_vec(rhs_dtype, VW as u32);

        // The weak scalar crosses the ABI as its `rhs_dtype` word (`i64`/`f64`);
        // convert it once to the result dtype (possibly a narrowing `i64 -> u8`)
        // and broadcast it into a lane vector for the vector body.
        let (sscalar, svec) = if scalar {
            let s = self.convert_scalar(rhs, rhs_dtype, out, out_elem)?;
            let mut v = out_vec.get_undef();
            for lane in 0..VW {
                let idx = i32t.const_int(lane, false);
                v = b
                    .build_insert_element(v, s, idx, "splat")
                    .map_err(|e| e.to_string())?;
            }
            (Some(s), Some(v))
        } else {
            (None, None)
        };

        // The shared exit block for whichever kernel runs.
        let done = self.ctx.append_basic_block(fv, "arr.done");

        // For an array-array op, pick the kernel at run time: the flat SIMD loop
        // when the operands already line up element-for-element (equal shapes —
        // the common case, no regression), else a strided broadcast loop that
        // re-reads stretched/missing axes. The flat path then falls through from
        // `flat_bb`. Inner-axis SIMD for the broadcast path is a follow-up; it is
        // the scalar loop below for now.
        if !scalar {
            let ra = rhs.into_pointer_value();
            let rp = rp.unwrap();
            let same = self
                .call1("zap_array_same_shape", &[la.into(), ra.into()], "same")?
                .into_int_value();
            let zero64 = i64t.const_int(0, false);
            let is_same = b
                .build_int_compare(IntPredicate::NE, same, zero64, "issame")
                .map_err(|e| e.to_string())?;
            let flat_bb = self.ctx.append_basic_block(fv, "arr.flat");
            let bcast_bb = self.ctx.append_basic_block(fv, "arr.bcast");
            b.build_conditional_branch(is_same, flat_bb, bcast_bb)
                .map_err(|e| e.to_string())?;

            // --- vectorized broadcast kernel (#896) ---
            // Walk the result one inner row at a time: `bcast_offsets` gives each
            // operand's base offset for the row (correct for any rank), then the
            // INNERMOST axis is vectorized — a contiguous operand is vector-loaded,
            // a stretched (size-1) one is splat across the lanes (its value is
            // constant across the row). Each operand's inner step is 0 (stretched)
            // or its stride (contiguous), computed once; the kernel is specialized
            // on the two booleans so the hot inner loop carries no per-lane branch
            // and no per-element offset call.
            b.position_at_end(bcast_bb);
            let rnd = self
                .call1("zap_array_ndim", &[res.into()], "rnd")?
                .into_int_value();
            let last_ax = b
                .build_int_sub(rnd, i64t.const_int(1, false), "lastax")
                .map_err(|e| e.to_string())?;
            let inner = self
                .call1("zap_array_dim", &[res.into(), last_ax.into()], "inner")?
                .into_int_value();
            // A zero-length result axis means an empty result — nothing to do, and
            // guards the `n / inner` below against a zero divisor.
            let inner_pos = b
                .build_int_compare(
                    IntPredicate::SGT,
                    inner,
                    i64t.const_int(0, false),
                    "innerpos",
                )
                .map_err(|e| e.to_string())?;
            let bc_go = self.ctx.append_basic_block(fv, "arr.bc.go");
            b.build_conditional_branch(inner_pos, bc_go, done)
                .map_err(|e| e.to_string())?;
            b.position_at_end(bc_go);
            let lstep = self
                .call1(
                    "zap_array_bcast_inner_stride",
                    &[la.into(), res.into()],
                    "lstep",
                )?
                .into_int_value();
            let rstep = self
                .call1(
                    "zap_array_bcast_inner_stride",
                    &[ra.into(), res.into()],
                    "rstep",
                )?
                .into_int_value();
            let zero64 = i64t.const_int(0, false);
            let lc = b
                .build_int_compare(IntPredicate::NE, lstep, zero64, "lc")
                .map_err(|e| e.to_string())?;
            let rc = b
                .build_int_compare(IntPredicate::NE, rstep, zero64, "rc")
                .map_err(|e| e.to_string())?;
            // Four specializations keyed on (lhs contiguous?, rhs contiguous?).
            let e_tt = self.ctx.append_basic_block(fv, "arr.bc.tt");
            let e_tf = self.ctx.append_basic_block(fv, "arr.bc.tf");
            let e_ft = self.ctx.append_basic_block(fv, "arr.bc.ft");
            let e_ff = self.ctx.append_basic_block(fv, "arr.bc.ff");
            let on_lc = self.ctx.append_basic_block(fv, "arr.bc.lc");
            let on_nlc = self.ctx.append_basic_block(fv, "arr.bc.nlc");
            b.build_conditional_branch(lc, on_lc, on_nlc)
                .map_err(|e| e.to_string())?;
            b.position_at_end(on_lc);
            b.build_conditional_branch(rc, e_tt, e_tf)
                .map_err(|e| e.to_string())?;
            b.position_at_end(on_nlc);
            b.build_conditional_branch(rc, e_ft, e_ff)
                .map_err(|e| e.to_string())?;
            let bk = BcastKernel {
                fv,
                la,
                ra,
                res,
                n,
                inner,
                lp,
                rp,
                op_ptr,
                op,
                out_float,
                scalar_lhs,
                out,
                out_elem,
                out_vec,
                out_align,
                lhs_dtype,
                lhs_elem,
                lhs_vec,
                lhs_align,
                rhs_dtype,
                rhs_elem,
                rhs_vec,
                rhs_align,
                done,
            };
            b.position_at_end(e_tt);
            self.emit_bcast_kernel(&bk, true, true)?;
            b.position_at_end(e_tf);
            self.emit_bcast_kernel(&bk, true, false)?;
            b.position_at_end(e_ft);
            self.emit_bcast_kernel(&bk, false, true)?;
            b.position_at_end(e_ff);
            self.emit_bcast_kernel(&bk, false, false)?;

            // The flat loop below runs in `flat_bb`.
            b.position_at_end(flat_bb);
        }

        // vend = n & ~(VW-1): the largest multiple of VW not exceeding n.
        let mask = i64t.const_int(!(VW - 1), false);
        let vend = b.build_and(n, mask, "vend").map_err(|e| e.to_string())?;

        let entry = b.get_insert_block().unwrap();
        let vhead = self.ctx.append_basic_block(fv, "arr.vhead");
        let vbody = self.ctx.append_basic_block(fv, "arr.vbody");
        let rhead = self.ctx.append_basic_block(fv, "arr.rhead");
        let rbody = self.ctx.append_basic_block(fv, "arr.rbody");
        b.build_unconditional_branch(vhead)
            .map_err(|e| e.to_string())?;

        // --- vector loop ---
        b.position_at_end(vhead);
        let i_phi = b.build_phi(i64t, "i").map_err(|e| e.to_string())?;
        let i = i_phi.as_basic_value().into_int_value();
        let vcond = b
            .build_int_compare(IntPredicate::SLT, i, vend, "vcond")
            .map_err(|e| e.to_string())?;
        b.build_conditional_branch(vcond, vbody, rhead)
            .map_err(|e| e.to_string())?;

        b.position_at_end(vbody);
        let lvec = self.vec_load(lhs_vec, lhs_elem, lp, i, lhs_align, "lv")?;
        let lvec = self.convert_vec(lvec, lhs_dtype, out, out_vec)?;
        let rvec = match rp {
            Some(rp) => {
                let rv = self.vec_load(rhs_vec, rhs_elem, rp, i, rhs_align, "rv")?;
                self.convert_vec(rv, rhs_dtype, out, out_vec)?
            }
            None => svec.unwrap(),
        };
        // For a non-commutative scalar-on-left op the scalar (`rvec`) is the
        // left arithmetic operand: `scalar <op> array[i]` (#506).
        let (vl, vr) = if scalar_lhs {
            (rvec, lvec)
        } else {
            (lvec, rvec)
        };
        let resvec = self.vec_arith(op, out_float, vl, vr)?;
        let optr = unsafe {
            b.build_gep(out_elem, op_ptr, &[i], "optr")
                .map_err(|e| e.to_string())?
        };
        let st = b.build_store(optr, resvec).map_err(|e| e.to_string())?;
        st.set_alignment(out_align).map_err(|e| e.to_string())?;
        let inext = b
            .build_int_add(i, i64t.const_int(VW, false), "inext")
            .map_err(|e| e.to_string())?;
        b.build_unconditional_branch(vhead)
            .map_err(|e| e.to_string())?;
        let zero = i64t.const_int(0, false);
        i_phi.add_incoming(&[(&zero, entry), (&inext, vbody)]);

        // --- scalar remainder loop ---
        b.position_at_end(rhead);
        let j_phi = b.build_phi(i64t, "j").map_err(|e| e.to_string())?;
        let j = j_phi.as_basic_value().into_int_value();
        let rcond = b
            .build_int_compare(IntPredicate::SLT, j, n, "rcond")
            .map_err(|e| e.to_string())?;
        b.build_conditional_branch(rcond, rbody, done)
            .map_err(|e| e.to_string())?;

        b.position_at_end(rbody);
        let lsc = self.elem_load(lhs_elem, lp, j, "ls")?;
        let lsc = self.convert_scalar(lsc, lhs_dtype, out, out_elem)?;
        let rsc = match rp {
            Some(rp) => {
                let rv = self.elem_load(rhs_elem, rp, j, "rs")?;
                self.convert_scalar(rv, rhs_dtype, out, out_elem)?
            }
            None => sscalar.unwrap(),
        };
        let (sl, sr) = if scalar_lhs { (rsc, lsc) } else { (lsc, rsc) };
        let ressc = self.scalar_arith(op, out_float, sl, sr)?;
        let optr = unsafe {
            b.build_gep(out_elem, op_ptr, &[j], "optrs")
                .map_err(|e| e.to_string())?
        };
        let st = b.build_store(optr, ressc).map_err(|e| e.to_string())?;
        st.set_alignment(out_align).map_err(|e| e.to_string())?;
        let jnext = b
            .build_int_add(j, i64t.const_int(1, false), "jnext")
            .map_err(|e| e.to_string())?;
        b.build_unconditional_branch(rhead)
            .map_err(|e| e.to_string())?;
        j_phi.add_incoming(&[(&vend, vhead), (&jnext, rbody)]);

        b.position_at_end(done);
        Ok(res.into())
    }

    /// Splat `scalar` across all `vw` lanes of `vec_ty` (a chain of
    /// `insertelement`s, which the backend folds to a broadcast).
    fn splat_vec(
        &self,
        vec_ty: VectorType<'ctx>,
        scalar: BasicValueEnum<'ctx>,
        vw: u64,
    ) -> Result<VectorValue<'ctx>, String> {
        let i32t = self.ctx.i32_type();
        let mut v = vec_ty.get_undef();
        for lane in 0..vw {
            v = self
                .builder
                .build_insert_element(v, scalar, i32t.const_int(lane, false), "splat")
                .map_err(|e| e.to_string())?;
        }
        Ok(v)
    }

    /// Emit one inner-axis specialization of the broadcast kernel (#896),
    /// positioned at its entry block. `lcontig`/`rcontig` say whether each
    /// operand is contiguous on the result's innermost axis (vector-load per
    /// lane) or stretched there (a per-row constant, splat across the lanes).
    /// Walks the result row by row — `bcast_offsets` gives each operand's base
    /// offset for the row (any rank), then the inner axis runs a `VW`-lane vector
    /// body + scalar tail — and finally branches to `bk.done`.
    fn emit_bcast_kernel(
        &self,
        bk: &BcastKernel<'ctx>,
        lcontig: bool,
        rcontig: bool,
    ) -> Result<(), String> {
        let b = self.builder;
        let i64t = self.ctx.i64_type();
        const VW: u64 = 4;
        let entry = b.get_insert_block().unwrap();
        let oa_slot = b.build_alloca(i64t, "bk.oa").map_err(|e| e.to_string())?;
        let ob_slot = b.build_alloca(i64t, "bk.ob").map_err(|e| e.to_string())?;
        let nrows = b
            .build_int_signed_div(bk.n, bk.inner, "bk.nrows")
            .map_err(|e| e.to_string())?;
        let mask = i64t.const_int(!(VW - 1), false);
        let vend = b
            .build_and(bk.inner, mask, "bk.vend")
            .map_err(|e| e.to_string())?;
        let rhead = self.ctx.append_basic_block(bk.fv, "bk.rhead");
        let rbody = self.ctx.append_basic_block(bk.fv, "bk.rbody");
        b.build_unconditional_branch(rhead)
            .map_err(|e| e.to_string())?;

        // --- per-row loop ---
        b.position_at_end(rhead);
        let r_phi = b.build_phi(i64t, "bk.r").map_err(|e| e.to_string())?;
        let r = r_phi.as_basic_value().into_int_value();
        let rcond = b
            .build_int_compare(IntPredicate::SLT, r, nrows, "bk.rcond")
            .map_err(|e| e.to_string())?;
        b.build_conditional_branch(rcond, rbody, bk.done)
            .map_err(|e| e.to_string())?;

        b.position_at_end(rbody);
        let rowbase = b
            .build_int_mul(r, bk.inner, "bk.rowbase")
            .map_err(|e| e.to_string())?;
        b.build_call(
            self.fns["zap_array_bcast_offsets"],
            &[
                bk.la.into(),
                bk.ra.into(),
                bk.res.into(),
                rowbase.into(),
                oa_slot.into(),
                ob_slot.into(),
            ],
            "",
        )
        .map_err(|e| e.to_string())?;
        let oa = b
            .build_load(i64t, oa_slot, "bk.oaL")
            .map_err(|e| e.to_string())?
            .into_int_value();
        let ob = b
            .build_load(i64t, ob_slot, "bk.obL")
            .map_err(|e| e.to_string())?
            .into_int_value();

        // A stretched operand's value is constant across the row: load + splat it
        // once (both the lane vector for the body and the scalar for the tail).
        let l_splat = if lcontig {
            None
        } else {
            let s = self.elem_load(bk.lhs_elem, bk.lp, oa, "bk.lspl")?;
            let s = self.convert_scalar(s, bk.lhs_dtype, bk.out, bk.out_elem)?;
            Some((s, self.splat_vec(bk.out_vec, s, VW)?))
        };
        let r_splat = if rcontig {
            None
        } else {
            let s = self.elem_load(bk.rhs_elem, bk.rp, ob, "bk.rspl")?;
            let s = self.convert_scalar(s, bk.rhs_dtype, bk.out, bk.out_elem)?;
            Some((s, self.splat_vec(bk.out_vec, s, VW)?))
        };

        let ivhead = self.ctx.append_basic_block(bk.fv, "bk.ivhead");
        let ivbody = self.ctx.append_basic_block(bk.fv, "bk.ivbody");
        let ithead = self.ctx.append_basic_block(bk.fv, "bk.ithead");
        let itbody = self.ctx.append_basic_block(bk.fv, "bk.itbody");
        let rcont = self.ctx.append_basic_block(bk.fv, "bk.rcont");
        b.build_unconditional_branch(ivhead)
            .map_err(|e| e.to_string())?;

        // --- inner vector loop ---
        b.position_at_end(ivhead);
        let k_phi = b.build_phi(i64t, "bk.k").map_err(|e| e.to_string())?;
        let k = k_phi.as_basic_value().into_int_value();
        let vcond = b
            .build_int_compare(IntPredicate::SLT, k, vend, "bk.vcond")
            .map_err(|e| e.to_string())?;
        b.build_conditional_branch(vcond, ivbody, ithead)
            .map_err(|e| e.to_string())?;
        b.position_at_end(ivbody);
        let lvec = if lcontig {
            let idx = b
                .build_int_add(oa, k, "bk.lvi")
                .map_err(|e| e.to_string())?;
            let v = self.vec_load(bk.lhs_vec, bk.lhs_elem, bk.lp, idx, bk.lhs_align, "bk.lv")?;
            self.convert_vec(v, bk.lhs_dtype, bk.out, bk.out_vec)?
        } else {
            l_splat.unwrap().1
        };
        let rvec = if rcontig {
            let idx = b
                .build_int_add(ob, k, "bk.rvi")
                .map_err(|e| e.to_string())?;
            let v = self.vec_load(bk.rhs_vec, bk.rhs_elem, bk.rp, idx, bk.rhs_align, "bk.rv")?;
            self.convert_vec(v, bk.rhs_dtype, bk.out, bk.out_vec)?
        } else {
            r_splat.unwrap().1
        };
        let (vl, vr) = if bk.scalar_lhs {
            (rvec, lvec)
        } else {
            (lvec, rvec)
        };
        let resv = self.vec_arith(bk.op, bk.out_float, vl, vr)?;
        let oidx = b
            .build_int_add(rowbase, k, "bk.voi")
            .map_err(|e| e.to_string())?;
        let optr = unsafe {
            b.build_gep(bk.out_elem, bk.op_ptr, &[oidx], "bk.vop")
                .map_err(|e| e.to_string())?
        };
        let st = b.build_store(optr, resv).map_err(|e| e.to_string())?;
        st.set_alignment(bk.out_align).map_err(|e| e.to_string())?;
        let knext = b
            .build_int_add(k, i64t.const_int(VW, false), "bk.knext")
            .map_err(|e| e.to_string())?;
        b.build_unconditional_branch(ivhead)
            .map_err(|e| e.to_string())?;
        k_phi.add_incoming(&[(&i64t.const_int(0, false), rbody), (&knext, ivbody)]);

        // --- inner scalar tail ---
        b.position_at_end(ithead);
        let j_phi = b.build_phi(i64t, "bk.j").map_err(|e| e.to_string())?;
        let j = j_phi.as_basic_value().into_int_value();
        let jcond = b
            .build_int_compare(IntPredicate::SLT, j, bk.inner, "bk.jcond")
            .map_err(|e| e.to_string())?;
        b.build_conditional_branch(jcond, itbody, rcont)
            .map_err(|e| e.to_string())?;
        b.position_at_end(itbody);
        let ls = if lcontig {
            let idx = b
                .build_int_add(oa, j, "bk.lji")
                .map_err(|e| e.to_string())?;
            let s = self.elem_load(bk.lhs_elem, bk.lp, idx, "bk.ls")?;
            self.convert_scalar(s, bk.lhs_dtype, bk.out, bk.out_elem)?
        } else {
            l_splat.unwrap().0
        };
        let rs = if rcontig {
            let idx = b
                .build_int_add(ob, j, "bk.rji")
                .map_err(|e| e.to_string())?;
            let s = self.elem_load(bk.rhs_elem, bk.rp, idx, "bk.rs")?;
            self.convert_scalar(s, bk.rhs_dtype, bk.out, bk.out_elem)?
        } else {
            r_splat.unwrap().0
        };
        let (sl, sr) = if bk.scalar_lhs { (rs, ls) } else { (ls, rs) };
        let ress = self.scalar_arith(bk.op, bk.out_float, sl, sr)?;
        let oidx = b
            .build_int_add(rowbase, j, "bk.joi")
            .map_err(|e| e.to_string())?;
        let optr = unsafe {
            b.build_gep(bk.out_elem, bk.op_ptr, &[oidx], "bk.jop")
                .map_err(|e| e.to_string())?
        };
        let st = b.build_store(optr, ress).map_err(|e| e.to_string())?;
        st.set_alignment(bk.out_align).map_err(|e| e.to_string())?;
        let jnext = b
            .build_int_add(j, i64t.const_int(1, false), "bk.jnext")
            .map_err(|e| e.to_string())?;
        b.build_unconditional_branch(ithead)
            .map_err(|e| e.to_string())?;
        j_phi.add_incoming(&[(&vend, ivhead), (&jnext, itbody)]);

        // --- next row ---
        b.position_at_end(rcont);
        let rnext = b
            .build_int_add(r, i64t.const_int(1, false), "bk.rnext")
            .map_err(|e| e.to_string())?;
        b.build_unconditional_branch(rhead)
            .map_err(|e| e.to_string())?;
        r_phi.add_incoming(&[(&i64t.const_int(0, false), entry), (&rnext, rcont)]);
        Ok(())
    }

    /// Element-wise unary math over an array (#505, D-0168): `out[i] = op(src[i])`
    /// with a `VW`-lane vector body + scalar remainder, mirroring [`array_binop`]
    /// with a single operand. Each lane is loaded at `src_dtype`, converted to the
    /// float result dtype `out`, run through the LLVM math intrinsic, and stored.
    fn array_unaryop(
        &self,
        op: zir::ArrayMathOp,
        out: zir::Dtype,
        src_dtype: zir::Dtype,
        src: BasicValueEnum<'ctx>,
        fv: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let b = self.builder;
        let i64t = self.ctx.i64_type();
        let sa = src.into_pointer_value();
        let code = i64t.const_int(out.code() as u64, false);
        let res = self
            .call1("zap_array_new_like", &[sa.into(), code.into()], "arr")?
            .into_pointer_value();
        let n = self
            .call1("zap_array_count", &[res.into()], "n")?
            .into_int_value();
        let sp = self
            .call1("zap_array_data", &[sa.into()], "sp")?
            .into_pointer_value();
        let op_ptr = self
            .call1("zap_array_data", &[res.into()], "op")?
            .into_pointer_value();

        let out_elem = self.basic(out.storage_ty());
        let out_align = out.elem_size();
        let src_elem = self.basic(src_dtype.storage_ty());
        let src_align = src_dtype.elem_size();
        const VW: u64 = 4;
        let out_vec = self.dtype_vec(out, VW as u32);
        let src_vec = self.dtype_vec(src_dtype, VW as u32);

        let mask = i64t.const_int(!(VW - 1), false);
        let vend = b.build_and(n, mask, "vend").map_err(|e| e.to_string())?;

        let entry = b.get_insert_block().unwrap();
        let vhead = self.ctx.append_basic_block(fv, "um.vhead");
        let vbody = self.ctx.append_basic_block(fv, "um.vbody");
        let rhead = self.ctx.append_basic_block(fv, "um.rhead");
        let rbody = self.ctx.append_basic_block(fv, "um.rbody");
        let done = self.ctx.append_basic_block(fv, "um.done");
        b.build_unconditional_branch(vhead)
            .map_err(|e| e.to_string())?;

        // --- vector loop ---
        b.position_at_end(vhead);
        let i_phi = b.build_phi(i64t, "i").map_err(|e| e.to_string())?;
        let i = i_phi.as_basic_value().into_int_value();
        let vcond = b
            .build_int_compare(IntPredicate::SLT, i, vend, "vcond")
            .map_err(|e| e.to_string())?;
        b.build_conditional_branch(vcond, vbody, rhead)
            .map_err(|e| e.to_string())?;
        b.position_at_end(vbody);
        let sv = self.vec_load(src_vec, src_elem, sp, i, src_align, "sv")?;
        let sv = self.convert_vec(sv, src_dtype, out, out_vec)?;
        let rv = self.math_vec(op, out_vec, sv)?;
        let optr = unsafe {
            b.build_gep(out_elem, op_ptr, &[i], "optr")
                .map_err(|e| e.to_string())?
        };
        let st = b.build_store(optr, rv).map_err(|e| e.to_string())?;
        st.set_alignment(out_align).map_err(|e| e.to_string())?;
        let inext = b
            .build_int_add(i, i64t.const_int(VW, false), "inext")
            .map_err(|e| e.to_string())?;
        b.build_unconditional_branch(vhead)
            .map_err(|e| e.to_string())?;
        let zero = i64t.const_int(0, false);
        i_phi.add_incoming(&[(&zero, entry), (&inext, vbody)]);

        // --- scalar remainder loop ---
        b.position_at_end(rhead);
        let j_phi = b.build_phi(i64t, "j").map_err(|e| e.to_string())?;
        let j = j_phi.as_basic_value().into_int_value();
        let rcond = b
            .build_int_compare(IntPredicate::SLT, j, n, "rcond")
            .map_err(|e| e.to_string())?;
        b.build_conditional_branch(rcond, rbody, done)
            .map_err(|e| e.to_string())?;
        b.position_at_end(rbody);
        let ssc = self.elem_load(src_elem, sp, j, "ss")?;
        let ssc = self.convert_scalar(ssc, src_dtype, out, out_elem)?;
        let rsc = self.math_scalar(op, out_elem, ssc)?;
        let optr = unsafe {
            b.build_gep(out_elem, op_ptr, &[j], "optrs")
                .map_err(|e| e.to_string())?
        };
        let st = b.build_store(optr, rsc).map_err(|e| e.to_string())?;
        st.set_alignment(out_align).map_err(|e| e.to_string())?;
        let jnext = b
            .build_int_add(j, i64t.const_int(1, false), "jnext")
            .map_err(|e| e.to_string())?;
        b.build_unconditional_branch(rhead)
            .map_err(|e| e.to_string())?;
        j_phi.add_incoming(&[(&vend, vhead), (&jnext, rbody)]);

        b.position_at_end(done);
        Ok(res.into())
    }

    /// The LLVM intrinsic name for a float unary-math op (`tan` is special-cased
    /// by the callers as `sin/cos`).
    fn math_intrinsic_name(op: zir::ArrayMathOp) -> &'static str {
        use zir::ArrayMathOp::*;
        match op {
            Sqrt => "llvm.sqrt",
            Exp => "llvm.exp",
            Log => "llvm.log",
            Sin => "llvm.sin",
            Cos => "llvm.cos",
            Fabs => "llvm.fabs",
            Tan => "llvm.sin", // unused directly; Tan builds sin/cos below
        }
    }

    /// Apply a unary-math op to a lane vector via the LLVM intrinsic. `tan` has
    /// no intrinsic, so it is `sin(x) / cos(x)`.
    fn math_vec(
        &self,
        op: zir::ArrayMathOp,
        vec_ty: VectorType<'ctx>,
        v: VectorValue<'ctx>,
    ) -> Result<VectorValue<'ctx>, String> {
        if matches!(op, zir::ArrayMathOp::Tan) {
            let s = self.call_unary_intrinsic("llvm.sin", vec_ty.into(), v.into())?;
            let c = self.call_unary_intrinsic("llvm.cos", vec_ty.into(), v.into())?;
            return self
                .builder
                .build_float_div(s.into_vector_value(), c.into_vector_value(), "vtan")
                .map_err(|e| e.to_string());
        }
        let name = Self::math_intrinsic_name(op);
        Ok(self
            .call_unary_intrinsic(name, vec_ty.into(), v.into())?
            .into_vector_value())
    }

    /// Scalar form of [`Self::math_vec`].
    fn math_scalar(
        &self,
        op: zir::ArrayMathOp,
        ty: BasicTypeEnum<'ctx>,
        v: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if matches!(op, zir::ArrayMathOp::Tan) {
            let s = self.call_unary_intrinsic("llvm.sin", ty, v)?;
            let c = self.call_unary_intrinsic("llvm.cos", ty, v)?;
            return self
                .builder
                .build_float_div(s.into_float_value(), c.into_float_value(), "tan")
                .map(|x| x.into())
                .map_err(|e| e.to_string());
        }
        self.call_unary_intrinsic(Self::math_intrinsic_name(op), ty, v)
    }

    /// Call an overloaded unary LLVM intrinsic (`llvm.sqrt`, …) whose single
    /// overload type is `ty` (a scalar float or a float vector).
    fn call_unary_intrinsic(
        &self,
        name: &str,
        ty: BasicTypeEnum<'ctx>,
        v: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let intr = inkwell::intrinsics::Intrinsic::find(name)
            .ok_or_else(|| format!("missing LLVM intrinsic {name}"))?;
        let decl = intr
            .get_declaration(self.module, &[ty])
            .ok_or_else(|| format!("no declaration for intrinsic {name}"))?;
        let cs = self
            .builder
            .build_call(decl, &[v.into()], "m")
            .map_err(|e| e.to_string())?;
        cs.try_as_basic_value()
            .left()
            .ok_or_else(|| format!("intrinsic {name} returned void"))
    }

    /// The `VW`-lane LLVM vector type for a dtype's storage element.
    fn dtype_vec(&self, d: zir::Dtype, vw: u32) -> VectorType<'ctx> {
        match d.storage_ty() {
            Ty::F64 => self.ctx.f64_type().vec_type(vw),
            Ty::F32 => self.ctx.f32_type().vec_type(vw),
            Ty::I64 => self.ctx.i64_type().vec_type(vw),
            Ty::I32 => self.ctx.i32_type().vec_type(vw),
            Ty::I16 => self.ctx.i16_type().vec_type(vw),
            Ty::I8 => self.ctx.i8_type().vec_type(vw),
            // `storage_ty` yields only these float/int widths for array dtypes.
            _ => unreachable!("array dtype storage is f32/f64 or i8/i16/i32/i64"),
        }
    }

    /// Convert a scalar element value from `src` dtype storage to `dst` dtype
    /// storage (ticket #49 promotion). `to` is the target LLVM scalar type.
    /// Covers widening (array operands, always `src` rank <= `dst`) and the one
    /// narrowing case — a weak `i64` scalar cast down to a `uint8` result.
    fn convert_scalar(
        &self,
        v: BasicValueEnum<'ctx>,
        src: zir::Dtype,
        dst: zir::Dtype,
        to: BasicTypeEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if src == dst {
            return Ok(v);
        }
        let b = self.builder;
        let r: BasicValueEnum = match (src.is_float(), dst.is_float()) {
            // int -> float: unsigned source (uint8/bool) uses uitofp, else sitofp.
            (false, true) => {
                let fv = to.into_float_type();
                if src.is_unsigned() {
                    b.build_unsigned_int_to_float(v.into_int_value(), fv, "u2f")
                } else {
                    b.build_signed_int_to_float(v.into_int_value(), fv, "s2f")
                }
                .map_err(|e| e.to_string())?
                .into()
            }
            // int -> int: same storage width (bool<->uint8) is identity; widen
            // with zext (unsigned src) / sext (signed); narrow (weak i64 scalar
            // -> uint8) truncates.
            (false, false) => {
                let to_int = to.into_int_type();
                let iv = v.into_int_value();
                let (fw, tw) = (iv.get_type().get_bit_width(), to_int.get_bit_width());
                if fw == tw {
                    iv.into()
                } else if tw > fw {
                    if src.is_unsigned() {
                        b.build_int_z_extend(iv, to_int, "zext")
                    } else {
                        b.build_int_s_extend(iv, to_int, "sext")
                    }
                    .map_err(|e| e.to_string())?
                    .into()
                } else {
                    b.build_int_truncate(iv, to_int, "trunc")
                        .map_err(|e| e.to_string())?
                        .into()
                }
            }
            // float -> float: widen (f32 -> f64, fpext) or narrow (f64 -> f32,
            // fptrunc) — the narrowing is the weak-float-scalar-into-f32-result
            // and `f32 + f64 -> f64` paths. `src == dst` already returned, so the
            // widths differ; `elem_size` orders them (f32 = 4, f64 = 8). float ->
            // int never arises (a float operand forces a float result).
            (true, true) => {
                let fv = v.into_float_value();
                let to_f = to.into_float_type();
                if dst.elem_size() > src.elem_size() {
                    b.build_float_ext(fv, to_f, "fpext")
                } else {
                    b.build_float_trunc(fv, to_f, "fptrunc")
                }
                .map_err(|e| e.to_string())?
                .into()
            }
            (true, false) => v,
        };
        Ok(r)
    }

    /// The vector form of [`Self::convert_scalar`]: convert a `VW`-lane vector
    /// from `src` dtype storage to `dst` dtype storage. `to` is the target LLVM
    /// vector type. Array operands only ever widen, so no truncation here.
    fn convert_vec(
        &self,
        v: VectorValue<'ctx>,
        src: zir::Dtype,
        dst: zir::Dtype,
        to: VectorType<'ctx>,
    ) -> Result<VectorValue<'ctx>, String> {
        if src == dst {
            return Ok(v);
        }
        let b = self.builder;
        let r = match (src.is_float(), dst.is_float()) {
            (false, true) => if src.is_unsigned() {
                b.build_unsigned_int_to_float(v, to, "vu2f")
            } else {
                b.build_signed_int_to_float(v, to, "vs2f")
            }
            .map_err(|e| e.to_string())?,
            (false, false) => {
                let fw = v
                    .get_type()
                    .get_element_type()
                    .into_int_type()
                    .get_bit_width();
                let tw = to.get_element_type().into_int_type().get_bit_width();
                if fw == tw {
                    v
                } else if src.is_unsigned() {
                    b.build_int_z_extend(v, to, "vzext")
                        .map_err(|e| e.to_string())?
                } else {
                    b.build_int_s_extend(v, to, "vsext")
                        .map_err(|e| e.to_string())?
                }
            }
            // float -> float vector: fpext (f32 -> f64) / fptrunc (f64 -> f32).
            (true, true) => if dst.elem_size() > src.elem_size() {
                b.build_float_ext(v, to, "vfpext")
            } else {
                b.build_float_trunc(v, to, "vfptrunc")
            }
            .map_err(|e| e.to_string())?,
            // float -> int never arises in element-wise promotion.
            (true, false) => v,
        };
        Ok(r)
    }

    /// Call a runtime function expected to return a single value.
    fn call1(
        &self,
        name: &str,
        args: &[inkwell::values::BasicMetadataValueEnum<'ctx>],
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let fv = self.fns[name];
        let cs = self
            .builder
            .build_call(fv, args, label)
            .map_err(|e| e.to_string())?;
        cs.try_as_basic_value()
            .left()
            .ok_or_else(|| format!("runtime call '{name}' returned void"))
    }

    /// A `VW`-lane vector load from `base[idx]`, aligned to one element
    /// (`align` bytes: 8 for `i64`/`f64`, 1 for `uint8`/`bool`) so the buffer
    /// need not be vector-aligned.
    fn vec_load(
        &self,
        vec_ty: VectorType<'ctx>,
        elem_ty: BasicTypeEnum<'ctx>,
        base: PointerValue<'ctx>,
        idx: inkwell::values::IntValue<'ctx>,
        align: u32,
        label: &str,
    ) -> Result<VectorValue<'ctx>, String> {
        let ptr = unsafe {
            self.builder
                .build_gep(elem_ty, base, &[idx], label)
                .map_err(|e| e.to_string())?
        };
        let v = self
            .builder
            .build_load(vec_ty, ptr, label)
            .map_err(|e| e.to_string())?;
        v.into_vector_value()
            .as_instruction()
            .unwrap()
            .set_alignment(align)
            .map_err(|e| e.to_string())?;
        Ok(v.into_vector_value())
    }

    /// A scalar element load from `base[idx]`. The pointee's natural alignment
    /// (i8 -> 1, i64/f64 -> 8) is already correct for the element-indexed GEP,
    /// so this keeps the default.
    fn elem_load(
        &self,
        elem_ty: BasicTypeEnum<'ctx>,
        base: PointerValue<'ctx>,
        idx: inkwell::values::IntValue<'ctx>,
        label: &str,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let ptr = unsafe {
            self.builder
                .build_gep(elem_ty, base, &[idx], label)
                .map_err(|e| e.to_string())?
        };
        self.builder
            .build_load(elem_ty, ptr, label)
            .map_err(|e| e.to_string())
    }

    fn vec_arith(
        &self,
        op: zir::BinOp,
        is_float: bool,
        l: VectorValue<'ctx>,
        r: VectorValue<'ctx>,
    ) -> Result<VectorValue<'ctx>, String> {
        use zir::BinOp::*;
        let b = self.builder;
        let v = if is_float {
            match op {
                Add => b.build_float_add(l, r, "vfadd"),
                Sub => b.build_float_sub(l, r, "vfsub"),
                Mul => b.build_float_mul(l, r, "vfmul"),
                // True division (#506): `out` is always a float dtype, so a
                // zero divisor yields IEEE ±inf/nan (no raise, D-0169).
                Div => b.build_float_div(l, r, "vfdiv"),
                _ => return Err("unsupported element-wise array op".into()),
            }
        } else {
            match op {
                Add => b.build_int_add(l, r, "viadd"),
                Sub => b.build_int_sub(l, r, "visub"),
                Mul => b.build_int_mul(l, r, "vimul"),
                _ => return Err("unsupported element-wise array op".into()),
            }
        };
        let v = v.map_err(|e| e.to_string())?;
        // FP determinism policy (#946, D-0196): stamp fast-math flags on the
        // vector float result; integer vector ops are exactly associative
        // (two's-complement wrap) and take no flag.
        Ok(if is_float { self.stamp_fp_vec(v) } else { v })
    }

    /// Stamp the FP-policy fast-math flags (#946, D-0196) onto a freshly built
    /// scalar float instruction: `contract` under `default`/`fast` (lets `a*b+c`
    /// fuse into an FMA), plus `reassoc` under `fast`. A no-op under `strict`
    /// (flags == 0) and for a non-instruction value (a constant-folded result),
    /// so bit-exact builds emit exactly the written rounding.
    fn stamp_fp(&self, v: inkwell::values::FloatValue<'ctx>) -> inkwell::values::FloatValue<'ctx> {
        let flags = self.fp.fast_math_flags();
        if flags != 0 {
            if let Some(inst) = v.as_instruction() {
                inst.set_fast_math_flags(flags);
            }
        }
        v
    }

    /// Vector-float form of [`Cg::stamp_fp`] (element-wise array float ops, #946).
    fn stamp_fp_vec(&self, v: VectorValue<'ctx>) -> VectorValue<'ctx> {
        let flags = self.fp.fast_math_flags();
        if flags != 0 {
            if let Some(inst) = v.as_instruction() {
                inst.set_fast_math_flags(flags);
            }
        }
        v
    }

    fn scalar_arith(
        &self,
        op: zir::BinOp,
        is_float: bool,
        l: BasicValueEnum<'ctx>,
        r: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use zir::BinOp::*;
        let b = self.builder;
        let v: BasicValueEnum = if is_float {
            let (lf, rf) = (l.into_float_value(), r.into_float_value());
            let fv = match op {
                Add => b.build_float_add(lf, rf, "fadd"),
                Sub => b.build_float_sub(lf, rf, "fsub"),
                Mul => b.build_float_mul(lf, rf, "fmul"),
                // True division (#506): `out` is always float -> ±inf/nan on a
                // zero divisor, no raise (D-0169).
                Div => b.build_float_div(lf, rf, "fdiv"),
                _ => return Err("unsupported element-wise array op".into()),
            }
            .map_err(|e| e.to_string())?;
            // FP determinism policy (#946, D-0196).
            self.stamp_fp(fv).into()
        } else {
            let (li, ri) = (l.into_int_value(), r.into_int_value());
            match op {
                Add => b.build_int_add(li, ri, "add"),
                Sub => b.build_int_sub(li, ri, "sub"),
                Mul => b.build_int_mul(li, ri, "mul"),
                _ => return Err("unsupported element-wise array op".into()),
            }
            .map_err(|e| e.to_string())?
            .into()
        };
        Ok(v)
    }

    fn term(
        &self,
        f: &Func,
        term: &Term,
        blocks: &[BasicBlock<'ctx>],
        vals: &[Option<BasicValueEnum<'ctx>>],
        fv: FunctionValue<'ctx>,
    ) -> Result<(), String> {
        let b = self.builder;
        // The implicit root scope joins every spawned coroutine at program exit
        // (ticket #25, D-0033): a bare `spawn` is never truly detached. Emitted
        // just before each `main` *return* (not before a loop back-edge or the
        // uncaught-exception `unreachable`) so fire-and-forget spawns the
        // program does not otherwise await still complete before exit. A no-op
        // when nothing was spawned.
        if f.is_main && matches!(term, Term::Ret(_)) {
            if let Some(join) = self.fns.get("zap_rt_join_all") {
                b.build_call(*join, &[], "").map_err(|e| e.to_string())?;
            }
        }
        // Precise GC (ticket #48): unlink this function's shadow frame before it
        // returns, so the per-thread head no longer points at the (about-to-be
        // destroyed) frame.
        #[cfg(feature = "gc-immix")]
        if matches!(term, Term::Ret(_)) {
            self.gc_unlink()?;
        }
        match term {
            Term::Ret(Some(v)) => {
                // Reload a returned GC pointer from its slot (#120): an interior
                // safepoint earlier in the block may have relocated it, leaving the
                // register stale. `gc_unlink` above only repoints the shadow head;
                // the frame alloca (and this slot) stays valid until the function
                // returns, and no safepoint runs between here and the `ret`.
                let rv = self.gc_reload_operand(*v, vals[*v as usize].expect("ret value"));
                // `main` is emitted as `i32 main()` (the C entry point), so a Zap
                // `main` declared to return `int64` must narrow its value to i32.
                if f.is_main {
                    let iv = rv.into_int_value();
                    let i32t = self.ctx.i32_type();
                    let r = if iv.get_type().get_bit_width() == 32 {
                        iv
                    } else {
                        b.build_int_truncate(iv, i32t, "mainret")
                            .map_err(|e| e.to_string())?
                    };
                    b.build_return(Some(&r)).map_err(|e| e.to_string())?;
                } else if let Some(ret_ty) = fv
                    .get_type()
                    .get_return_type()
                    .filter(|t| t.is_struct_type())
                {
                    // An inline value struct / tuple is returned by value
                    // (register/sret, D-0051): load the aggregate from the
                    // pointer the body holds and return it.
                    let agg = b
                        .build_load(ret_ty, rv.into_pointer_value(), "ret.agg")
                        .map_err(|e| e.to_string())?;
                    b.build_return(Some(&agg)).map_err(|e| e.to_string())?;
                } else {
                    b.build_return(Some(&rv)).map_err(|e| e.to_string())?;
                }
            }
            Term::Ret(None) => {
                if f.is_main {
                    let zero = self.ctx.i32_type().const_int(0, false);
                    b.build_return(Some(&zero)).map_err(|e| e.to_string())?;
                } else if f.ret == Ty::Void {
                    b.build_return(None).map_err(|e| e.to_string())?;
                } else {
                    b.build_unreachable().map_err(|e| e.to_string())?;
                }
            }
            Term::Br(bb) => {
                b.build_unconditional_branch(blocks[*bb as usize])
                    .map_err(|e| e.to_string())?;
            }
            Term::BrIf {
                cond,
                then_bb,
                else_bb,
            } => {
                let c = vals[*cond as usize].expect("cond").into_int_value();
                b.build_conditional_branch(c, blocks[*then_bb as usize], blocks[*else_bb as usize])
                    .map_err(|e| e.to_string())?;
            }
            Term::Unreachable => {
                b.build_unreachable().map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    fn printf_lit(&self, text: &str) -> Result<(), String> {
        let printf = self.module.get_function("printf").unwrap();
        let fmt = self
            .builder
            .build_global_string_ptr(text, "s")
            .map_err(|e| e.to_string())?
            .as_pointer_value();
        self.builder
            .build_call(printf, &[fmt.into()], "")
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn printf_val(&self, fmt_text: &str, val: BasicValueEnum<'ctx>) -> Result<(), String> {
        let printf = self.module.get_function("printf").unwrap();
        let fmt = self
            .builder
            .build_global_string_ptr(fmt_text, "fmt")
            .map_err(|e| e.to_string())?
            .as_pointer_value();
        self.builder
            .build_call(printf, &[fmt.into(), val.into()], "")
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn int_pred(p: CmpPred, unsigned: bool) -> IntPredicate {
    match p {
        CmpPred::Eq => IntPredicate::EQ,
        CmpPred::Ne => IntPredicate::NE,
        // Unsigned operands use the unsigned ordering predicates so a `uint64`
        // above `INT64_MAX` compares as the larger value, not as negative (#552).
        CmpPred::Lt if unsigned => IntPredicate::ULT,
        CmpPred::Le if unsigned => IntPredicate::ULE,
        CmpPred::Gt if unsigned => IntPredicate::UGT,
        CmpPred::Ge if unsigned => IntPredicate::UGE,
        CmpPred::Lt => IntPredicate::SLT,
        CmpPred::Le => IntPredicate::SLE,
        CmpPred::Gt => IntPredicate::SGT,
        CmpPred::Ge => IntPredicate::SGE,
    }
}

fn float_pred(p: CmpPred) -> FloatPredicate {
    match p {
        CmpPred::Eq => FloatPredicate::OEQ,
        CmpPred::Ne => FloatPredicate::ONE,
        CmpPred::Lt => FloatPredicate::OLT,
        CmpPred::Le => FloatPredicate::OLE,
        CmpPred::Gt => FloatPredicate::OGT,
        CmpPred::Ge => FloatPredicate::OGE,
    }
}

#[cfg(test)]
mod tests_codegen_ir;
#[cfg(test)]
mod tests_runtime_contract;

#[cfg(test)]
mod cache_tmp_name_tests {
    use super::cache_tmp_seq;
    use std::collections::HashSet;
    use std::sync::{Arc, Barrier, Mutex};

    /// Issue #917 guard for the runtime-cache pre-publish temp names. Many
    /// threads draw sequence values behind a barrier so they start together —
    /// the exact condition under which the old `SystemTime`-nanosecond name (on
    /// a microsecond-quantised clock) handed two threads compiling the same
    /// cache `key` the same temp path and raced the atomic rename. The
    /// process-global counter yields all-distinct values, so this passes
    /// deterministically; a regression to clock-based naming fails it here.
    #[test]
    fn cache_tmp_seq_values_are_distinct_under_parallel_allocation() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 256;

        let barrier = Arc::new(Barrier::new(THREADS));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    (0..PER_THREAD).map(|_| cache_tmp_seq()).collect::<Vec<_>>()
                })
            })
            .collect();

        let seen: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
        for h in handles {
            let seqs = h.join().expect("allocation thread panicked");
            let mut guard = seen.lock().unwrap();
            for s in seqs {
                assert!(guard.insert(s), "cache temp sequence value collided: {s}");
            }
        }
        assert_eq!(seen.lock().unwrap().len(), THREADS * PER_THREAD);
    }
}
