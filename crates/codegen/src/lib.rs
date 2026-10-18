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

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module as LlvmModule;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FloatType, IntType};
use inkwell::values::{BasicValueEnum, FunctionValue};
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

    let status = Command::new("cc")
        .arg(&obj_path)
        .arg(&rt_path)
        .arg("-o")
        .arg(out_path)
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

    fn translate_fn(&mut self, f: &Func) -> Result<(), String> {
        let fv = self.fns[&f.name];
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
                self.inst(f, inst, &mut vals)?;
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
            Ty::Ptr | Ty::Void => self.ctx.ptr_type(AddressSpace::default()).into(),
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
                        self.printf_val("%f", v)?;
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
        }
        Ok(())
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
                b.build_return(Some(&rv)).map_err(|e| e.to_string())?;
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
