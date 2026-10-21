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
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetData, TargetMachine,
};
use inkwell::types::{
    BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FloatType, IntType, VectorType,
};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, PointerValue, VectorValue,
};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate, OptimizationLevel};

use zapc_ir::{self as zir, lower_module, CastKind, CmpPred, Func, Inst, Owner, Term, Ty};
use zapc_syntax::ast::Module as AstModule;

/// Built-in stdlib module registry (ticket #121): per-module runtime-symbol
/// declarations live in `modules/<name>.rs`, grouped by module instead of a
/// single hand-maintained `declare_rt` cluster, so new modules are conflict-free
/// parallel-addable. See [`modules`] for the seam contract.
mod modules;

/// A code-generation backend.
pub trait Backend {
    /// Compile a (type-checked) AST module to a native object file.
    fn compile_object(&self, module: &AstModule, obj_path: &Path) -> Result<(), String>;
}

/// The LLVM backend.
pub struct LlvmBackend;

impl Backend for LlvmBackend {
    fn compile_object(&self, module: &AstModule, obj_path: &Path) -> Result<(), String> {
        let (zir_module, errs) = lower_module(module);
        if !errs.is_empty() {
            return Err(errs
                .iter()
                .map(|e| e.message.clone())
                .collect::<Vec<_>>()
                .join("; "));
        }

        let ctx = Context::create();
        let llmod = ctx.create_module("zap");
        let builder = ctx.create_builder();

        // Create the target machine before translation so its data layout drives
        // both the emitted object and the GC type-descriptor field offsets
        // (ticket #48 — see Cg::translate).
        Target::initialize_native(&InitializationConfig::default())
            .map_err(|e| format!("failed to initialize native target: {e}"))?;
        let triple = TargetMachine::get_default_triple();
        let target = Target::from_triple(&triple)
            .map_err(|e| format!("unknown target: {}", e.to_string()))?;
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

        let mut cg = Cg::new(&ctx, &llmod, &builder);
        cg.translate(&zir_module)?;

        llmod
            .verify()
            .map_err(|e| format!("internal error: invalid LLVM module:\n{}", e.to_string()))?;

        tm.write_to_file(&llmod, FileType::Object, obj_path)
            .map_err(|e| format!("failed to write object file: {}", e.to_string()))?;
        Ok(())
    }
}

/// Compile an AST module to a native executable (object + `cc` link).
/// The Zap C runtime, compiled and linked with each program.
const RUNTIME_C: &str = include_str!("zaprt.c");
/// The precise Immix collector, `#include`d by `zaprt.c` under `ZAP_GC_IMMIX`
/// (ticket #48). Written next to the runtime so the relative include resolves.
const RUNTIME_GC_C: &str = include_str!("zapgc.inc.c");

pub fn build_executable(module: &AstModule, out_path: &Path) -> Result<(), String> {
    let obj_path = out_path.with_extension("o");
    LlvmBackend.compile_object(module, &obj_path)?;

    // Write the runtime into a per-build temp dir and let cc compile + link it
    // in. The GC collector include is written alongside so `#include
    // "zapgc.inc.c"` (relative) resolves (ticket #48).
    let build_dir = std::env::temp_dir().join(format!("zap-build-{}", std::process::id()));
    std::fs::create_dir_all(&build_dir).map_err(|e| format!("failed to create build dir: {e}"))?;
    let rt_path = build_dir.join("zaprt.c");
    std::fs::write(&rt_path, RUNTIME_C).map_err(|e| format!("failed to write runtime: {e}"))?;
    std::fs::write(build_dir.join("zapgc.inc.c"), RUNTIME_GC_C)
        .map_err(|e| format!("failed to write gc runtime: {e}"))?;

    // `-lgc` links the conservative Boehm collector the runtime allocates
    // through (ticket #20, D-0009); the dev container provides `libgc-dev`.
    // `-lstdc++` pulls in the Itanium unwinding personality and the
    // `__cxa_*` exception entry points the runtime calls (ticket #22); the
    // actual stack unwinder (`_Unwind_*`) comes from libgcc, linked by default.
    let mut cmd = Command::new("cc");
    cmd.arg(&obj_path).arg(&rt_path).arg("-o").arg(out_path);
    // Select the collector (ticket #48): the precise Immix GC is compiled into
    // the runtime with -DZAP_GC_IMMIX and needs no Boehm library; the default
    // build links the conservative Boehm collector (-lgc).
    #[cfg(feature = "gc-immix")]
    cmd.arg("-DZAP_GC_IMMIX");
    #[cfg(not(feature = "gc-immix"))]
    cmd.arg("-lgc");
    cmd.arg("-lstdc++")
        // `-lm` provides libm `pow`/`floor`, used by the scalar `**` and `//`
        // runtime helpers (ticket #19).
        .arg("-lm")
        // `-lpcre2-8` provides the 8-bit PCRE2 API the regex runtime wraps
        // (ticket #23); the dev container / CI provide `libpcre2-dev`.
        .arg("-lpcre2-8")
        // `-lz` provides the system zlib backing the `zlib`/`gzip` compression
        // modules (ticket #106); the dev container / CI provide `zlib1g-dev`
        // and it ships on macOS.
        .arg("-lz")
        // `-lssl -lcrypto` provide the system OpenSSL/LibreSSL backing the `tls`
        // module and the `http` `https://` client (ticket #147, D-0062); the dev
        // container / CI provide `libssl-dev` (OpenSSL 3.x on Ubuntu 24.04) and
        // LibreSSL ships on macOS. `-lssl` depends on `-lcrypto`, so that order.
        .arg("-lssl")
        .arg("-lcrypto")
        // `-lpthread` backs the goroutine runtime (ticket #25): each `spawn`
        // starts a GC-registered pthread, and channels block on pthread
        // mutex/condvars. `libgc` must be thread-aware (the runtime defines
        // `GC_THREADS` and uses `GC_pthread_create`); the dev container's
        // `libgc-dev` is built with thread support.
        .arg("-lpthread");
    // CPython embedding flags (ticket #27, D-0012): the `-I...` cflags let
    // `zaprt.c` find `<Python.h>`, and the libs link libpython. Discovered with
    // `pkg-config` (the dev container / CI provide `python3-dev` + `pkg-config`)
    // so the exact libpython version and path are not hardcoded.
    cmd.args(python_embed_flags()?);
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
    let _ = std::fs::remove_file(&obj_path);
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
fn python_embed_flags() -> Result<Vec<String>, String> {
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
        | FieldGet { dest, .. }
        | LandingPad { dest, .. }
        | ArrayBinOp { dest, .. }
        | FuncAddr { dest, .. }
        | IfaceMake { dest, .. } => Some(*dest),
        Call { dest, .. }
        | Invoke { dest, .. }
        | IfaceCall { dest, .. }
        | IfaceInvoke { dest, .. } => *dest,
        _ => None,
    }
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
    /// Per-struct GC type-descriptor global (ticket #48), indexed by struct id.
    /// Passed to `zap_alloc_typed` so the precise collector can trace a struct
    /// instance's pointer fields. Empty under the Boehm build.
    #[cfg_attr(not(feature = "gc-immix"), allow(dead_code))]
    struct_descs: Vec<PointerValue<'ctx>>,
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
    /// Cache of interface dispatch tables: `(concrete_sid, iid)` -> the static
    /// `[N x ptr]` itable global, built once per implementing pair.
    itables: std::cell::RefCell<std::collections::HashMap<(u32, u32), PointerValue<'ctx>>>,
}

/// A function's shadow-stack frame (ticket #48). Codegen reserves one root slot
/// per GC-pointer-typed SSA value, links the frame onto the per-thread
/// `zap_shadow_head` on entry, stores each pointer value into its slot at the
/// end of its defining block, and unlinks on return. A GC safepoint (top of
/// every block) then finds every live pointer via the chain. Over-retention of
/// dead-but-slotted values is safe for the non-moving collector.
#[derive(Clone)]
#[cfg_attr(not(feature = "gc-immix"), allow(dead_code))]
struct GcFrame<'ctx> {
    /// Alloca of `{ ptr prev; i64 nroots; [N x ptr] roots }` in the entry block.
    frame_ptr: PointerValue<'ctx>,
    frame_ty: inkwell::types::StructType<'ctx>,
    /// value id -> root slot index, or `-1` if the value is not a GC pointer.
    slot_of: Vec<i32>,
    /// Number of root slots (the `[N x ptr]` array length).
    n: u32,
}

impl<'a, 'ctx> Cg<'a, 'ctx> {
    fn new(
        ctx: &'ctx Context,
        module: &'a LlvmModule<'ctx>,
        builder: &'a Builder<'ctx>,
    ) -> Cg<'a, 'ctx> {
        Cg {
            ctx,
            module,
            builder,
            fns: std::collections::HashMap::new(),
            structs: Vec::new(),
            struct_inline: Vec::new(),
            struct_descs: Vec::new(),
            gc_frame: None,
            target_data: None,
            entry_block: None,
            struct_names: Vec::new(),
            interfaces: Vec::new(),
            itables: std::cell::RefCell::new(std::collections::HashMap::new()),
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

        // Exception / unwinding entry points (ticket #22, see
        // design/RUNTIME-ABI.md §"Exception / unwinding ABI"). `zap_panic` begins
        // unwinding; the landing-pad glue extracts the in-flight exception object
        // (`zap_eh_begin_catch`) and balances the catch (`zap_eh_end_catch`);
        // `zap_uncaught` reports an exception that reached the top-level handler.
        self.declare_rt("zap_panic", &[ptr.into()], None);
        self.declare_rt("zap_eh_begin_catch", &[ptr.into()], Some(ptr.into()));
        self.declare_rt("zap_eh_end_catch", &[], None);
        self.declare_rt("zap_uncaught", &[ptr.into()], None);
        // The Itanium personality routine (libstdc++/libgcc). Landing pads name it;
        // it parses the unwind tables and drives phase-1/phase-2 unwinding. Its
        // exact signature is immaterial — it is only referenced by name — so it is
        // declared variadic `i32 (...)`.
        let personality =
            self.module
                .add_function("__gxx_personality_v0", i32t.fn_type(&[], true), None);
        self.fns
            .insert("__gxx_personality_v0".to_string(), personality);

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
        self.declare_rt("zap_list_get_i64", &[p.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_list_get_ptr", &[p.into(), i.into()], Some(p.into()));
        self.declare_rt("zap_list_set_i64", &[p.into(), i.into(), i.into()], None);
        self.declare_rt("zap_list_set_ptr", &[p.into(), i.into(), p.into()], None);
        self.declare_rt("zap_list_len", &[p.into()], Some(i.into()));
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
        // set
        self.declare_rt("zap_set_new", &[], Some(p.into()));
        self.declare_rt("zap_set_len", &[p.into()], Some(i.into()));
        self.declare_rt("zap_set_add_i64", &[p.into(), i.into()], None);
        self.declare_rt("zap_set_add_str", &[p.into(), p.into()], None);
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
        self.declare_rt("zap_heap_items", &[p.into()], Some(p.into()));
        // Struct-key hashing helpers (operator overloading, D-0047).
        self.declare_rt("zap_str_cmp", &[p.into(), p.into()], Some(i.into()));
        self.declare_rt("zap_str_hash", &[p.into()], Some(i.into()));
        self.declare_rt("zap_i64_hash", &[i.into()], Some(i.into()));
        self.declare_rt("zap_hash_combine", &[i.into(), i.into()], Some(i.into()));

        // Numeric-array entry points (ticket #24, see design/0002-arrays.md and
        // design/RUNTIME-ABI.md §"Collection layout"). Construction, element
        // access, length, and reductions are runtime calls; the element-wise
        // ops (`Inst::ArrayBinOp`) are vectorized inline in codegen instead.
        let f = self.ctx.f64_type();
        self.declare_rt("zap_array_new", &[i.into(), i.into()], Some(p.into()));
        self.declare_rt("zap_array_len", &[p.into()], Some(i.into()));
        self.declare_rt("zap_array_data", &[p.into()], Some(p.into()));
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

        // Scalar numeric helpers (ticket #19): integer/float power (`**`) and
        // Python floor division (`//`). The lowering emits plain `Inst::Call`s
        // to these, so codegen only declares their fixed C signatures.
        self.declare_rt("zap_ipow", &[i.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_powf", &[f.into(), f.into()], Some(f.into()));
        self.declare_rt("zap_ifloordiv", &[i.into(), i.into()], Some(i.into()));
        self.declare_rt("zap_ffloordiv", &[f.into(), f.into()], Some(f.into()));

        // Decorators & reflection (ticket #16, D-0036). `zap_i64_to_str` and
        // `zap_str_eq` back the `@derive(Repr)` / `@derive(Eq)` generated methods;
        // the `zap_reflect_*` entry points back the runtime-reflection surface
        // (metadata registered at startup, queried by `reflect_has`/`reflect_names`).
        self.declare_rt("zap_i64_to_str", &[i.into()], Some(p.into()));
        self.declare_rt("zap_str_eq", &[p.into(), p.into()], Some(i.into()));
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
        // design/0005-concurrency.md). Goroutines are backed by GC-registered OS
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
        self.declare_rt("zap_scope_exit", &[p.into()], None);
        self.declare_rt("zap_scope_exit_cancel", &[p.into()], None);
        self.declare_rt("zap_spawn_scoped", &[p.into(), p.into(), p.into()], None);
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
        self.declare_rt("zap_chan_close", &[p.into()], None);

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
        }

        // Build LLVM struct types (fields are scalars or pointers, so no
        // recursive-by-value ordering concerns).
        for sl in &m.structs {
            let field_tys: Vec<BasicTypeEnum> = sl.fields.iter().map(|t| self.basic(*t)).collect();
            self.structs.push(self.ctx.struct_type(&field_tys, false));
            self.struct_inline.push(sl.inline);
            self.struct_names.push(sl.name.clone());
        }
        // Interface metadata (D-0016) for fat-pointer construction and dynamic
        // dispatch (`Inst::IfaceMake` / `IfaceCall` / `IfaceInvoke`).
        self.interfaces = m.interfaces.clone();

        // Emit a GC type descriptor per struct (ticket #48): kind PRECISE with
        // the byte offsets of the pointer fields, computed from the struct's
        // LLVM layout. Only under the precise-GC build.
        if Self::GC_IMMIX {
            for sid in 0..m.structs.len() {
                let d = self.emit_struct_desc(sid as u32, &m.structs[sid]);
                self.struct_descs.push(d);
            }
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
            self.fns.insert(f.name.clone(), fv);
        }

        for f in &m.funcs {
            self.translate_fn(f)?;
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

    /// Emit a GC safepoint poll (`zap_safepoint()`), the only place a collection
    /// can run. Called at the top of every basic block (FORK 4a: covers function
    /// entry and loop back-edges). No-op under Boehm.
    #[cfg(feature = "gc-immix")]
    fn gc_safepoint(&self) -> Result<(), String> {
        let sp = self.fns["zap_safepoint"];
        self.builder
            .build_call(sp, &[], "")
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Address of the per-thread `zap_shadow_head` TLS global.
    #[cfg(feature = "gc-immix")]
    fn gc_head(&self) -> PointerValue<'ctx> {
        self.module
            .get_global("zap_shadow_head")
            .expect("zap_shadow_head declared")
            .as_pointer_value()
    }

    /// Build and link this function's shadow frame in the entry block, null-init
    /// its roots, and store the GC-pointer-typed parameters. Sets `self.gc_frame`.
    /// A no-op (leaves `gc_frame = None`) for functions with no GC pointers.
    #[cfg(feature = "gc-immix")]
    fn gc_setup_frame(&mut self, f: &zir::Func, fv: FunctionValue<'ctx>) -> Result<(), String> {
        self.gc_frame = None;
        let mut slot_of = vec![-1i32; f.value_types.len()];
        let mut n: u32 = 0;
        for (vid, ty) in f.value_types.iter().enumerate() {
            if ty.is_ptr() {
                slot_of[vid] = n as i32;
                n += 1;
            }
        }
        if n == 0 {
            return Ok(()); // nothing to root
        }
        let p = self.ctx.ptr_type(AddressSpace::default());
        let i64t = self.ctx.i64_type();
        let roots_ty = p.array_type(n);
        let frame_ty = self
            .ctx
            .struct_type(&[p.into(), i64t.into(), roots_ty.into()], false);
        let b = self.builder;
        let frame = b
            .build_alloca(frame_ty, "gcframe")
            .map_err(|e| e.to_string())?;
        // frame.prev = *head; frame.nroots = n; roots[*] = null; *head = &frame.
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
        b.build_store(nslot, i64t.const_int(n as u64, false))
            .map_err(|e| e.to_string())?;
        let roots = b
            .build_struct_gep(frame_ty, frame, 2, "gc.roots")
            .map_err(|e| e.to_string())?;
        // Zero the roots array in one memset.
        b.build_memset(
            roots,
            8,
            self.ctx.i8_type().const_zero(),
            i64t.const_int((n as u64) * 8, false),
        )
        .map_err(|e| e.to_string())?;
        b.build_store(head, frame).map_err(|e| e.to_string())?;
        self.gc_frame = Some(GcFrame {
            frame_ptr: frame,
            frame_ty,
            slot_of,
            n,
        });
        // Root the GC-pointer-typed parameters.
        for (i, ty) in f.params.iter().enumerate() {
            if ty.is_ptr() {
                if let Some(v) = fv.get_nth_param(i as u32) {
                    self.gc_store_root(i as u32, v)?;
                }
            }
        }
        Ok(())
    }

    /// Store SSA value `vid` into its root slot, if it has one. `val` must be a
    /// pointer value. Called at the end of the value's defining block.
    #[cfg(feature = "gc-immix")]
    fn gc_store_root(&self, vid: u32, val: BasicValueEnum<'ctx>) -> Result<(), String> {
        let Some(frame) = &self.gc_frame else {
            return Ok(());
        };
        let slot = frame.slot_of[vid as usize];
        if slot < 0 || !val.is_pointer_value() {
            return Ok(());
        }
        let b = self.builder;
        let roots = b
            .build_struct_gep(frame.frame_ty, frame.frame_ptr, 2, "gc.roots")
            .map_err(|e| e.to_string())?;
        let p = self.ctx.ptr_type(AddressSpace::default());
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
            .map_err(|e| e.to_string())?
        };
        b.build_store(elem, val.into_pointer_value())
            .map_err(|e| e.to_string())?;
        Ok(())
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
            fv.set_personality_function(self.fns["__gxx_personality_v0"]);
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
        // frame in the entry block, rooting the GC-pointer parameters, before any
        // safepoint can observe them.
        #[cfg(feature = "gc-immix")]
        self.gc_setup_frame(f, fv)?;

        for (bi, b) in f.blocks.iter().enumerate() {
            self.builder.position_at_end(blocks[bi]);
            // A GC safepoint at the top of every block (FORK 4a: covers function
            // entry and loop back-edges) — the only place a collection runs. A
            // landing-pad block starts with the mandatory `landingpad`
            // instruction, so its safepoint + head-restore are emitted in the
            // LandingPad handler instead.
            #[cfg(feature = "gc-immix")]
            if !block_starts_with_landing_pad(b) {
                self.gc_safepoint()?;
            }
            for inst in &b.insts {
                self.inst(f, inst, &mut vals, fv, &blocks)?;
                // Root a GC pointer the instant it is defined — NOT deferred to
                // the end of the block. A block may contain several calls, each an
                // interior GC safepoint; a heap pointer defined by one call must
                // already be in its shadow-frame slot before a *later* same-block
                // call can trigger a cross-goroutine collection, or that object is
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
            Ty::I8 => self.ctx.i8_type().into(),
            Ty::I16 => self.ctx.i16_type().into(),
            Ty::I32 => self.ctx.i32_type().into(),
            Ty::I64 => self.ctx.i64_type().into(),
            Ty::F32 => self.ctx.f32_type().into(),
            Ty::F64 => self.ctx.f64_type().into(),
            Ty::Ptr
            | Ty::Void
            | Ty::Struct(_)
            | Ty::List(_)
            | Ty::Dict(..)
            | Ty::Set(_)
            | Ty::Heap(_)
            | Ty::Array(_)
            | Ty::Chan(_)
            | Ty::Match
            | Ty::Py => self.ctx.ptr_type(AddressSpace::default()).into(),
        }
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
            Ty::I8 => self.ctx.i8_type(),
            Ty::I16 => self.ctx.i16_type(),
            Ty::I32 => self.ctx.i32_type(),
            _ => self.ctx.i64_type(),
        }
    }

    fn float_ty(&self, t: Ty) -> FloatType<'ctx> {
        match t {
            Ty::F32 => self.ctx.f32_type(),
            _ => self.ctx.f64_type(),
        }
    }

    fn inst(
        &self,
        f: &Func,
        inst: &Inst,
        vals: &mut [Option<BasicValueEnum<'ctx>>],
        fv: FunctionValue<'ctx>,
        blocks: &[BasicBlock<'ctx>],
    ) -> Result<(), String> {
        let get = |vals: &[Option<BasicValueEnum<'ctx>>], v: u32| -> BasicValueEnum<'ctx> {
            vals[v as usize].expect("ZIR value used before definition")
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
                    match op {
                        Add => b.build_float_add(lf, rf, "fadd"),
                        Sub => b.build_float_sub(lf, rf, "fsub"),
                        Mul => b.build_float_mul(lf, rf, "fmul"),
                        Div => b.build_float_div(lf, rf, "fdiv"),
                        Rem => b.build_float_rem(lf, rf, "frem"),
                        _ => return Err("float op not supported".into()),
                    }
                    .map_err(|e| e.to_string())?
                    .into()
                } else {
                    let (li, ri) = (l.into_int_value(), r.into_int_value());
                    use zir::BinOp::*;
                    match op {
                        Add => b.build_int_add(li, ri, "add"),
                        Sub => b.build_int_sub(li, ri, "sub"),
                        Mul => b.build_int_mul(li, ri, "mul"),
                        Div => b.build_int_signed_div(li, ri, "sdiv"),
                        Rem => b.build_int_signed_rem(li, ri, "srem"),
                        And => b.build_and(li, ri, "and"),
                        Or => b.build_or(li, ri, "or"),
                        Xor => b.build_xor(li, ri, "xor"),
                        Shl => b.build_left_shift(li, ri, "shl"),
                        Shr => b.build_right_shift(li, ri, true, "ashr"),
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
                let c = b
                    .build_int_compare(
                        int_pred(*pred),
                        get(vals, *lhs).into_int_value(),
                        get(vals, *rhs).into_int_value(),
                        "icmp",
                    )
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
                    CastKind::Trunc => b
                        .build_int_truncate(v.into_int_value(), self.int_ty(to), "trunc")
                        .map_err(|e| e.to_string())?
                        .into(),
                    CastKind::SIToFP => b
                        .build_signed_int_to_float(v.into_int_value(), self.float_ty(to), "sitofp")
                        .map_err(|e| e.to_string())?
                        .into(),
                    CastKind::FPToSI => b
                        .build_float_to_signed_int(v.into_float_value(), self.int_ty(to), "fptosi")
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
                let p = self.entry_alloca(self.basic(*pointee), "slot")?;
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
                    if matches!(ty, Ty::Ptr) {
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
                let i8t = self.ctx.i8_type();
                let arr_vals: Vec<_> = bytes
                    .iter()
                    .map(|byte| i8t.const_int(*byte as u64, false))
                    .collect();
                let arr = i8t.const_array(&arr_vals);
                let global = self.module.add_global(arr.get_type(), None, "str");
                global.set_initializer(&arr);
                global.set_constant(true);
                let data_ptr = global.as_pointer_value();
                let len = self.ctx.i64_type().const_int(bytes.len() as u64, false);
                let str_new = self.fns["zap_str_new"];
                let cs = b
                    .build_call(str_new, &[data_ptr.into(), len.into()], "strnew")
                    .map_err(|e| e.to_string())?;
                vals[*dest as usize] = cs.try_as_basic_value().left();
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
            Inst::FieldGet {
                dest,
                base,
                sid,
                idx,
            } => {
                let st = self.structs[*sid as usize];
                let p = get(vals, *base).into_pointer_value();
                let fp = b
                    .build_struct_gep(st, p, *idx, "fieldptr")
                    .map_err(|_| "invalid field index".to_string())?;
                let ty = self.basic(f.value_ty(*dest));
                let v = b.build_load(ty, fp, "field").map_err(|e| e.to_string())?;
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
                let fp = b
                    .build_struct_gep(st, p, *idx, "fieldptr")
                    .map_err(|_| "invalid field index".to_string())?;
                b.build_store(fp, get(vals, *val))
                    .map_err(|e| e.to_string())?;
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
                // A single catch-all clause (`catch ptr null`) matches any Zap
                // exception; selector-based `except` dispatch is deferred, so the
                // selector is unused. `zap_eh_begin_catch` turns the in-flight
                // `_Unwind_Exception*` into the Zap exception object.
                let ptr_ty = self.ctx.ptr_type(AddressSpace::default());
                let lp_ty = self
                    .ctx
                    .struct_type(&[ptr_ty.into(), self.ctx.i32_type().into()], false);
                let null = ptr_ty.const_null();
                let pf = self.fns["__gxx_personality_v0"];
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
                    self.gc_safepoint()?;
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
            } => {
                let lv = get(vals, *lhs);
                let rv = get(vals, *rhs);
                let res =
                    self.array_binop(*op, *out, *lhs_dtype, *rhs_dtype, lv, rv, *scalar, fv)?;
                vals[*dest as usize] = Some(res);
            }
            Inst::Spawn { func, args, owner } => {
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
                        b.build_call(spawn, &[thunk_ptr.into(), env.into(), scope_v.into()], "")
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
        }
        Ok(())
    }

    /// The static interface dispatch table for the `(concrete_sid, iid)` pair
    /// (D-0016): a constant `[N x ptr]` global whose slot *k* is the address of
    /// the implementor's `Struct__<method_k>`, in interface declaration order.
    /// Built once per pair and cached.
    fn itable_global(&self, concrete_sid: u32, iid: u32) -> Result<PointerValue<'ctx>, String> {
        if let Some(g) = self.itables.borrow().get(&(concrete_sid, iid)) {
            return Ok(*g);
        }
        let iface = &self.interfaces[iid as usize];
        let concrete = &self.struct_names[concrete_sid as usize];
        let p = self.ctx.ptr_type(AddressSpace::default());
        let mut slots: Vec<PointerValue<'ctx>> = Vec::with_capacity(iface.methods.len());
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
        // Load slot `method` of the itable (`[N x ptr]`).
        let arr_ty = p.array_type(iface.methods.len() as u32);
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
            let v = match msig.params.get(i) {
                Some(pt) => self.abi_arg(self.abi_ty(*pt), v)?,
                None => v,
            };
            argvals.push(v);
        }
        Ok((callee, fnty, argvals))
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
        // The thunk is each goroutine's base frame (ticket #47,
        // design/0005-concurrency.md "Interaction with exception unwinding"):
        // unwinding must stop here, not run off the green stack into the
        // makecontext bootstrap. So the call to the target is an `invoke` with a
        // catch-all landing pad that routes the uncaught exception to
        // `zap_g_uncaught`, which decides at runtime based on the coroutine's
        // owning scope (ticket #50, D-0044): a scoped coroutine records its
        // failure on the scope (which re-raises it — possibly as an aggregate
        // `ExceptionGroup` — at its dedent) and returns here so the coroutine ends
        // cleanly; a root/detached coroutine aborts the program with the message
        // (exit 1), exactly as `zap_uncaught` did before. In-goroutine `try/except`
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
        thunk.set_personality_function(self.fns["__gxx_personality_v0"]);
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
        // Normal path: the goroutine returned; hand `null` back to `zap_spawn`.
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
                self.fns["__gxx_personality_v0"],
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
        fv: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let b = self.builder;
        let i64t = self.ctx.i64_type();
        let i32t = self.ctx.i32_type();
        let la = lhs.into_pointer_value();

        // n = len(lhs); res = new(out, n). Both element buffers come back as
        // raw pointers via `zap_array_data`, so this loop never needs to know
        // the array object's field layout.
        let n = self
            .call1("zap_array_len", &[la.into()], "n")?
            .into_int_value();
        let code = i64t.const_int(out.code() as u64, false);
        let res = self
            .call1("zap_array_new", &[code.into(), n.into()], "arr")?
            .into_pointer_value();
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

        // vend = n & ~(VW-1): the largest multiple of VW not exceeding n.
        let mask = i64t.const_int(!(VW - 1), false);
        let vend = b.build_and(n, mask, "vend").map_err(|e| e.to_string())?;

        let entry = b.get_insert_block().unwrap();
        let vhead = self.ctx.append_basic_block(fv, "arr.vhead");
        let vbody = self.ctx.append_basic_block(fv, "arr.vbody");
        let rhead = self.ctx.append_basic_block(fv, "arr.rhead");
        let rbody = self.ctx.append_basic_block(fv, "arr.rbody");
        let done = self.ctx.append_basic_block(fv, "arr.done");
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
        let resvec = self.vec_arith(op, out_float, lvec, rvec)?;
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
        let ressc = self.scalar_arith(op, out_float, lsc, rsc)?;
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

    /// The `VW`-lane LLVM vector type for a dtype's storage element.
    fn dtype_vec(&self, d: zir::Dtype, vw: u32) -> VectorType<'ctx> {
        match d.storage_ty() {
            Ty::F64 => self.ctx.f64_type().vec_type(vw),
            Ty::I64 => self.ctx.i64_type().vec_type(vw),
            Ty::I8 => self.ctx.i8_type().vec_type(vw),
            // `storage_ty` yields only f64/i64/i8 for the array dtypes.
            _ => unreachable!("array dtype storage is f64/i64/i8"),
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
            // float -> float (f64 is the only float dtype, so `src == dst`
            // already returned); float -> int never arises (promotion only
            // moves up the lattice toward float).
            _ => v,
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
            _ => v,
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
        v.map_err(|e| e.to_string())
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
            match op {
                Add => b.build_float_add(lf, rf, "fadd"),
                Sub => b.build_float_sub(lf, rf, "fsub"),
                Mul => b.build_float_mul(lf, rf, "fmul"),
                _ => return Err("unsupported element-wise array op".into()),
            }
            .map_err(|e| e.to_string())?
            .into()
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
                let rv = vals[*v as usize].expect("ret value");
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

fn int_pred(p: CmpPred) -> IntPredicate {
    match p {
        CmpPred::Eq => IntPredicate::EQ,
        CmpPred::Ne => IntPredicate::NE,
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
mod tests {
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
        let mut cg = Cg::new(&ctx, &llmod, &builder);
        cg.translate(&zir).expect("translate");
        llmod
            .verify()
            .map_err(|e| e.to_string())
            .expect("valid LLVM module");
        llmod.print_to_string().to_string()
    }

    // The payoff of ticket #24: element-wise array arithmetic must lower to
    // explicit LLVM vector instructions (SIMD), not a scalar loop.
    #[test]
    fn array_elementwise_vectorizes_f64() {
        let ir = llvm_ir(
            "def main():\n    auto a = array[f64]([1.0, 2.0, 3.0, 4.0, 5.0])\n    auto b = array[f64]([5.0, 4.0, 3.0, 2.0, 1.0])\n    print(sum(a + b))\n",
        );
        assert!(
            ir.contains("fadd <4 x double>"),
            "expected a vectorized fadd in IR:\n{ir}"
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
        assert!(
            ir.contains("fmul <4 x double>"),
            "expected a vectorized fmul for scalar broadcast:\n{ir}"
        );
    }
}
