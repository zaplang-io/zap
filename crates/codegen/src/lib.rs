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
    let status = Command::new("cc")
        .arg(&obj_path)
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
        .status()
        .map_err(|e| format!("failed to run linker (cc): {e}"))?;
    let _ = std::fs::remove_file(&obj_path);
    let _ = std::fs::remove_file(&rt_path);
    if !status.success() {
        return Err(format!("linker (cc) failed: {status}"));
    }
    Ok(())
}

struct Cg<'a, 'ctx> {
    ctx: &'ctx Context,
    module: &'a LlvmModule<'ctx>,
    builder: &'a Builder<'ctx>,
    fns: std::collections::HashMap<String, FunctionValue<'ctx>>,
    structs: Vec<inkwell::types::StructType<'ctx>>,
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
        self.declare_rt("zap_set_items", &[p.into()], Some(p.into()));

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

        // Build LLVM struct types (fields are scalars or pointers, so no
        // recursive-by-value ordering concerns).
        for sl in &m.structs {
            let field_tys: Vec<BasicTypeEnum> = sl.fields.iter().map(|t| self.basic(*t)).collect();
            self.structs.push(self.ctx.struct_type(&field_tys, false));
        }

        // Declare all functions first (so calls resolve).
        for f in &m.funcs {
            let params: Vec<BasicMetadataTypeEnum> =
                f.params.iter().map(|t| self.basic(*t).into()).collect();
            let fnty = if f.is_main {
                i32t.fn_type(&[], false)
            } else {
                match f.ret {
                    Ty::Void => self.ctx.void_type().fn_type(&params, false),
                    rt => self.basic(rt).fn_type(&params, false),
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
        for (i, slot) in vals.iter_mut().take(f.params.len()).enumerate() {
            *slot = Some(fv.get_nth_param(i as u32).unwrap());
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
            | Ty::Array(_)
            | Ty::Match => self.ctx.ptr_type(AddressSpace::default()).into(),
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
                let argvals: Vec<_> = args.iter().map(|a| get(vals, *a).into()).collect();
                let cs = b
                    .build_call(fv, &argvals, "call")
                    .map_err(|e| e.to_string())?;
                if let Some(d) = dest {
                    if let Some(v) = cs.try_as_basic_value().left() {
                        vals[*d as usize] = Some(v);
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
                let argvals: Vec<BasicValueEnum> = args.iter().map(|a| get(vals, *a)).collect();
                let cont = self.ctx.append_basic_block(fv, "invoke.cont");
                let cs = b
                    .build_invoke(callee, &argvals, cont, blocks[*unwind as usize], "invoke")
                    .map_err(|e| e.to_string())?;
                b.position_at_end(cont);
                if let Some(d) = dest {
                    if let Some(v) = cs.try_as_basic_value().left() {
                        vals[*d as usize] = Some(v);
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
        }
        Ok(())
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
        _fv: FunctionValue<'ctx>,
    ) -> Result<(), String> {
        let b = self.builder;
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
