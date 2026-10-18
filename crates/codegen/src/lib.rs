//! Zap code generation (Phase 1) — lowers the type-checked AST to native code
//! via LLVM (using `inkwell`), behind a small [`Backend`] trait so the backend
//! stays swappable.
//!
//! Scope: the vertical slice — `int64`/`float64`/`bool` scalars, functions
//! (incl. recursion), `let`/assignment/aug-assignment, `if`/`while`/`do-while`/
//! `for`-over-`range`, `return`, calls, arithmetic/comparison, and `print`
//! (via libc `printf`). Anything outside this returns a clear "not yet
//! supported" error rather than miscompiling.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module as LlvmModule;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum};
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate, OptimizationLevel};
use zapc_syntax::ast::*;

/// A code-generation backend.
pub trait Backend {
    /// Compile a (type-checked) module to a native object file at `obj_path`.
    fn compile_object(&self, module: &Module, obj_path: &Path) -> Result<(), String>;
}

/// The LLVM backend.
pub struct LlvmBackend;

impl Backend for LlvmBackend {
    fn compile_object(&self, module: &Module, obj_path: &Path) -> Result<(), String> {
        let ctx = Context::create();
        let llmod = ctx.create_module("zap");
        let builder = ctx.create_builder();
        let mut c = Compiler::new(&ctx, &llmod, &builder);
        c.declare_functions(module)?;
        c.define_functions(module)?;

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

/// Compile `module` to a native executable at `out_path` (object + `cc` link).
pub fn build_executable(module: &Module, out_path: &Path) -> Result<(), String> {
    let obj_path = out_path.with_extension("o");
    LlvmBackend.compile_object(module, &obj_path)?;
    let status = Command::new("cc")
        .arg(&obj_path)
        .arg("-o")
        .arg(out_path)
        .status()
        .map_err(|e| format!("failed to run linker (cc): {e}"))?;
    let _ = std::fs::remove_file(&obj_path);
    if !status.success() {
        return Err(format!("linker (cc) failed: {status}"));
    }
    Ok(())
}

type Scope<'ctx> = HashMap<String, (PointerValue<'ctx>, BasicTypeEnum<'ctx>)>;

struct Compiler<'a, 'ctx> {
    ctx: &'ctx Context,
    module: &'a LlvmModule<'ctx>,
    builder: &'a Builder<'ctx>,
    fns: HashMap<String, FunctionValue<'ctx>>,
    scopes: Vec<Scope<'ctx>>,
    cur_fn: Option<FunctionValue<'ctx>>,
    /// (continue-target, break-target) for the enclosing loops.
    loops: Vec<(BasicBlock<'ctx>, BasicBlock<'ctx>)>,
}

impl<'a, 'ctx> Compiler<'a, 'ctx> {
    fn new(
        ctx: &'ctx Context,
        module: &'a LlvmModule<'ctx>,
        builder: &'a Builder<'ctx>,
    ) -> Compiler<'a, 'ctx> {
        Compiler {
            ctx,
            module,
            builder,
            fns: HashMap::new(),
            scopes: Vec::new(),
            cur_fn: None,
            loops: Vec::new(),
        }
    }

    // --- scopes ---

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
    fn declare_var(&mut self, name: &str, ptr: PointerValue<'ctx>, ty: BasicTypeEnum<'ctx>) {
        self.scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), (ptr, ty));
    }
    fn lookup_var(&self, name: &str) -> Option<(PointerValue<'ctx>, BasicTypeEnum<'ctx>)> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    // --- type mapping ---

    fn basic_type(&self, t: &Type) -> Result<BasicTypeEnum<'ctx>, String> {
        match t {
            Type::Named { name, .. } => match name.as_str() {
                "int64" | "i64" => Ok(self.ctx.i64_type().into()),
                "int32" | "i32" => Ok(self.ctx.i32_type().into()),
                "float64" | "f64" => Ok(self.ctx.f64_type().into()),
                "float32" | "f32" => Ok(self.ctx.f32_type().into()),
                "bool" => Ok(self.ctx.bool_type().into()),
                other => Err(format!("type '{other}' is not supported by codegen yet")),
            },
            _ => Err("only named scalar types are supported by codegen yet".into()),
        }
    }

    // --- declaration pass ---

    fn declare_functions(&mut self, module: &Module) -> Result<(), String> {
        // printf: i32 printf(ptr, ...)
        let i32t = self.ctx.i32_type();
        let ptr = self.ctx.ptr_type(AddressSpace::default());
        let printf_ty = i32t.fn_type(&[ptr.into()], true);
        self.module.add_function("printf", printf_ty, None);

        for item in &module.items {
            if let Item::Function(f) = item {
                let fv = if f.name == "main" {
                    let fnty = i32t.fn_type(&[], false);
                    self.module.add_function("main", fnty, None)
                } else {
                    let mut params: Vec<BasicMetadataTypeEnum> = Vec::new();
                    for p in &f.params {
                        let pt =
                            p.ty.as_ref()
                                .ok_or_else(|| format!("parameter '{}' needs a type", p.name))?;
                        params.push(self.basic_type(pt)?.into());
                    }
                    let fnty = match &f.ret {
                        Some(rt) => self.basic_type(rt)?.fn_type(&params, false),
                        None => self.ctx.void_type().fn_type(&params, false),
                    };
                    self.module.add_function(&f.name, fnty, None)
                };
                self.fns.insert(f.name.clone(), fv);
            }
        }
        Ok(())
    }

    // --- definition pass ---

    fn define_functions(&mut self, module: &Module) -> Result<(), String> {
        for item in &module.items {
            if let Item::Function(f) = item {
                self.define_function(f)?;
            }
        }
        Ok(())
    }

    fn define_function(&mut self, f: &Function) -> Result<(), String> {
        let fv = *self.fns.get(&f.name).unwrap();
        let entry = self.ctx.append_basic_block(fv, "entry");
        self.builder.position_at_end(entry);
        self.cur_fn = Some(fv);
        self.push_scope();

        if f.name != "main" {
            for (i, p) in f.params.iter().enumerate() {
                let pv = fv.get_nth_param(i as u32).unwrap();
                let bt = pv.get_type();
                let ptr = self
                    .builder
                    .build_alloca(bt, &p.name)
                    .map_err(|e| e.to_string())?;
                self.builder
                    .build_store(ptr, pv)
                    .map_err(|e| e.to_string())?;
                self.declare_var(&p.name, ptr, bt);
            }
        }

        for st in &f.body.stmts {
            self.gen_stmt(st)?;
        }

        // Ensure the current block is terminated.
        if !self.block_terminated() {
            if f.name == "main" {
                let zero = self.ctx.i32_type().const_int(0, false);
                self.builder
                    .build_return(Some(&zero))
                    .map_err(|e| e.to_string())?;
            } else if fv.get_type().get_return_type().is_none() {
                self.builder.build_return(None).map_err(|e| e.to_string())?;
            } else {
                self.builder
                    .build_unreachable()
                    .map_err(|e| e.to_string())?;
            }
        }

        self.pop_scope();
        self.cur_fn = None;
        Ok(())
    }

    fn block_terminated(&self) -> bool {
        self.builder
            .get_insert_block()
            .and_then(|b| b.get_terminator())
            .is_some()
    }

    fn ensure_branch(&self, dest: BasicBlock<'ctx>) -> Result<(), String> {
        if !self.block_terminated() {
            self.builder
                .build_unconditional_branch(dest)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    // --- statements ---

    fn gen_stmt(&mut self, st: &Stmt) -> Result<(), String> {
        match st {
            Stmt::Let {
                ty, name, value, ..
            } => {
                let (val, vty) = self.gen_expr(value)?;
                let declared = match ty {
                    Some(t) => self.basic_type(t)?,
                    None => vty,
                };
                let val = self.coerce(val, declared)?;
                let ptr = self
                    .builder
                    .build_alloca(declared, name)
                    .map_err(|e| e.to_string())?;
                self.builder
                    .build_store(ptr, val)
                    .map_err(|e| e.to_string())?;
                self.declare_var(name, ptr, declared);
                Ok(())
            }
            Stmt::Assign { target, value, .. } => {
                let (val, _) = self.gen_expr(value)?;
                if let Expr::Name(n, _) = target {
                    let (ptr, ty) = self
                        .lookup_var(n)
                        .ok_or_else(|| format!("unknown variable '{n}'"))?;
                    let val = self.coerce(val, ty)?;
                    self.builder
                        .build_store(ptr, val)
                        .map_err(|e| e.to_string())?;
                    Ok(())
                } else {
                    Err("only simple variable assignment is supported by codegen yet".into())
                }
            }
            Stmt::AugAssign {
                target, op, value, ..
            } => {
                if let Expr::Name(n, _) = target {
                    let (ptr, ty) = self
                        .lookup_var(n)
                        .ok_or_else(|| format!("unknown variable '{n}'"))?;
                    let cur = self
                        .builder
                        .build_load(ty, ptr, n)
                        .map_err(|e| e.to_string())?;
                    let (rhs, rty) = self.gen_expr(value)?;
                    let (res, _) = self.gen_binop(*op, cur, ty, rhs, rty)?;
                    let res = self.coerce(res, ty)?;
                    self.builder
                        .build_store(ptr, res)
                        .map_err(|e| e.to_string())?;
                    Ok(())
                } else {
                    Err("only simple variable aug-assignment is supported by codegen yet".into())
                }
            }
            Stmt::Expr(e) => {
                self.gen_expr(e)?;
                Ok(())
            }
            Stmt::If {
                branches,
                else_block,
                ..
            } => self.gen_if(branches, else_block.as_ref()),
            Stmt::While { cond, body, .. } => self.gen_while(cond, body),
            Stmt::DoWhile { body, cond, .. } => self.gen_do_while(body, cond),
            Stmt::For {
                target, iter, body, ..
            } => self.gen_for(target, iter, body),
            Stmt::Return { value, .. } => self.gen_return(value.as_ref()),
            Stmt::Break(_) => {
                let (_, brk) = *self.loops.last().ok_or("'break' outside a loop")?;
                self.builder
                    .build_unconditional_branch(brk)
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
            Stmt::Continue(_) => {
                let (cont, _) = *self.loops.last().ok_or("'continue' outside a loop")?;
                self.builder
                    .build_unconditional_branch(cont)
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
            Stmt::Raise { .. }
            | Stmt::Try { .. }
            | Stmt::With { .. }
            | Stmt::Match { .. }
            | Stmt::Spawn { .. } => {
                Err("this statement is not supported by codegen yet (Phase 1 slice)".into())
            }
        }
    }

    fn gen_block(&mut self, b: &Block) -> Result<(), String> {
        self.push_scope();
        for st in &b.stmts {
            self.gen_stmt(st)?;
        }
        self.pop_scope();
        Ok(())
    }

    fn gen_if(
        &mut self,
        branches: &[(Expr, Block)],
        else_block: Option<&Block>,
    ) -> Result<(), String> {
        let func = self.cur_fn.unwrap();
        let merge = self.ctx.append_basic_block(func, "ifcont");
        for (cond, body) in branches {
            let (cv, cty) = self.gen_expr(cond)?;
            let cond_i1 = self.to_bool(cv, cty)?;
            let then_bb = self.ctx.append_basic_block(func, "then");
            let next_bb = self.ctx.append_basic_block(func, "ifnext");
            self.builder
                .build_conditional_branch(cond_i1, then_bb, next_bb)
                .map_err(|e| e.to_string())?;
            self.builder.position_at_end(then_bb);
            self.gen_block(body)?;
            self.ensure_branch(merge)?;
            self.builder.position_at_end(next_bb);
        }
        if let Some(eb) = else_block {
            self.gen_block(eb)?;
        }
        self.ensure_branch(merge)?;
        self.builder.position_at_end(merge);
        Ok(())
    }

    fn gen_while(&mut self, cond: &Expr, body: &Block) -> Result<(), String> {
        let func = self.cur_fn.unwrap();
        let cond_bb = self.ctx.append_basic_block(func, "while.cond");
        let body_bb = self.ctx.append_basic_block(func, "while.body");
        let end_bb = self.ctx.append_basic_block(func, "while.end");
        self.builder
            .build_unconditional_branch(cond_bb)
            .map_err(|e| e.to_string())?;
        self.builder.position_at_end(cond_bb);
        let (cv, cty) = self.gen_expr(cond)?;
        let cond_i1 = self.to_bool(cv, cty)?;
        self.builder
            .build_conditional_branch(cond_i1, body_bb, end_bb)
            .map_err(|e| e.to_string())?;
        self.builder.position_at_end(body_bb);
        self.loops.push((cond_bb, end_bb));
        self.gen_block(body)?;
        self.loops.pop();
        self.ensure_branch(cond_bb)?;
        self.builder.position_at_end(end_bb);
        Ok(())
    }

    fn gen_do_while(&mut self, body: &Block, cond: &Expr) -> Result<(), String> {
        let func = self.cur_fn.unwrap();
        let body_bb = self.ctx.append_basic_block(func, "do.body");
        let cond_bb = self.ctx.append_basic_block(func, "do.cond");
        let end_bb = self.ctx.append_basic_block(func, "do.end");
        self.builder
            .build_unconditional_branch(body_bb)
            .map_err(|e| e.to_string())?;
        self.builder.position_at_end(body_bb);
        self.loops.push((cond_bb, end_bb));
        self.gen_block(body)?;
        self.loops.pop();
        self.ensure_branch(cond_bb)?;
        self.builder.position_at_end(cond_bb);
        let (cv, cty) = self.gen_expr(cond)?;
        let cond_i1 = self.to_bool(cv, cty)?;
        self.builder
            .build_conditional_branch(cond_i1, body_bb, end_bb)
            .map_err(|e| e.to_string())?;
        self.builder.position_at_end(end_bb);
        Ok(())
    }

    fn gen_for(&mut self, target: &Expr, iter: &Expr, body: &Block) -> Result<(), String> {
        // Only `range(...)` iteration is supported in the Phase 1 slice.
        let (start, stop, step) = match iter {
            Expr::Call { callee, args, .. } if is_name(callee, "range") => {
                let vals: Result<Vec<_>, String> = args
                    .iter()
                    .map(|a| {
                        let (v, t) = self.gen_expr(&a.value)?;
                        self.coerce(v, self.ctx.i64_type().into()).map(|v| (v, t))
                    })
                    .collect();
                let vals = vals?;
                let i64t = self.ctx.i64_type();
                match vals.len() {
                    1 => (
                        i64t.const_int(0, false).into(),
                        vals[0].0,
                        i64t.const_int(1, false).into(),
                    ),
                    2 => (vals[0].0, vals[1].0, i64t.const_int(1, false).into()),
                    3 => (vals[0].0, vals[1].0, vals[2].0),
                    n => return Err(format!("range() takes 1-3 arguments, got {n}")),
                }
            }
            _ => return Err("only `for ... in range(...)` is supported by codegen yet".into()),
        };

        let name = match target {
            Expr::Name(n, _) => n.clone(),
            _ => return Err("only a simple loop variable is supported by codegen yet".into()),
        };

        let func = self.cur_fn.unwrap();
        let i64t = self.ctx.i64_type();
        self.push_scope();
        let var = self
            .builder
            .build_alloca(i64t, &name)
            .map_err(|e| e.to_string())?;
        self.builder
            .build_store(var, start)
            .map_err(|e| e.to_string())?;
        self.declare_var(&name, var, i64t.into());

        let cond_bb = self.ctx.append_basic_block(func, "for.cond");
        let body_bb = self.ctx.append_basic_block(func, "for.body");
        let incr_bb = self.ctx.append_basic_block(func, "for.incr");
        let end_bb = self.ctx.append_basic_block(func, "for.end");

        self.builder
            .build_unconditional_branch(cond_bb)
            .map_err(|e| e.to_string())?;
        self.builder.position_at_end(cond_bb);
        let cur = self
            .builder
            .build_load(i64t, var, &name)
            .map_err(|e| e.to_string())?;
        let cmp = self
            .builder
            .build_int_compare(
                IntPredicate::SLT,
                cur.into_int_value(),
                stop.into_int_value(),
                "for.cmp",
            )
            .map_err(|e| e.to_string())?;
        self.builder
            .build_conditional_branch(cmp, body_bb, end_bb)
            .map_err(|e| e.to_string())?;

        self.builder.position_at_end(body_bb);
        self.loops.push((incr_bb, end_bb));
        for st in &body.stmts {
            self.gen_stmt(st)?;
        }
        self.loops.pop();
        self.ensure_branch(incr_bb)?;

        self.builder.position_at_end(incr_bb);
        let cur2 = self
            .builder
            .build_load(i64t, var, &name)
            .map_err(|e| e.to_string())?;
        let next = self
            .builder
            .build_int_add(cur2.into_int_value(), step.into_int_value(), "for.next")
            .map_err(|e| e.to_string())?;
        self.builder
            .build_store(var, next)
            .map_err(|e| e.to_string())?;
        self.builder
            .build_unconditional_branch(cond_bb)
            .map_err(|e| e.to_string())?;

        self.builder.position_at_end(end_bb);
        self.pop_scope();
        Ok(())
    }

    fn gen_return(&mut self, value: Option<&Expr>) -> Result<(), String> {
        let fv = self.cur_fn.unwrap();
        let ret_ty = fv.get_type().get_return_type();
        match value {
            Some(e) => {
                let (v, _) = self.gen_expr(e)?;
                let v = match ret_ty {
                    Some(rt) => self.coerce(v, rt)?,
                    None => v,
                };
                self.builder
                    .build_return(Some(&v))
                    .map_err(|e| e.to_string())?;
            }
            None => match ret_ty {
                None => {
                    self.builder.build_return(None).map_err(|e| e.to_string())?;
                }
                Some(rt) => {
                    // e.g. a bare `return` in `main`: return a zero of the type.
                    let zero = rt.into_int_type().const_int(0, false);
                    self.builder
                        .build_return(Some(&zero))
                        .map_err(|e| e.to_string())?;
                }
            },
        }
        Ok(())
    }

    // --- expressions ---

    fn gen_expr(
        &mut self,
        e: &Expr,
    ) -> Result<(BasicValueEnum<'ctx>, BasicTypeEnum<'ctx>), String> {
        match e {
            Expr::Int(s, _) => {
                let v = parse_int_lit(s);
                let t = self.ctx.i64_type();
                Ok((t.const_int(v as u64, false).into(), t.into()))
            }
            Expr::Float(s, _) => {
                let v: f64 = s.replace('_', "").parse().unwrap_or(0.0);
                let t = self.ctx.f64_type();
                Ok((t.const_float(v).into(), t.into()))
            }
            Expr::Bool(b, _) => {
                let t = self.ctx.bool_type();
                Ok((t.const_int(*b as u64, false).into(), t.into()))
            }
            Expr::Name(n, _) => {
                let (ptr, ty) = self
                    .lookup_var(n)
                    .ok_or_else(|| format!("unknown variable '{n}'"))?;
                let v = self
                    .builder
                    .build_load(ty, ptr, n)
                    .map_err(|e| e.to_string())?;
                Ok((v, ty))
            }
            Expr::Unary { op, expr, .. } => {
                let (v, t) = self.gen_expr(expr)?;
                match op {
                    UnOp::Neg => {
                        if v.is_float_value() {
                            let r = self
                                .builder
                                .build_float_neg(v.into_float_value(), "neg")
                                .map_err(|e| e.to_string())?;
                            Ok((r.into(), t))
                        } else {
                            let r = self
                                .builder
                                .build_int_neg(v.into_int_value(), "neg")
                                .map_err(|e| e.to_string())?;
                            Ok((r.into(), t))
                        }
                    }
                    UnOp::Pos => Ok((v, t)),
                    UnOp::Not => {
                        let b = self.to_bool(v, t)?;
                        let r = self
                            .builder
                            .build_not(b, "not")
                            .map_err(|e| e.to_string())?;
                        Ok((r.into(), self.ctx.bool_type().into()))
                    }
                    UnOp::BitNot => {
                        let r = self
                            .builder
                            .build_not(v.into_int_value(), "bnot")
                            .map_err(|e| e.to_string())?;
                        Ok((r.into(), t))
                    }
                }
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                let (l, lt) = self.gen_expr(lhs)?;
                let (r, rt) = self.gen_expr(rhs)?;
                self.gen_binop(*op, l, lt, r, rt)
            }
            Expr::Call { callee, args, .. } => self.gen_call(callee, args),
            Expr::IfElse {
                cond, then, els, ..
            } => {
                let (cv, cty) = self.gen_expr(cond)?;
                let cond_i1 = self.to_bool(cv, cty)?;
                let (tv, tt) = self.gen_expr(then)?;
                let (ev, _) = self.gen_expr(els)?;
                let sel = self
                    .builder
                    .build_select(cond_i1, tv, ev, "sel")
                    .map_err(|e| e.to_string())?;
                Ok((sel, tt))
            }
            _ => Err("this expression is not supported by codegen yet (Phase 1 slice)".into()),
        }
    }

    fn gen_call(
        &mut self,
        callee: &Expr,
        args: &[Arg],
    ) -> Result<(BasicValueEnum<'ctx>, BasicTypeEnum<'ctx>), String> {
        if let Expr::Name(fname, _) = callee {
            if fname == "print" {
                return self.gen_print(args);
            }
            if fname == "range" {
                return Err("range() can only be used in a for-loop (Phase 1)".into());
            }
            if let Some(fv) = self.fns.get(fname).copied() {
                let mut argvals = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let (v, _) = self.gen_expr(&a.value)?;
                    let target = fv
                        .get_type()
                        .get_param_types()
                        .get(i)
                        .copied()
                        .ok_or_else(|| format!("too many arguments to '{fname}'"))?;
                    let v = self.coerce(v, target)?;
                    argvals.push(v.into());
                }
                let cs = self
                    .builder
                    .build_call(fv, &argvals, "call")
                    .map_err(|e| e.to_string())?;
                match cs.try_as_basic_value().left() {
                    Some(v) => Ok((v, v.get_type())),
                    None => {
                        // void call used in value position: produce a dummy.
                        let t = self.ctx.i32_type();
                        Ok((t.const_int(0, false).into(), t.into()))
                    }
                }
            } else {
                Err(format!("call to unknown function '{fname}'"))
            }
        } else {
            Err("only direct function calls are supported by codegen yet".into())
        }
    }

    fn gen_print(
        &mut self,
        args: &[Arg],
    ) -> Result<(BasicValueEnum<'ctx>, BasicTypeEnum<'ctx>), String> {
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                self.printf_lit(" ")?;
            }
            let (v, _) = self.gen_expr(&a.value)?;
            if v.is_float_value() {
                self.printf_val("%f", v)?;
            } else if v.is_int_value() {
                let iv = self.coerce(v, self.ctx.i64_type().into())?;
                self.printf_val("%lld", iv)?;
            } else {
                return Err("print supports numbers only in the Phase 1 slice".into());
            }
        }
        self.printf_lit("\n")?;
        let t = self.ctx.i32_type();
        Ok((t.const_int(0, false).into(), t.into()))
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

    #[allow(clippy::too_many_arguments)]
    fn gen_binop(
        &mut self,
        op: BinOp,
        l: BasicValueEnum<'ctx>,
        lt: BasicTypeEnum<'ctx>,
        r: BasicValueEnum<'ctx>,
        _rt: BasicTypeEnum<'ctx>,
    ) -> Result<(BasicValueEnum<'ctx>, BasicTypeEnum<'ctx>), String> {
        use BinOp::*;
        let is_float = l.is_float_value() || r.is_float_value();
        let b = self.builder;

        // Comparisons produce i1.
        if matches!(op, Eq | Ne | Lt | Le | Gt | Ge) {
            let bool_t = self.ctx.bool_type().into();
            if is_float {
                let (lf, rf) = (self.as_f64(l)?, self.as_f64(r)?);
                let pred = match op {
                    Eq => FloatPredicate::OEQ,
                    Ne => FloatPredicate::ONE,
                    Lt => FloatPredicate::OLT,
                    Le => FloatPredicate::OLE,
                    Gt => FloatPredicate::OGT,
                    Ge => FloatPredicate::OGE,
                    _ => unreachable!(),
                };
                let c = b
                    .build_float_compare(pred, lf, rf, "fcmp")
                    .map_err(|e| e.to_string())?;
                return Ok((c.into(), bool_t));
            } else {
                let pred = match op {
                    Eq => IntPredicate::EQ,
                    Ne => IntPredicate::NE,
                    Lt => IntPredicate::SLT,
                    Le => IntPredicate::SLE,
                    Gt => IntPredicate::SGT,
                    Ge => IntPredicate::SGE,
                    _ => unreachable!(),
                };
                let c = b
                    .build_int_compare(pred, l.into_int_value(), r.into_int_value(), "icmp")
                    .map_err(|e| e.to_string())?;
                return Ok((c.into(), bool_t));
            }
        }

        match op {
            Add | Sub | Mul | Mod | FloorDiv => {
                if is_float {
                    let (lf, rf) = (self.as_f64(l)?, self.as_f64(r)?);
                    let res = match op {
                        Add => b.build_float_add(lf, rf, "fadd"),
                        Sub => b.build_float_sub(lf, rf, "fsub"),
                        Mul => b.build_float_mul(lf, rf, "fmul"),
                        Mod => b.build_float_rem(lf, rf, "frem"),
                        FloorDiv => b.build_float_div(lf, rf, "fdiv"),
                        _ => unreachable!(),
                    }
                    .map_err(|e| e.to_string())?;
                    Ok((res.into(), self.ctx.f64_type().into()))
                } else {
                    let (li, ri) = (l.into_int_value(), r.into_int_value());
                    let res = match op {
                        Add => b.build_int_add(li, ri, "add"),
                        Sub => b.build_int_sub(li, ri, "sub"),
                        Mul => b.build_int_mul(li, ri, "mul"),
                        Mod => b.build_int_signed_rem(li, ri, "srem"),
                        FloorDiv => b.build_int_signed_div(li, ri, "sdiv"),
                        _ => unreachable!(),
                    }
                    .map_err(|e| e.to_string())?;
                    Ok((res.into(), lt))
                }
            }
            Div => {
                let (lf, rf) = (self.as_f64(l)?, self.as_f64(r)?);
                let res = b
                    .build_float_div(lf, rf, "fdiv")
                    .map_err(|e| e.to_string())?;
                Ok((res.into(), self.ctx.f64_type().into()))
            }
            BitAnd | BitOr | BitXor | Shl | Shr => {
                let (li, ri) = (l.into_int_value(), r.into_int_value());
                let res = match op {
                    BitAnd => b.build_and(li, ri, "and"),
                    BitOr => b.build_or(li, ri, "or"),
                    BitXor => b.build_xor(li, ri, "xor"),
                    Shl => b.build_left_shift(li, ri, "shl"),
                    Shr => b.build_right_shift(li, ri, true, "ashr"),
                    _ => unreachable!(),
                }
                .map_err(|e| e.to_string())?;
                Ok((res.into(), lt))
            }
            And | Or => {
                let lb = self.to_bool(l, lt)?;
                let rb = self.to_bool(r, _rt)?;
                let res = match op {
                    And => b.build_and(lb, rb, "land"),
                    Or => b.build_or(lb, rb, "lor"),
                    _ => unreachable!(),
                }
                .map_err(|e| e.to_string())?;
                Ok((res.into(), self.ctx.bool_type().into()))
            }
            Pow => Err("'**' is not supported by codegen yet".into()),
            In => Err("'in' is not supported by codegen yet".into()),
            Send => Err("channel send is not supported by codegen yet".into()),
            Eq | Ne | Lt | Le | Gt | Ge => unreachable!("handled above"),
        }
    }

    // --- coercions / helpers ---

    fn coerce(
        &self,
        val: BasicValueEnum<'ctx>,
        target: BasicTypeEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match (val, target) {
            (BasicValueEnum::IntValue(iv), BasicTypeEnum::IntType(tt)) => {
                let vw = iv.get_type().get_bit_width();
                let tw = tt.get_bit_width();
                if vw == tw {
                    Ok(iv.into())
                } else if vw < tw {
                    Ok(self
                        .builder
                        .build_int_s_extend(iv, tt, "sext")
                        .map_err(|e| e.to_string())?
                        .into())
                } else {
                    Ok(self
                        .builder
                        .build_int_truncate(iv, tt, "trunc")
                        .map_err(|e| e.to_string())?
                        .into())
                }
            }
            (BasicValueEnum::IntValue(iv), BasicTypeEnum::FloatType(tt)) => Ok(self
                .builder
                .build_signed_int_to_float(iv, tt, "sitofp")
                .map_err(|e| e.to_string())?
                .into()),
            (BasicValueEnum::FloatValue(fv), BasicTypeEnum::FloatType(tt)) => {
                if fv.get_type() == tt {
                    Ok(fv.into())
                } else {
                    Ok(self
                        .builder
                        .build_float_cast(fv, tt, "fcast")
                        .map_err(|e| e.to_string())?
                        .into())
                }
            }
            _ => Ok(val),
        }
    }

    fn as_f64(&self, v: BasicValueEnum<'ctx>) -> Result<inkwell::values::FloatValue<'ctx>, String> {
        if v.is_float_value() {
            Ok(v.into_float_value())
        } else {
            Ok(self
                .builder
                .build_signed_int_to_float(v.into_int_value(), self.ctx.f64_type(), "sitofp")
                .map_err(|e| e.to_string())?)
        }
    }

    fn to_bool(
        &self,
        v: BasicValueEnum<'ctx>,
        _t: BasicTypeEnum<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        if v.is_int_value() {
            let iv = v.into_int_value();
            if iv.get_type().get_bit_width() == 1 {
                Ok(iv)
            } else {
                let zero = iv.get_type().const_int(0, false);
                Ok(self
                    .builder
                    .build_int_compare(IntPredicate::NE, iv, zero, "tobool")
                    .map_err(|e| e.to_string())?)
            }
        } else if v.is_float_value() {
            let fv = v.into_float_value();
            let zero = fv.get_type().const_float(0.0);
            Ok(self
                .builder
                .build_float_compare(FloatPredicate::ONE, fv, zero, "tobool")
                .map_err(|e| e.to_string())?)
        } else {
            Err("value cannot be used as a condition".into())
        }
    }
}

fn is_name(e: &Expr, name: &str) -> bool {
    matches!(e, Expr::Name(n, _) if n == name)
}

fn parse_int_lit(s: &str) -> i64 {
    let clean: String = s.chars().filter(|c| *c != '_').collect();
    let parse_radix = |body: &str, radix: u32| i64::from_str_radix(body, radix).unwrap_or(0);
    if let Some(h) = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
    {
        parse_radix(h, 16)
    } else if let Some(o) = clean
        .strip_prefix("0o")
        .or_else(|| clean.strip_prefix("0O"))
    {
        parse_radix(o, 8)
    } else if let Some(bn) = clean
        .strip_prefix("0b")
        .or_else(|| clean.strip_prefix("0B"))
    {
        parse_radix(bn, 2)
    } else {
        clean.parse::<i64>().unwrap_or(0)
    }
}
