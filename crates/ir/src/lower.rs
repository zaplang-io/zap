//! AST -> ZIR lowering.
//!
//! Mirrors the vertical slice the backend supports: `int64`/`float64`/`bool`,
//! functions (incl. recursion), `let`/assign/aug-assign, `if`/`while`/
//! `do-while`/`for`-over-`range`, `return`, calls, arithmetic/comparison, and
//! `print`. Type-directed decisions (int vs float ops, widening casts,
//! truthiness) happen here, so the ZIR -> LLVM step is mechanical. Unsupported
//! constructs produce a [`LowerError`].

use std::collections::{HashMap, HashSet};

use zapc_syntax::ast::{self, BinOp as AstBin, Expr, Stmt, UnOp as AstUn};

use crate::ir::*;

#[derive(Clone, Debug, PartialEq)]
pub struct LowerError {
    pub message: String,
}

/// Lower a type-checked module to ZIR.
pub fn lower_module(module: &ast::Module) -> (Module, Vec<LowerError>) {
    let mut sigs: HashMap<String, (Vec<Ty>, Ty)> = HashMap::new();
    let mut errors = Vec::new();

    for item in &module.items {
        if let ast::Item::Function(f) = item {
            let params: Vec<Ty> = f
                .params
                .iter()
                .map(|p| {
                    p.ty.as_ref()
                        .map(ast_ty)
                        .unwrap_or(Ok(Ty::Ptr))
                        .unwrap_or(Ty::Ptr)
                })
                .collect();
            let ret = match &f.ret {
                Some(t) => ast_ty(t).unwrap_or(Ty::Void),
                None => Ty::Void,
            };
            sigs.insert(f.name.clone(), (params, ret));
        }
    }

    let mut out = Module::default();
    for item in &module.items {
        if let ast::Item::Function(f) = item {
            match lower_fn(f, &sigs) {
                Ok(func) => out.funcs.push(func),
                Err(e) => errors.push(e),
            }
        }
    }
    (out, errors)
}

fn ast_ty(t: &ast::Type) -> Result<Ty, LowerError> {
    match t {
        ast::Type::Named { name, .. } => match name.as_str() {
            "int64" | "i64" => Ok(Ty::I64),
            "int32" | "i32" => Ok(Ty::I32),
            "float64" | "f64" => Ok(Ty::F64),
            "float32" | "f32" => Ok(Ty::F32),
            "bool" => Ok(Ty::I1),
            "char" => Ok(Ty::I32),
            other => Err(err(format!(
                "type '{other}' is not supported by codegen yet"
            ))),
        },
        _ => Err(err("only named scalar types are supported by codegen yet")),
    }
}

fn err(m: impl Into<String>) -> LowerError {
    LowerError { message: m.into() }
}

fn lower_fn(f: &ast::Function, sigs: &HashMap<String, (Vec<Ty>, Ty)>) -> Result<Func, LowerError> {
    let (params, ret) = sigs.get(&f.name).cloned().unwrap();
    let is_main = f.name == "main";
    let mut b = FnLower::new(f.name.clone(), params.clone(), ret, is_main, sigs);

    // Parameters: values 0..n are the incoming args; copy each into a slot.
    for (i, p) in f.params.iter().enumerate() {
        let pty = params[i];
        let slot = b.alloca(pty);
        b.store(slot, i as Value);
        b.declare(&p.name, slot, pty);
    }

    for st in &f.body.stmts {
        b.stmt(st)?;
    }

    if !b.terminated() {
        if is_main || ret == Ty::Void {
            b.set_term(Term::Ret(None));
        } else {
            b.set_term(Term::Unreachable);
        }
    }
    Ok(b.func)
}

struct FnLower<'a> {
    func: Func,
    cur: BlockId,
    scopes: Vec<HashMap<String, (Value, Ty)>>,
    loops: Vec<(BlockId, BlockId)>, // (continue, break)
    term_set: HashSet<BlockId>,     // blocks with a real terminator
    sigs: &'a HashMap<String, (Vec<Ty>, Ty)>,
}

impl<'a> FnLower<'a> {
    fn new(
        name: String,
        params: Vec<Ty>,
        ret: Ty,
        is_main: bool,
        sigs: &'a HashMap<String, (Vec<Ty>, Ty)>,
    ) -> FnLower<'a> {
        let value_types = params.clone();
        let func = Func {
            name,
            params,
            ret,
            is_main,
            value_types,
            blocks: vec![Block {
                id: 0,
                insts: Vec::new(),
                term: Term::Unreachable,
            }],
        };
        FnLower {
            func,
            cur: 0,
            scopes: vec![HashMap::new()],
            loops: Vec::new(),
            term_set: HashSet::new(),
            sigs,
        }
    }

    // --- builder primitives ---

    fn new_value(&mut self, ty: Ty) -> Value {
        let v = self.func.value_types.len() as Value;
        self.func.value_types.push(ty);
        v
    }

    fn new_block(&mut self) -> BlockId {
        let id = self.func.blocks.len() as BlockId;
        self.func.blocks.push(Block {
            id,
            insts: Vec::new(),
            term: Term::Unreachable,
        });
        id
    }

    fn position(&mut self, b: BlockId) {
        self.cur = b;
    }

    fn emit(&mut self, inst: Inst) {
        self.func.blocks[self.cur as usize].insts.push(inst);
    }

    fn set_term(&mut self, t: Term) {
        self.func.blocks[self.cur as usize].term = t;
        self.term_set.insert(self.cur);
    }

    fn terminated(&self) -> bool {
        self.term_set.contains(&self.cur)
    }

    fn ensure_br(&mut self, dest: BlockId) {
        if !self.terminated() {
            self.set_term(Term::Br(dest));
        }
    }

    // scopes
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
    fn declare(&mut self, name: &str, ptr: Value, ty: Ty) {
        self.scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), (ptr, ty));
    }
    fn lookup(&self, name: &str) -> Option<(Value, Ty)> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    // instruction helpers
    fn alloca(&mut self, pointee: Ty) -> Value {
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::Alloca { dest: d, pointee });
        d
    }
    fn store(&mut self, ptr: Value, val: Value) {
        self.emit(Inst::Store { ptr, val });
    }
    fn load(&mut self, ptr: Value, ty: Ty) -> Value {
        let d = self.new_value(ty);
        self.emit(Inst::Load { dest: d, ptr });
        d
    }
    fn iconst(&mut self, ty: Ty, val: i64) -> Value {
        let d = self.new_value(ty);
        self.emit(Inst::IConst { dest: d, val });
        d
    }
    fn fconst(&mut self, ty: Ty, val: f64) -> Value {
        let d = self.new_value(ty);
        self.emit(Inst::FConst { dest: d, val });
        d
    }

    // --- statements ---

    fn stmt(&mut self, st: &Stmt) -> Result<(), LowerError> {
        match st {
            Stmt::Let {
                ty, name, value, ..
            } => {
                let (v, vt) = self.expr(value)?;
                let declared = match ty {
                    Some(t) => ast_ty(t)?,
                    None => vt,
                };
                let v = self.coerce(v, vt, declared);
                let slot = self.alloca(declared);
                self.store(slot, v);
                self.declare(name, slot, declared);
                Ok(())
            }
            Stmt::Assign { target, value, .. } => {
                let (v, vt) = self.expr(value)?;
                if let Expr::Name(n, _) = target {
                    let (ptr, ty) = self
                        .lookup(n)
                        .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
                    let v = self.coerce(v, vt, ty);
                    self.store(ptr, v);
                    Ok(())
                } else {
                    Err(err(
                        "only simple variable assignment is supported by codegen yet",
                    ))
                }
            }
            Stmt::AugAssign {
                target, op, value, ..
            } => {
                if let Expr::Name(n, _) = target {
                    let (ptr, ty) = self
                        .lookup(n)
                        .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
                    let cur = self.load(ptr, ty);
                    let (rhs, rt) = self.expr(value)?;
                    let (res, rty) = self.binop(*op, cur, ty, rhs, rt)?;
                    let res = self.coerce(res, rty, ty);
                    self.store(ptr, res);
                    Ok(())
                } else {
                    Err(err(
                        "only simple variable aug-assignment is supported by codegen yet",
                    ))
                }
            }
            Stmt::Expr(e) => {
                self.expr(e)?;
                Ok(())
            }
            Stmt::If {
                branches,
                else_block,
                ..
            } => self.lower_if(branches, else_block.as_ref()),
            Stmt::While { cond, body, .. } => self.lower_while(cond, body),
            Stmt::DoWhile { body, cond, .. } => self.lower_do_while(body, cond),
            Stmt::For {
                target, iter, body, ..
            } => self.lower_for(target, iter, body),
            Stmt::Return { value, .. } => {
                match value {
                    Some(e) => {
                        let (v, vt) = self.expr(e)?;
                        let ret = self.func.ret;
                        let v = if ret == Ty::Void {
                            v
                        } else {
                            self.coerce(v, vt, ret)
                        };
                        self.set_term(Term::Ret(Some(v)));
                    }
                    None => self.set_term(Term::Ret(None)),
                }
                Ok(())
            }
            Stmt::Break(_) => {
                let (_, brk) = *self
                    .loops
                    .last()
                    .ok_or_else(|| err("'break' outside a loop"))?;
                self.set_term(Term::Br(brk));
                Ok(())
            }
            Stmt::Continue(_) => {
                let (cont, _) = *self
                    .loops
                    .last()
                    .ok_or_else(|| err("'continue' outside a loop"))?;
                self.set_term(Term::Br(cont));
                Ok(())
            }
            Stmt::Raise { .. }
            | Stmt::Try { .. }
            | Stmt::With { .. }
            | Stmt::Match { .. }
            | Stmt::Spawn { .. } => Err(err(
                "this statement is not supported by codegen yet (Phase 1 slice)",
            )),
        }
    }

    fn lower_block(&mut self, b: &ast::Block) -> Result<(), LowerError> {
        self.push_scope();
        for st in &b.stmts {
            self.stmt(st)?;
        }
        self.pop_scope();
        Ok(())
    }

    fn lower_if(
        &mut self,
        branches: &[(Expr, ast::Block)],
        else_block: Option<&ast::Block>,
    ) -> Result<(), LowerError> {
        let merge = self.new_block();
        for (cond, body) in branches {
            let (cv, cty) = self.expr(cond)?;
            let c = self.truthy(cv, cty);
            let then_bb = self.new_block();
            let next_bb = self.new_block();
            self.set_term(Term::BrIf {
                cond: c,
                then_bb,
                else_bb: next_bb,
            });
            self.position(then_bb);
            self.lower_block(body)?;
            self.ensure_br(merge);
            self.position(next_bb);
        }
        if let Some(eb) = else_block {
            self.lower_block(eb)?;
        }
        self.ensure_br(merge);
        self.position(merge);
        Ok(())
    }

    fn lower_while(&mut self, cond: &Expr, body: &ast::Block) -> Result<(), LowerError> {
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let (cv, cty) = self.expr(cond)?;
        let c = self.truthy(cv, cty);
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(body_bb);
        self.loops.push((cond_bb, end_bb));
        self.lower_block(body)?;
        self.loops.pop();
        self.ensure_br(cond_bb);
        self.position(end_bb);
        Ok(())
    }

    fn lower_do_while(&mut self, body: &ast::Block, cond: &Expr) -> Result<(), LowerError> {
        let body_bb = self.new_block();
        let cond_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(body_bb));
        self.position(body_bb);
        self.loops.push((cond_bb, end_bb));
        self.lower_block(body)?;
        self.loops.pop();
        self.ensure_br(cond_bb);
        self.position(cond_bb);
        let (cv, cty) = self.expr(cond)?;
        let c = self.truthy(cv, cty);
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(end_bb);
        Ok(())
    }

    fn lower_for(
        &mut self,
        target: &Expr,
        iter: &Expr,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let (start, stop, step) = match iter {
            Expr::Call { callee, args, .. } if is_name(callee, "range") => {
                let mut vs = Vec::new();
                for a in args {
                    let (v, t) = self.expr(&a.value)?;
                    vs.push(self.coerce(v, t, Ty::I64));
                }
                match vs.len() {
                    1 => (self.iconst(Ty::I64, 0), vs[0], self.iconst(Ty::I64, 1)),
                    2 => (vs[0], vs[1], self.iconst(Ty::I64, 1)),
                    3 => (vs[0], vs[1], vs[2]),
                    n => return Err(err(format!("range() takes 1-3 arguments, got {n}"))),
                }
            }
            _ => {
                return Err(err(
                    "only `for ... in range(...)` is supported by codegen yet",
                ))
            }
        };
        let name = match target {
            Expr::Name(n, _) => n.clone(),
            _ => {
                return Err(err(
                    "only a simple loop variable is supported by codegen yet",
                ))
            }
        };

        self.push_scope();
        let var = self.alloca(Ty::I64);
        self.store(var, start);
        self.declare(&name, var, Ty::I64);

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(var, Ty::I64);
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: stop,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        self.loops.push((incr_bb, end_bb));
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.loops.pop();
        self.ensure_br(incr_bb);

        self.position(incr_bb);
        let cur2 = self.load(var, Ty::I64);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: step,
        });
        self.store(var, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        self.pop_scope();
        Ok(())
    }

    // --- expressions ---

    fn expr(&mut self, e: &Expr) -> Result<(Value, Ty), LowerError> {
        match e {
            Expr::Int(s, _) => {
                let v = parse_int_lit(s);
                Ok((self.iconst(Ty::I64, v), Ty::I64))
            }
            Expr::Float(s, _) => {
                let v: f64 = s.replace('_', "").parse().unwrap_or(0.0);
                Ok((self.fconst(Ty::F64, v), Ty::F64))
            }
            Expr::Bool(b, _) => {
                let d = self.new_value(Ty::I1);
                self.emit(Inst::BConst { dest: d, val: *b });
                Ok((d, Ty::I1))
            }
            Expr::Name(n, _) => {
                let (ptr, ty) = self
                    .lookup(n)
                    .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
                Ok((self.load(ptr, ty), ty))
            }
            Expr::Unary { op, expr, .. } => {
                let (v, t) = self.expr(expr)?;
                match op {
                    AstUn::Neg => {
                        let d = self.new_value(t);
                        self.emit(Inst::Un {
                            dest: d,
                            op: UnOp::Neg,
                            val: v,
                        });
                        Ok((d, t))
                    }
                    AstUn::Pos => Ok((v, t)),
                    AstUn::Not => {
                        let b = self.truthy(v, t);
                        let d = self.new_value(Ty::I1);
                        self.emit(Inst::Un {
                            dest: d,
                            op: UnOp::Not,
                            val: b,
                        });
                        Ok((d, Ty::I1))
                    }
                    AstUn::BitNot => {
                        let d = self.new_value(t);
                        self.emit(Inst::Un {
                            dest: d,
                            op: UnOp::BitNot,
                            val: v,
                        });
                        Ok((d, t))
                    }
                }
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                let (l, lt) = self.expr(lhs)?;
                let (r, rt) = self.expr(rhs)?;
                self.binop(*op, l, lt, r, rt)
            }
            Expr::Call { callee, args, .. } => self.call(callee, args),
            Expr::IfElse {
                cond, then, els, ..
            } => {
                let (cv, cty) = self.expr(cond)?;
                let c = self.truthy(cv, cty);
                let (a, at) = self.expr(then)?;
                let (b, _bt) = self.expr(els)?;
                let d = self.new_value(at);
                self.emit(Inst::Select {
                    dest: d,
                    cond: c,
                    a,
                    b,
                });
                Ok((d, at))
            }
            _ => Err(err(
                "this expression is not supported by codegen yet (Phase 1 slice)",
            )),
        }
    }

    fn call(&mut self, callee: &Expr, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
        if let Expr::Name(fname, _) = callee {
            if fname == "print" {
                let mut vals = Vec::new();
                for a in args {
                    let (v, _) = self.expr(&a.value)?;
                    vals.push(v);
                }
                self.emit(Inst::Print { args: vals });
                return Ok((self.iconst(Ty::I32, 0), Ty::I32));
            }
            if fname == "range" {
                return Err(err("range() can only be used in a for-loop (Phase 1)"));
            }
            if let Some((ptys, ret)) = self.sigs.get(fname).cloned() {
                let mut argvals = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let (v, vt) = self.expr(&a.value)?;
                    let target = *ptys.get(i).unwrap_or(&vt);
                    argvals.push(self.coerce(v, vt, target));
                }
                let dest = if ret == Ty::Void {
                    None
                } else {
                    Some(self.new_value(ret))
                };
                self.emit(Inst::Call {
                    dest,
                    func: fname.clone(),
                    args: argvals,
                });
                let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
                return Ok((rv, if ret == Ty::Void { Ty::I32 } else { ret }));
            }
            Err(err(format!("call to unknown function '{fname}'")))
        } else {
            Err(err(
                "only direct function calls are supported by codegen yet",
            ))
        }
    }

    fn binop(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let is_float = lt.is_float() || rt.is_float();

        if matches!(op, Eq | Ne | Lt | Le | Gt | Ge) {
            let pred = match op {
                Eq => CmpPred::Eq,
                Ne => CmpPred::Ne,
                Lt => CmpPred::Lt,
                Le => CmpPred::Le,
                Gt => CmpPred::Gt,
                Ge => CmpPred::Ge,
                _ => unreachable!(),
            };
            let d = self.new_value(Ty::I1);
            if is_float {
                let lf = self.as_f64(l, lt);
                let rf = self.as_f64(r, rt);
                self.emit(Inst::FCmp {
                    dest: d,
                    pred,
                    lhs: lf,
                    rhs: rf,
                });
            } else {
                self.emit(Inst::ICmp {
                    dest: d,
                    pred,
                    lhs: l,
                    rhs: r,
                });
            }
            return Ok((d, Ty::I1));
        }

        match op {
            Add | Sub | Mul | Mod | FloorDiv => {
                let irop = match op {
                    Add => BinOp::Add,
                    Sub => BinOp::Sub,
                    Mul => BinOp::Mul,
                    Mod => BinOp::Rem,
                    FloorDiv => BinOp::Div,
                    _ => unreachable!(),
                };
                if is_float {
                    let lf = self.as_f64(l, lt);
                    let rf = self.as_f64(r, rt);
                    let d = self.new_value(Ty::F64);
                    self.emit(Inst::Bin {
                        dest: d,
                        op: irop,
                        lhs: lf,
                        rhs: rf,
                    });
                    Ok((d, Ty::F64))
                } else {
                    let d = self.new_value(lt);
                    self.emit(Inst::Bin {
                        dest: d,
                        op: irop,
                        lhs: l,
                        rhs: r,
                    });
                    Ok((d, lt))
                }
            }
            Div => {
                let lf = self.as_f64(l, lt);
                let rf = self.as_f64(r, rt);
                let d = self.new_value(Ty::F64);
                self.emit(Inst::Bin {
                    dest: d,
                    op: BinOp::Div,
                    lhs: lf,
                    rhs: rf,
                });
                Ok((d, Ty::F64))
            }
            BitAnd | BitOr | BitXor | Shl | Shr => {
                let irop = match op {
                    BitAnd => BinOp::And,
                    BitOr => BinOp::Or,
                    BitXor => BinOp::Xor,
                    Shl => BinOp::Shl,
                    Shr => BinOp::Shr,
                    _ => unreachable!(),
                };
                let d = self.new_value(lt);
                self.emit(Inst::Bin {
                    dest: d,
                    op: irop,
                    lhs: l,
                    rhs: r,
                });
                Ok((d, lt))
            }
            And | Or => {
                let lb = self.truthy(l, lt);
                let rb = self.truthy(r, rt);
                let irop = if matches!(op, And) {
                    BinOp::And
                } else {
                    BinOp::Or
                };
                let d = self.new_value(Ty::I1);
                self.emit(Inst::Bin {
                    dest: d,
                    op: irop,
                    lhs: lb,
                    rhs: rb,
                });
                Ok((d, Ty::I1))
            }
            Pow => Err(err("'**' is not supported by codegen yet")),
            In => Err(err("'in' is not supported by codegen yet")),
            Send => Err(err("channel send is not supported by codegen yet")),
            Eq | Ne | Lt | Le | Gt | Ge => unreachable!(),
        }
    }

    // --- coercions / helpers ---

    fn coerce(&mut self, v: Value, from: Ty, to: Ty) -> Value {
        if from == to {
            return v;
        }
        if let (Some(fw), Some(tw)) = (from.int_width(), to.int_width()) {
            let d = self.new_value(to);
            let kind = if tw > fw {
                CastKind::SExt
            } else {
                CastKind::Trunc
            };
            self.emit(Inst::Cast {
                dest: d,
                kind,
                val: v,
            });
            return d;
        }
        if from.int_width().is_some() && to.is_float() {
            let d = self.new_value(to);
            self.emit(Inst::Cast {
                dest: d,
                kind: CastKind::SIToFP,
                val: v,
            });
            return d;
        }
        if from.is_float() && to.is_float() {
            let d = self.new_value(to);
            let kind = if to == Ty::F64 {
                CastKind::FPExt
            } else {
                CastKind::FPTrunc
            };
            self.emit(Inst::Cast {
                dest: d,
                kind,
                val: v,
            });
            return d;
        }
        v
    }

    fn as_f64(&mut self, v: Value, t: Ty) -> Value {
        if t.is_float() {
            v
        } else {
            let d = self.new_value(Ty::F64);
            self.emit(Inst::Cast {
                dest: d,
                kind: CastKind::SIToFP,
                val: v,
            });
            d
        }
    }

    fn truthy(&mut self, v: Value, t: Ty) -> Value {
        if t == Ty::I1 {
            return v;
        }
        if t.is_float() {
            let z = self.fconst(t, 0.0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::FCmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: v,
                rhs: z,
            });
            d
        } else {
            let z = self.iconst(t, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: v,
                rhs: z,
            });
            d
        }
    }
}

fn is_name(e: &Expr, name: &str) -> bool {
    matches!(e, Expr::Name(n, _) if n == name)
}

fn parse_int_lit(s: &str) -> i64 {
    let clean: String = s.chars().filter(|c| *c != '_').collect();
    let r = |b: &str, radix: u32| i64::from_str_radix(b, radix).unwrap_or(0);
    if let Some(h) = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
    {
        r(h, 16)
    } else if let Some(o) = clean
        .strip_prefix("0o")
        .or_else(|| clean.strip_prefix("0O"))
    {
        r(o, 8)
    } else if let Some(bn) = clean
        .strip_prefix("0b")
        .or_else(|| clean.strip_prefix("0B"))
    {
        r(bn, 2)
    } else {
        clean.parse::<i64>().unwrap_or(0)
    }
}
