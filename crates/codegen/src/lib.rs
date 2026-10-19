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
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::{
    BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FloatType, IntType, VectorType,
};
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue, VectorValue};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate, OptimizationLevel};

use zapc_ir::{self as zir, lower_module, CastKind, CmpPred, Func, Inst, Term, Ty};
use zapc_syntax::ast::Module as AstModule;

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
        let mut cg = Cg::new(&ctx, &llmod, &builder);
        cg.translate(&zir_module)?;

        llmod
            .verify()
            .map_err(|e| format!("internal error: invalid LLVM module:\n{}", e.to_string()))?;

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
        tm.write_to_file(&llmod, FileType::Object, obj_path)
            .map_err(|e| format!("failed to write object file: {}", e.to_string()))?;
        Ok(())
    }
}

/// Compile an AST module to a native executable (object + `cc` link).
/// The Zap C runtime, compiled and linked with each program.
const RUNTIME_C: &str = include_str!("zaprt.c");

pub fn build_executable(module: &AstModule, out_path: &Path) -> Result<(), String> {
    let obj_path = out_path.with_extension("o");
    LlvmBackend.compile_object(module, &obj_path)?;

    // Write the runtime to a temp .c and let cc compile + link it in.
    let rt_path = std::env::temp_dir().join(format!("zaprt-{}.c", std::process::id()));
    std::fs::write(&rt_path, RUNTIME_C).map_err(|e| format!("failed to write runtime: {e}"))?;

    // `-lgc` links the conservative Boehm collector the runtime allocates
    // through (ticket #20, D-0009); the dev container provides `libgc-dev`.
    // `-lstdc++` pulls in the Itanium unwinding personality and the
    // `__cxa_*` exception entry points the runtime calls (ticket #22); the
    // actual stack unwinder (`_Unwind_*`) comes from libgcc, linked by default.
    let mut cmd = Command::new("cc");
    cmd.arg(&obj_path)
        .arg(&rt_path)
        .arg("-o")
        .arg(out_path)
        .arg("-lgc")
        .arg("-lstdc++")
        // `-lm` provides libm `pow`/`floor`, used by the scalar `**` and `//`
        // runtime helpers (ticket #19).
        .arg("-lm")
        // `-lpcre2-8` provides the 8-bit PCRE2 API the regex runtime wraps
        // (ticket #23); the dev container / CI provide `libpcre2-dev`.
        .arg("-lpcre2-8")
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
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run linker (cc): {e}"))?;
    let _ = std::fs::remove_file(&obj_path);
    let _ = std::fs::remove_file(&rt_path);
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
        }
    }

    fn translate(&mut self, m: &zir::Module) -> Result<(), String> {
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
        self.declare_rt("zap_chan_new", &[i.into()], Some(p.into()));
        self.declare_rt("zap_chan_send_i64", &[p.into(), i.into()], None);
        self.declare_rt("zap_chan_send_ptr", &[p.into(), p.into()], None);
        self.declare_rt("zap_chan_recv_i64", &[p.into()], Some(i.into()));
        self.declare_rt("zap_chan_recv_ptr", &[p.into()], Some(p.into()));
        self.declare_rt("zap_chan_close", &[p.into()], None);

        // Build LLVM struct types (fields are scalars or pointers, so no
        // recursive-by-value ordering concerns).
        for sl in &m.structs {
            let field_tys: Vec<BasicTypeEnum> = sl.fields.iter().map(|t| self.basic(*t)).collect();
            self.structs.push(self.ctx.struct_type(&field_tys, false));
            self.struct_inline.push(sl.inline);
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

        for (bi, b) in f.blocks.iter().enumerate() {
            self.builder.position_at_end(blocks[bi]);
            for inst in &b.insts {
                self.inst(f, inst, &mut vals, fv, &blocks)?;
            }
            self.term(f, &b.term, &blocks, &vals, fv)?;
        }
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
            let slot = self
                .builder
                .build_alloca(v.get_type(), "ret.spill")
                .map_err(|e| e.to_string())?;
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
                let p = b
                    .build_alloca(self.basic(*pointee), "slot")
                    .map_err(|e| e.to_string())?;
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
                let size = st
                    .size_of()
                    .ok_or_else(|| "struct has no size".to_string())?;
                let alloc = self.fns["zap_alloc"];
                let cs = b
                    .build_call(alloc, &[size.into()], "structnew")
                    .map_err(|e| e.to_string())?;
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
                let begin = self.fns["zap_eh_begin_catch"];
                let cs = b
                    .build_call(begin, &[exc_ptr.into()], "begincatch")
                    .map_err(|e| e.to_string())?;
                vals[*dest as usize] = cs.try_as_basic_value().left();
            }
            Inst::ArrayBinOp {
                dest,
                op,
                dtype,
                lhs,
                rhs,
                scalar,
            } => {
                let lv = get(vals, *lhs);
                let rv = get(vals, *rhs);
                let res = self.array_binop(*op, *dtype, lv, rv, *scalar, fv)?;
                vals[*dest as usize] = Some(res);
            }
            Inst::Spawn { func, args } => {
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
                let size = env_struct
                    .size_of()
                    .ok_or_else(|| "spawn closure has no size".to_string())?;
                let alloc = self.fns["zap_alloc"];
                let env = b
                    .build_call(alloc, &[size.into()], "spawnenv")
                    .map_err(|e| e.to_string())?
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
                let spawn = self.fns["zap_spawn"];
                b.build_call(spawn, &[thunk_ptr.into(), env.into()], "")
                    .map_err(|e| e.to_string())?;
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
        }
        Ok(())
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
        // catch-all landing pad that reports the uncaught exception via
        // `zap_uncaught` (message + exit 1), mirroring the implicit top-level
        // handler in `main`. In-goroutine `try/except` is unaffected (it unwinds
        // through normal CFI on the green stack); this only catches what reaches
        // the top. Requires the Itanium personality + an unwind table.
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
        b.build_call(self.fns["zap_uncaught"], &[exc.into()], "")
            .map_err(|e| e.to_string())?;
        b.build_unreachable().map_err(|e| e.to_string())?;
        if let Some(bb) = saved {
            b.position_at_end(bb);
        }
        Ok(thunk)
    }

    /// Lower an element-wise array op to a SIMD loop (ticket #24): a vector body
    /// that processes `VW` lanes per iteration with explicit LLVM vector
    /// load/arith/store, followed by a scalar remainder loop for the final
    /// `len % VW` elements. `lhs` is the source array pointer; `rhs` is either a
    /// second array pointer (`!scalar`) or a scalar broadcast across all lanes
    /// (`scalar`). Returns the freshly allocated result array pointer. This is
    /// the deliberate exception to the "mechanical" rule — the whole point of
    /// the ticket is that the arithmetic vectorizes.
    fn array_binop(
        &self,
        op: zir::BinOp,
        dtype: zir::Dtype,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
        scalar: bool,
        fv: FunctionValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let b = self.builder;
        let i64t = self.ctx.i64_type();
        let i32t = self.ctx.i32_type();
        let la = lhs.into_pointer_value();

        // n = len(lhs); res = new(dtype, n). Both element buffers come back as
        // raw pointers via `zap_array_data`, so this loop never needs to know
        // the array object's field layout.
        let n = self
            .call1("zap_array_len", &[la.into()], "n")?
            .into_int_value();
        let code = i64t.const_int(dtype.code() as u64, false);
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

        let is_float = dtype.is_float();
        let elem_ty = self.basic(dtype.ty());
        const VW: u64 = 4;
        let vec_ty: VectorType<'ctx> = if is_float {
            self.ctx.f64_type().vec_type(VW as u32)
        } else {
            i64t.vec_type(VW as u32)
        };

        // Broadcast the scalar operand into a lane vector for the vector body.
        let svec = if scalar {
            let mut v = vec_ty.get_undef();
            for lane in 0..VW {
                let idx = i32t.const_int(lane, false);
                v = b
                    .build_insert_element(v, rhs, idx, "splat")
                    .map_err(|e| e.to_string())?;
            }
            Some(v)
        } else {
            None
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
        let lvec = self.vec_load(vec_ty, elem_ty, lp, i, "lv")?;
        let rvec = match rp {
            Some(rp) => self.vec_load(vec_ty, elem_ty, rp, i, "rv")?,
            None => svec.unwrap(),
        };
        let resvec = self.vec_arith(op, is_float, lvec, rvec)?;
        let optr = unsafe {
            b.build_gep(elem_ty, op_ptr, &[i], "optr")
                .map_err(|e| e.to_string())?
        };
        let st = b.build_store(optr, resvec).map_err(|e| e.to_string())?;
        st.set_alignment(8).map_err(|e| e.to_string())?;
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
        let lsc = self.elem_load(elem_ty, lp, j, "ls")?;
        let rsc = match rp {
            Some(rp) => self.elem_load(elem_ty, rp, j, "rs")?,
            None => rhs,
        };
        let ressc = self.scalar_arith(op, is_float, lsc, rsc)?;
        let optr = unsafe {
            b.build_gep(elem_ty, op_ptr, &[j], "optrs")
                .map_err(|e| e.to_string())?
        };
        b.build_store(optr, ressc).map_err(|e| e.to_string())?;
        let jnext = b
            .build_int_add(j, i64t.const_int(1, false), "jnext")
            .map_err(|e| e.to_string())?;
        b.build_unconditional_branch(rhead)
            .map_err(|e| e.to_string())?;
        j_phi.add_incoming(&[(&vend, vhead), (&jnext, rbody)]);

        b.position_at_end(done);
        Ok(res.into())
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

    /// A `VW`-lane vector load from `base[idx]`, aligned to the element (8) so
    /// the buffer need not be vector-aligned.
    fn vec_load(
        &self,
        vec_ty: VectorType<'ctx>,
        elem_ty: BasicTypeEnum<'ctx>,
        base: PointerValue<'ctx>,
        idx: inkwell::values::IntValue<'ctx>,
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
            .set_alignment(8)
            .map_err(|e| e.to_string())?;
        Ok(v.into_vector_value())
    }

    /// A scalar element load from `base[idx]`.
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
