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
/// Struct layouts/ids collected before lowering function bodies.
struct StructCtx {
    ids: HashMap<String, u32>,
    names: Vec<String>,
    fields: Vec<Vec<(String, Ty)>>,
}

impl StructCtx {
    /// Index and type of `name` in struct `sid`.
    fn field(&self, sid: u32, name: &str) -> Option<(u32, Ty)> {
        self.fields[sid as usize]
            .iter()
            .enumerate()
            .find(|(_, (n, _))| n == name)
            .map(|(i, (_, t))| (i as u32, *t))
    }
}

pub fn lower_module(module: &ast::Module) -> (Module, Vec<LowerError>) {
    let mut errors = Vec::new();

    // Collect struct ids, then resolve field types (two passes so fields may
    // reference structs declared later).
    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut names: Vec<String> = Vec::new();
    for item in &module.items {
        if let ast::Item::Struct(s) = item {
            ids.insert(s.name.clone(), names.len() as u32);
            names.push(s.name.clone());
        }
    }
    let mut fields: Vec<Vec<(String, Ty)>> = vec![Vec::new(); names.len()];
    for item in &module.items {
        if let ast::Item::Struct(s) = item {
            let sid = ids[&s.name];
            for fld in &s.fields {
                let ft = resolve_ty(&fld.ty, &ids).unwrap_or(Ty::Ptr);
                fields[sid as usize].push((fld.name.clone(), ft));
            }
        }
    }
    let ctx = StructCtx { ids, names, fields };

    // Signatures: top-level functions and struct methods (mangled `S__m`).
    let mut sigs: HashMap<String, (Vec<Ty>, Ty)> = HashMap::new();
    for item in &module.items {
        match item {
            ast::Item::Function(f) => {
                let params = f.params.iter().map(|p| param_ty(p, None, &ctx)).collect();
                let ret = f
                    .ret
                    .as_ref()
                    .map(|t| resolve_ty(t, &ctx.ids).unwrap_or(Ty::Void))
                    .unwrap_or(Ty::Void);
                sigs.insert(f.name.clone(), (params, ret));
            }
            ast::Item::Struct(s) => {
                let sid = ctx.ids[&s.name];
                for m in &s.methods {
                    let params = m
                        .params
                        .iter()
                        .map(|p| param_ty(p, Some(sid), &ctx))
                        .collect();
                    let ret = m
                        .ret
                        .as_ref()
                        .map(|t| resolve_ty(t, &ctx.ids).unwrap_or(Ty::Void))
                        .unwrap_or(Ty::Void);
                    sigs.insert(format!("{}__{}", s.name, m.name), (params, ret));
                }
            }
            _ => {}
        }
    }

    let mut out = Module::default();
    for sid in 0..ctx.names.len() {
        out.structs.push(StructLayout {
            name: ctx.names[sid].clone(),
            fields: ctx.fields[sid].iter().map(|(_, t)| *t).collect(),
        });
    }
    for item in &module.items {
        match item {
            ast::Item::Function(f) => match lower_fn(f, f.name.clone(), None, &sigs, &ctx) {
                Ok(func) => out.funcs.push(func),
                Err(e) => errors.push(e),
            },
            ast::Item::Struct(s) => {
                let sid = ctx.ids[&s.name];
                for m in &s.methods {
                    let name = format!("{}__{}", s.name, m.name);
                    match lower_fn(m, name, Some(sid), &sigs, &ctx) {
                        Ok(func) => out.funcs.push(func),
                        Err(e) => errors.push(e),
                    }
                }
            }
            _ => {}
        }
    }
    (out, errors)
}

fn resolve_ty(t: &ast::Type, ids: &HashMap<String, u32>) -> Result<Ty, LowerError> {
    match t {
        ast::Type::Named { name, args, .. } => match name.as_str() {
            "int64" | "i64" => Ok(Ty::I64),
            "int32" | "i32" => Ok(Ty::I32),
            "float64" | "f64" => Ok(Ty::F64),
            "float32" | "f32" => Ok(Ty::F32),
            "bool" => Ok(Ty::I1),
            "char" => Ok(Ty::I32),
            "str" | "bytes" => Ok(Ty::Ptr),
            "list" => {
                let e = match args.first() {
                    Some(a) => elem_of(a, ids)?,
                    None => Elem::I64,
                };
                Ok(Ty::List(e))
            }
            "array" => {
                let d = match args.first() {
                    Some(a) => dtype_of(a, ids)?,
                    None => Dtype::F64,
                };
                Ok(Ty::Array(d))
            }
            "dict" => {
                let k = match args.first() {
                    Some(a) => elem_of(a, ids)?,
                    None => Elem::I64,
                };
                let v = match args.get(1) {
                    Some(a) => elem_of(a, ids)?,
                    None => Elem::I64,
                };
                Ok(Ty::Dict(k, v))
            }
            "set" => {
                let e = match args.first() {
                    Some(a) => elem_of(a, ids)?,
                    None => Elem::I64,
                };
                Ok(Ty::Set(e))
            }
            other => match ids.get(other) {
                Some(id) => Ok(Ty::Struct(*id)),
                None => Err(err(format!(
                    "type '{other}' is not supported by codegen yet"
                ))),
            },
        },
        _ => Err(err("only named scalar types are supported by codegen yet")),
    }
}

/// The runtime representation of a collection element/key/value given its
/// source type.
fn elem_of(t: &ast::Type, ids: &HashMap<String, u32>) -> Result<Elem, LowerError> {
    elem_kind(resolve_ty(t, ids)?)
}

/// The array dtype for a `array[T]` element type (ticket #24). Only `f64` and
/// `i64` are supported in this increment; other numeric dtypes (`f32`, `i32`)
/// and dtype promotion are deferred.
fn dtype_of(t: &ast::Type, ids: &HashMap<String, u32>) -> Result<Dtype, LowerError> {
    match resolve_ty(t, ids)? {
        Ty::F64 => Ok(Dtype::F64),
        Ty::I64 => Ok(Dtype::I64),
        other => Err(err(format!(
            "array[{other}] is not supported yet (only array[f64] and array[i64])"
        ))),
    }
}

/// The array dtype named by a construction subscript like `array[f64]`, where
/// the dtype appears as a bare name in expression position.
fn dtype_from_name(name: &str) -> Result<Dtype, LowerError> {
    match name {
        "float64" | "f64" => Ok(Dtype::F64),
        "int64" | "i64" => Ok(Dtype::I64),
        other => Err(err(format!(
            "array[{other}] is not supported yet (only array[f64] and array[i64])"
        ))),
    }
}

/// Classify a ZIR type into the element word kind collections store. Elements
/// are uniform 8-byte words (RUNTIME-ABI §"Collection layout"): integers/bools
/// go in as `i64`, reference types as pointers. Floats need a separate word
/// path and are deferred.
fn elem_kind(t: Ty) -> Result<Elem, LowerError> {
    match t {
        Ty::I1 | Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 => Ok(Elem::I64),
        Ty::Ptr | Ty::Struct(_) | Ty::List(_) | Ty::Dict(..) | Ty::Set(_) | Ty::Array(_) => {
            Ok(Elem::Ptr)
        }
        Ty::F32 | Ty::F64 => Err(err(
            "collections of floating-point elements are not supported yet",
        )),
        Ty::Void => Err(err("a collection element cannot be void")),
    }
}

fn param_ty(p: &ast::Param, self_sid: Option<u32>, ctx: &StructCtx) -> Ty {
    if p.name == "self" {
        if let Some(sid) = self_sid {
            return Ty::Struct(sid);
        }
    }
    p.ty.as_ref()
        .map(|t| resolve_ty(t, &ctx.ids).unwrap_or(Ty::Ptr))
        .unwrap_or(Ty::Ptr)
}

fn err(m: impl Into<String>) -> LowerError {
    LowerError { message: m.into() }
}

fn lower_fn(
    f: &ast::Function,
    zir_name: String,
    self_sid: Option<u32>,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let (params, ret) = sigs.get(&zir_name).cloned().unwrap();
    let is_main = zir_name == "main";
    let _ = self_sid;
    let mut b = FnLower::new(zir_name, params.clone(), ret, is_main, sigs, ctx);

    // Parameters: values 0..n are the incoming args; copy each into a slot.
    for (i, p) in f.params.iter().enumerate() {
        let pty = params[i];
        let slot = b.alloca(pty);
        b.store(slot, i as Value);
        b.declare(&p.name, slot, pty);
    }

    if is_main {
        // `main` installs an implicit top-level landing pad so an exception that
        // propagates out of the program is caught and reported (message + exit 1)
        // rather than ending in a raw abort (RUNTIME-ABI §"Exception / unwinding
        // ABI": "unwinds to the top frame and aborts"). Every call in `main`
        // therefore `invoke`s to this pad.
        let lp = b.new_block();
        b.landing_pads.push(lp);
        for st in &f.body.stmts {
            b.stmt(st)?;
        }
        b.landing_pads.pop();
        if !b.terminated() {
            b.set_term(Term::Ret(None));
        }
        if b.used_lps.contains(&lp) {
            b.position(lp);
            let exc = b.new_value(Ty::Ptr);
            b.emit(Inst::LandingPad { dest: exc });
            b.emit(Inst::Call {
                dest: None,
                func: "zap_uncaught".to_string(),
                args: vec![exc],
            });
            b.set_term(Term::Unreachable);
        }
    } else {
        for st in &f.body.stmts {
            b.stmt(st)?;
        }
        if !b.terminated() {
            if ret == Ty::Void {
                b.set_term(Term::Ret(None));
            } else {
                b.set_term(Term::Unreachable);
            }
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
    /// Stack of active landing-pad blocks (innermost `try` last). A call emitted
    /// while this is non-empty becomes an [`Inst::Invoke`] whose unwind edge goes
    /// to the top entry, so a raised exception transfers to the nearest handler.
    landing_pads: Vec<BlockId>,
    /// Landing-pad blocks actually targeted by an [`Inst::Invoke`]. A `try` whose
    /// body emitted no throwing call cannot raise, so its handler (and the
    /// `landingpad` that LLVM requires an `invoke` predecessor for) is omitted.
    used_lps: HashSet<BlockId>,
    sigs: &'a HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &'a StructCtx,
}

impl<'a> FnLower<'a> {
    fn new(
        name: String,
        params: Vec<Ty>,
        ret: Ty,
        is_main: bool,
        sigs: &'a HashMap<String, (Vec<Ty>, Ty)>,
        ctx: &'a StructCtx,
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
            landing_pads: Vec::new(),
            used_lps: HashSet::new(),
            sigs,
            ctx,
        }
    }

    /// Emit a call that may raise. Inside a `try` (a landing pad is active) this
    /// becomes an [`Inst::Invoke`] whose unwind edge targets the nearest handler;
    /// otherwise a plain [`Inst::Call`] (the exception, if any, unwinds through
    /// this frame to a caller's handler or to the top-level). The caller sets the
    /// terminator (e.g. `Unreachable` after `zap_panic`).
    fn throwing_call(&mut self, dest: Option<Value>, func: &str, args: Vec<Value>) {
        if let Some(&lp) = self.landing_pads.last() {
            self.used_lps.insert(lp);
            self.emit(Inst::Invoke {
                dest,
                func: func.to_string(),
                args,
                unwind: lp,
            });
        } else {
            self.emit(Inst::Call {
                dest,
                func: func.to_string(),
                args,
            });
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
                    Some(t) => resolve_ty(t, &self.ctx.ids)?,
                    None => vt,
                };
                let v = self.coerce(v, vt, declared);
                let slot = self.alloca(declared);
                self.store(slot, v);
                self.declare(name, slot, declared);
                Ok(())
            }
            Stmt::Assign { target, value, .. } => {
                if let Expr::Name(n, _) = target {
                    let (v, vt) = self.expr(value)?;
                    let (ptr, ty) = self
                        .lookup(n)
                        .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
                    let v = self.coerce(v, vt, ty);
                    self.store(ptr, v);
                    Ok(())
                } else if let Expr::Attr { base, name, .. } = target {
                    let (bv, bty) = self.expr(base)?;
                    let Ty::Struct(sid) = bty else {
                        return Err(err("field assignment on a non-struct value"));
                    };
                    let (idx, fty) = self
                        .ctx
                        .field(sid, name)
                        .ok_or_else(|| err(format!("no field '{name}' on struct")))?;
                    let (v, vt) = self.expr(value)?;
                    let v = self.coerce(v, vt, fty);
                    self.emit(Inst::FieldSet {
                        base: bv,
                        sid,
                        idx,
                        val: v,
                    });
                    Ok(())
                } else if let Expr::Index { base, index, .. } = target {
                    self.lower_index_set(base, index, value)
                } else {
                    Err(err(
                        "only variable, field, and index assignment are supported by codegen yet",
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
            Stmt::Raise { value, .. } => self.lower_raise(value.as_ref()),
            Stmt::Try {
                body,
                excepts,
                finally,
                ..
            } => self.lower_try(body, excepts, finally.as_ref()),
            Stmt::With { .. } | Stmt::Match { .. } | Stmt::Spawn { .. } => Err(err(
                "this statement is not supported by codegen yet (Phase 1 slice)",
            )),
        }
    }

    /// Balance the `__cxa_begin_catch` performed on landing-pad entry. Called on
    /// each path that leaves a handler normally or re-raises.
    fn emit_end_catch(&mut self) {
        self.emit(Inst::Call {
            dest: None,
            func: "zap_eh_end_catch".to_string(),
            args: Vec::new(),
        });
    }

    /// `raise EXPR` — build the exception and begin unwinding via `zap_panic`
    /// (RUNTIME-ABI §"Exception / unwinding ABI"). For this increment the
    /// exception object is the message string itself; the typed exception
    /// hierarchy (D-0029) is deferred. `zap_panic` never returns, so the normal
    /// path is `unreachable` (inside a `try`, the invoke's unwind edge carries
    /// control to the landing pad instead).
    fn lower_raise(&mut self, value: Option<&Expr>) -> Result<(), LowerError> {
        let Some(e) = value else {
            return Err(err("bare 'raise' (re-raise) is not supported yet"));
        };
        let (v, vt) = self.expr(e)?;
        if vt != Ty::Ptr {
            return Err(err(
                "raise currently expects a str value (the exception message)",
            ));
        }
        self.throwing_call(None, "zap_panic", vec![v]);
        if !self.terminated() {
            self.set_term(Term::Unreachable);
        }
        Ok(())
    }

    /// `try: BODY except [Type [as e]]: HANDLER finally: FIN` with real stack
    /// unwinding. Calls in BODY become `invoke`s whose unwind edge targets a
    /// landing-pad block; the handler matches catch-all for this increment
    /// (type-directed `except` matching is deferred). `finally` runs on the
    /// normal path and on the handled/propagating paths. If no `except` clause
    /// is present the exception is re-raised after `finally` (Python semantics).
    fn lower_try(
        &mut self,
        body: &ast::Block,
        excepts: &[ast::ExceptClause],
        finally: Option<&ast::Block>,
    ) -> Result<(), LowerError> {
        let lp = self.new_block();
        let after = self.new_block();

        // --- protected body: calls unwind to `lp` ---
        self.landing_pads.push(lp);
        self.push_scope();
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.pop_scope();
        self.landing_pads.pop();
        // Normal completion: run `finally`, then continue past the `try`.
        if !self.terminated() {
            if let Some(f) = finally {
                self.lower_block(f)?;
            }
            self.ensure_br(after);
        }

        // If the body emitted no throwing call it cannot raise, so the handler
        // is dead — skip the landing pad entirely (LLVM requires a `landingpad`
        // block to have an `invoke` predecessor).
        if !self.used_lps.contains(&lp) {
            self.position(after);
            return Ok(());
        }

        // --- landing pad: entered while unwinding ---
        self.position(lp);
        let exc = self.new_value(Ty::Ptr);
        self.emit(Inst::LandingPad { dest: exc });
        if let Some(first) = excepts.first() {
            // Catch-all handler (this increment). `as e` binds the message string.
            self.push_scope();
            if let Some(bind) = &first.binding {
                let slot = self.alloca(Ty::Ptr);
                self.store(slot, exc);
                self.declare(bind, slot, Ty::Ptr);
            }
            for st in &first.body.stmts {
                self.stmt(st)?;
            }
            self.pop_scope();
            if !self.terminated() {
                self.emit_end_catch();
                if let Some(f) = finally {
                    self.lower_block(f)?;
                }
                self.ensure_br(after);
            }
            // NOTE: a second/third `except` clause is currently unreachable
            // (matching is catch-all). Type-directed dispatch is deferred.
        } else {
            // finally-only `try`: run cleanup, then propagate to the caller.
            if let Some(f) = finally {
                self.lower_block(f)?;
            }
            if !self.terminated() {
                self.emit_end_catch();
                self.throwing_call(None, "zap_panic", vec![exc]);
                if !self.terminated() {
                    self.set_term(Term::Unreachable);
                }
            }
        }

        self.position(after);
        Ok(())
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
                // `for x in <list>` — iterate by index over the list's length.
                let (lst, lty) = self.expr(iter)?;
                if let Ty::List(elem) = lty {
                    return self.lower_for_list(target, lst, elem, body);
                }
                if let Ty::Set(elem) = lty {
                    return self.lower_for_set(target, lst, elem, body);
                }
                return Err(err(
                    "only `for ... in range(...)`, `for ... in <list>`, and `for ... in <set>` are supported by codegen yet",
                ));
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

    /// `for x in xs` where `xs : list[T]`. Re-reads the length each iteration so
    /// a body that appends behaves like a live iterator.
    fn lower_for_list(
        &mut self,
        target: &Expr,
        lst: Value,
        elem: Elem,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let name = match target {
            Expr::Name(n, _) => n.clone(),
            _ => {
                return Err(err(
                    "only a simple loop variable is supported by codegen yet",
                ))
            }
        };

        self.push_scope();
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let var = self.alloca(elem.ty());
        self.declare(&name, var, elem.ty());

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let len = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(len),
            func: "zap_list_len".to_string(),
            args: vec![lst],
        });
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: len,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let e = self.new_value(elem.ty());
        self.emit(Inst::Call {
            dest: Some(e),
            func: format!("zap_list_get_{}", elem.val_suffix()),
            args: vec![lst, i],
        });
        self.store(var, e);
        self.loops.push((incr_bb, end_bb));
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.loops.pop();
        self.ensure_br(incr_bb);

        self.position(incr_bb);
        let cur2 = self.load(idx, Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        self.pop_scope();
        Ok(())
    }

    /// `for x in s` where `s : set[T]`. Snapshots the set's live elements into a
    /// fresh list (sets are unordered; iteration order is unspecified) and then
    /// iterates that list.
    fn lower_for_set(
        &mut self,
        target: &Expr,
        set_val: Value,
        elem: Elem,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let items = self.new_value(Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(items),
            func: "zap_set_items".to_string(),
            args: vec![set_val],
        });
        self.lower_for_list(target, items, elem, body)
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
            Expr::Str {
                value,
                raw,
                bytes,
                fstring,
                ..
            } => {
                if *fstring {
                    return Err(err("f-strings are not supported by codegen yet"));
                }
                let data = if *raw {
                    value.clone().into_bytes()
                } else {
                    unescape(value)
                };
                let _ = bytes; // byte strings share the string object for now
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::StrConst {
                    dest: d,
                    bytes: data,
                });
                Ok((d, Ty::Ptr))
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
            Expr::List(elems, _) => self.lower_list_lit(elems),
            Expr::Dict(items, _) => self.lower_dict_lit(items),
            Expr::Set(elems, _) => self.lower_set_lit(elems),
            Expr::Index { base, index, .. } => self.lower_index_get(base, index),
            Expr::Attr { base, name, .. } => {
                let (bv, bty) = self.expr(base)?;
                let Ty::Struct(sid) = bty else {
                    return Err(err("field access on a non-struct value"));
                };
                let (idx, fty) = self
                    .ctx
                    .field(sid, name)
                    .ok_or_else(|| err(format!("no field '{name}' on struct")))?;
                let d = self.new_value(fty);
                self.emit(Inst::FieldGet {
                    dest: d,
                    base: bv,
                    sid,
                    idx,
                });
                Ok((d, fty))
            }
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
        // Array construction `array[dtype]([...])` (ticket #24). The callee
        // parses as a subscript `array[dtype]`, so it is intercepted here before
        // the plain-name dispatch below.
        if let Expr::Index { base, index, .. } = callee {
            if is_name(base, "array") {
                return self.lower_array_new(index, args);
            }
        }
        if let Expr::Name(fname, _) = callee {
            if fname == "sum" {
                if args.len() != 1 {
                    return Err(err("sum() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                let Ty::Array(d) = vt else {
                    return Err(err("sum() expects an array[T] value"));
                };
                let dest = self.new_value(d.ty());
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: format!("zap_array_sum_{}", d.suffix()),
                    args: vec![v],
                });
                return Ok((dest, d.ty()));
            }
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
            if fname == "len" {
                if args.len() != 1 {
                    return Err(err("len() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                let func = match vt {
                    Ty::List(_) => "zap_list_len",
                    Ty::Dict(..) => "zap_dict_len",
                    Ty::Set(_) => "zap_set_len",
                    Ty::Array(_) => "zap_array_len",
                    Ty::Ptr => "zap_str_len",
                    _ => return Err(err("len() expects a list, dict, set, array, or str")),
                };
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: func.to_string(),
                    args: vec![v],
                });
                return Ok((d, Ty::I64));
            }
            // Struct construction: `StructName(fields...)`.
            if let Some(&sid) = self.ctx.ids.get(fname) {
                return self.construct(sid, args);
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
                self.throwing_call(dest, fname, argvals);
                let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
                return Ok((rv, if ret == Ty::Void { Ty::I32 } else { ret }));
            }
            Err(err(format!("call to unknown function '{fname}'")))
        } else if let Expr::Attr { base, name, .. } = callee {
            // Method call: `recv.method(args)`.
            let (recv, bty) = self.expr(base)?;
            if let Ty::List(elem) = bty {
                return self.lower_list_method(recv, elem, name, args);
            }
            if let Ty::Set(elem) = bty {
                return self.lower_set_method(recv, elem, name, args);
            }
            let Ty::Struct(sid) = bty else {
                return Err(err("method call on a non-struct value"));
            };
            let mangled = format!("{}__{}", self.ctx.names[sid as usize], name);
            let (ptys, ret) = self
                .sigs
                .get(&mangled)
                .cloned()
                .ok_or_else(|| err(format!("no method '{name}' on that struct")))?;
            let mut argvals = vec![recv]; // self
            for (i, a) in args.iter().enumerate() {
                let (v, vt) = self.expr(&a.value)?;
                let target = *ptys.get(i + 1).unwrap_or(&vt);
                argvals.push(self.coerce(v, vt, target));
            }
            let dest = if ret == Ty::Void {
                None
            } else {
                Some(self.new_value(ret))
            };
            self.throwing_call(dest, &mangled, argvals);
            let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
            Ok((rv, if ret == Ty::Void { Ty::I32 } else { ret }))
        } else {
            Err(err(
                "only direct function calls are supported by codegen yet",
            ))
        }
    }

    fn construct(&mut self, sid: u32, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        for (i, a) in args.iter().enumerate() {
            let (idx, fty) = match &a.name {
                Some(fname) => self
                    .ctx
                    .field(sid, fname)
                    .ok_or_else(|| err(format!("no field '{fname}' on struct")))?,
                None => {
                    let f = self.ctx.fields[sid as usize]
                        .get(i)
                        .ok_or_else(|| err("too many fields in constructor"))?;
                    (i as u32, f.1)
                }
            };
            let (v, vt) = self.expr(&a.value)?;
            let v = self.coerce(v, vt, fty);
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx,
                val: v,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    // --- collections (ticket #21) ---
    //
    // Lists and dicts lower to runtime calls (like strings), so the ZIR -> LLVM
    // step stays mechanical. The element/key/value `Elem` kind chosen here
    // selects the typed entry point (`..._i64` / `..._ptr` / `..._str`).

    // --- numeric arrays (ticket #24) ---
    //
    // Construction, element access, `len`, and `sum` lower to runtime calls
    // (like collections). Only the element-wise ops (`binop`) use a dedicated
    // ZIR node so codegen can emit SIMD; everything else stays a plain call.

    /// `array[dtype]([e0, e1, ...])` — allocate an array of the literal's length
    /// and fill it element-by-element. N-D literals, non-literal arguments
    /// (`array[f64](xs)`), and `zeros`/`arange` constructors are deferred.
    fn lower_array_new(
        &mut self,
        dtype_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let Expr::Name(dname, _) = dtype_expr else {
            return Err(err("array[...] expects a dtype name, e.g. array[f64]"));
        };
        let dtype = dtype_from_name(dname)?;
        if args.len() != 1 {
            return Err(err(
                "array[T](...) takes exactly one argument (a list literal)",
            ));
        }
        let Expr::List(elems, _) = &args[0].value else {
            return Err(err(
                "array[T](...) takes a list literal, e.g. array[f64]([1.0, 2.0, 3.0]) \
                 (N-D and non-literal construction are not supported yet)",
            ));
        };
        let code = self.iconst(Ty::I64, dtype.code());
        let n = self.iconst(Ty::I64, elems.len() as i64);
        let arr = self.new_value(Ty::Array(dtype));
        self.emit(Inst::Call {
            dest: Some(arr),
            func: "zap_array_new".to_string(),
            args: vec![code, n],
        });
        let setter = format!("zap_array_set_{}", dtype.suffix());
        for (i, e) in elems.iter().enumerate() {
            let (v, vt) = self.expr(e)?;
            let v = self.coerce(v, vt, dtype.ty());
            let idx = self.iconst(Ty::I64, i as i64);
            self.emit(Inst::Call {
                dest: None,
                func: setter.clone(),
                args: vec![arr, idx, v],
            });
        }
        Ok((arr, Ty::Array(dtype)))
    }

    fn lower_list_lit(&mut self, elems: &[Expr]) -> Result<(Value, Ty), LowerError> {
        // Evaluate elements left-to-right first; the first fixes the element
        // kind (an empty literal defaults to `i64`).
        let mut lowered = Vec::with_capacity(elems.len());
        for e in elems {
            lowered.push(self.expr(e)?);
        }
        let elem = match lowered.first() {
            Some(&(_, t)) => elem_kind(t)?,
            None => Elem::I64,
        };
        let list = self.new_value(Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(list),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let push = format!("zap_list_push_{}", elem.val_suffix());
        for (v, vt) in lowered {
            if elem_kind(vt)? != elem {
                return Err(err("list elements must all have the same representation"));
            }
            let v = self.coerce(v, vt, elem.ty());
            self.emit(Inst::Call {
                dest: None,
                func: push.clone(),
                args: vec![list, v],
            });
        }
        Ok((list, Ty::List(elem)))
    }

    fn lower_dict_lit(&mut self, items: &[(Expr, Expr)]) -> Result<(Value, Ty), LowerError> {
        let mut lowered = Vec::with_capacity(items.len());
        for (k, v) in items {
            let kv = self.expr(k)?;
            let vv = self.expr(v)?;
            lowered.push((kv, vv));
        }
        let (kk, vk) = match lowered.first() {
            Some(&((_, kt), (_, vt))) => (elem_kind(kt)?, elem_kind(vt)?),
            None => (Elem::I64, Elem::I64),
        };
        let dict = self.new_value(Ty::Dict(kk, vk));
        self.emit(Inst::Call {
            dest: Some(dict),
            func: "zap_dict_new".to_string(),
            args: vec![],
        });
        let set = format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix());
        for ((kv, kt), (vv, vt)) in lowered {
            if elem_kind(kt)? != kk || elem_kind(vt)? != vk {
                return Err(err(
                    "dict keys/values must all have the same representation",
                ));
            }
            let kv = self.coerce(kv, kt, kk.ty());
            let vv = self.coerce(vv, vt, vk.ty());
            self.emit(Inst::Call {
                dest: None,
                func: set.clone(),
                args: vec![dict, kv, vv],
            });
        }
        Ok((dict, Ty::Dict(kk, vk)))
    }

    fn lower_set_lit(&mut self, elems: &[Expr]) -> Result<(Value, Ty), LowerError> {
        // Elements are evaluated left-to-right; the first fixes the element kind
        // (an empty literal is a dict, so a set literal always has >= 1 element).
        let mut lowered = Vec::with_capacity(elems.len());
        for e in elems {
            lowered.push(self.expr(e)?);
        }
        let elem = match lowered.first() {
            Some(&(_, t)) => elem_kind(t)?,
            None => Elem::I64,
        };
        let set = self.new_value(Ty::Set(elem));
        self.emit(Inst::Call {
            dest: Some(set),
            func: "zap_set_new".to_string(),
            args: vec![],
        });
        let add = format!("zap_set_add_{}", elem.key_suffix());
        for (v, vt) in lowered {
            if elem_kind(vt)? != elem {
                return Err(err("set elements must all have the same representation"));
            }
            let v = self.coerce(v, vt, elem.ty());
            self.emit(Inst::Call {
                dest: None,
                func: add.clone(),
                args: vec![set, v],
            });
        }
        Ok((set, Ty::Set(elem)))
    }

    fn lower_set_method(
        &mut self,
        recv: Value,
        elem: Elem,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "add" => {
                if args.len() != 1 {
                    return Err(err("add() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if elem_kind(vt)? != elem {
                    return Err(err("add() value has the wrong element type"));
                }
                let v = self.coerce(v, vt, elem.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_set_add_{}", elem.key_suffix()),
                    args: vec![recv, v],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            other => Err(err(format!("set has no method '{other}' (supported: add)"))),
        }
    }

    fn lower_index_get(&mut self, base: &Expr, index: &Expr) -> Result<(Value, Ty), LowerError> {
        let (bv, bty) = self.expr(base)?;
        match bty {
            Ty::List(elem) => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce(iv, it, Ty::I64);
                let d = self.new_value(elem.ty());
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_list_get_{}", elem.val_suffix()),
                    args: vec![bv, iv],
                });
                Ok((d, elem.ty()))
            }
            Ty::Dict(kk, vk) => {
                let (kv, kt) = self.expr(index)?;
                let kv = self.coerce(kv, kt, kk.ty());
                let d = self.new_value(vk.ty());
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_dict_get_{}_{}", kk.key_suffix(), vk.val_suffix()),
                    args: vec![bv, kv],
                });
                Ok((d, vk.ty()))
            }
            Ty::Array(d) => {
                // 1-D element access `a[i]`. Multi-axis indexing `m[i, j]` and
                // slices are deferred (N-D track).
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce(iv, it, Ty::I64);
                let dest = self.new_value(d.ty());
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: format!("zap_array_get_{}", d.suffix()),
                    args: vec![bv, iv],
                });
                Ok((dest, d.ty()))
            }
            _ => Err(err(
                "indexing is only supported on list, dict, and array values",
            )),
        }
    }

    fn lower_index_set(
        &mut self,
        base: &Expr,
        index: &Expr,
        value: &Expr,
    ) -> Result<(), LowerError> {
        let (bv, bty) = self.expr(base)?;
        match bty {
            Ty::List(elem) => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce(iv, it, Ty::I64);
                let (v, vt) = self.expr(value)?;
                let v = self.coerce(v, vt, elem.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_list_set_{}", elem.val_suffix()),
                    args: vec![bv, iv, v],
                });
                Ok(())
            }
            Ty::Dict(kk, vk) => {
                let (kv, kt) = self.expr(index)?;
                let kv = self.coerce(kv, kt, kk.ty());
                let (v, vt) = self.expr(value)?;
                let v = self.coerce(v, vt, vk.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix()),
                    args: vec![bv, kv, v],
                });
                Ok(())
            }
            Ty::Array(d) => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce(iv, it, Ty::I64);
                let (v, vt) = self.expr(value)?;
                let v = self.coerce(v, vt, d.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_array_set_{}", d.suffix()),
                    args: vec![bv, iv, v],
                });
                Ok(())
            }
            _ => Err(err(
                "indexed assignment is only supported on list, dict, and array values",
            )),
        }
    }

    fn lower_list_method(
        &mut self,
        recv: Value,
        elem: Elem,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "append" => {
                if args.len() != 1 {
                    return Err(err("append() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if elem_kind(vt)? != elem {
                    return Err(err("append() value has the wrong element type"));
                }
                let v = self.coerce(v, vt, elem.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_list_push_{}", elem.val_suffix()),
                    args: vec![recv, v],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            other => Err(err(format!(
                "list has no method '{other}' (supported: append)"
            ))),
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

        // Element-wise numeric-array ops (ticket #24): same-shape `a + b` and
        // scalar broadcast `a + 2.0`, lowered to a SIMD kernel in codegen.
        if matches!(lt, Ty::Array(_)) || matches!(rt, Ty::Array(_)) {
            return self.array_binop(op, l, lt, r, rt);
        }

        // Membership: `x in <set>` -> `zap_set_contains_*`, normalized to a bool.
        if matches!(op, In) {
            let Ty::Set(elem) = rt else {
                return Err(err("'in' is only supported with a set on the right"));
            };
            if elem_kind(lt)? != elem {
                return Err(err("'in' element has the wrong type for this set"));
            }
            let l = self.coerce(l, lt, elem.ty());
            let word = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(word),
                func: format!("zap_set_contains_{}", elem.key_suffix()),
                args: vec![r, l],
            });
            let zero = self.iconst(Ty::I64, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: word,
                rhs: zero,
            });
            return Ok((d, Ty::I1));
        }

        // Reference-typed operands: the only supported one is `str + str`.
        if matches!(lt, Ty::Ptr) || matches!(rt, Ty::Ptr) {
            if matches!(op, Add) && lt == Ty::Ptr && rt == Ty::Ptr {
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_concat".to_string(),
                    args: vec![l, r],
                });
                return Ok((d, Ty::Ptr));
            }
            return Err(err(
                "unsupported operation on a reference type (only str + str)",
            ));
        }

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

    /// Element-wise `+`/`-`/`*` on arrays, including scalar broadcast. Emits the
    /// [`Inst::ArrayBinOp`] node codegen vectorizes. Both-array operands must
    /// share a dtype (no promotion yet); `scalar op array` is only accepted for
    /// the commutative `+`/`*` (reordered to `array op scalar`), so the SIMD
    /// kernel always computes `array[i] op scalar`.
    fn array_binop(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let irop = match op {
            Add => BinOp::Add,
            Sub => BinOp::Sub,
            Mul => BinOp::Mul,
            _ => {
                return Err(err(
                    "only +, -, and * are supported on arrays yet (/, reductions beyond sum, etc. are deferred)",
                ))
            }
        };
        let is_scalar = |t: Ty| t.is_float() || t.int_width().is_some();
        match (lt, rt) {
            (Ty::Array(ld), Ty::Array(rd)) => {
                if ld != rd {
                    return Err(err(
                        "element-wise array ops require the same dtype on both sides (dtype promotion is deferred)",
                    ));
                }
                let dest = self.new_value(Ty::Array(ld));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    dtype: ld,
                    lhs: l,
                    rhs: r,
                    scalar: false,
                });
                Ok((dest, Ty::Array(ld)))
            }
            (Ty::Array(ld), _) if is_scalar(rt) => {
                let s = self.coerce(r, rt, ld.ty());
                let dest = self.new_value(Ty::Array(ld));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    dtype: ld,
                    lhs: l,
                    rhs: s,
                    scalar: true,
                });
                Ok((dest, Ty::Array(ld)))
            }
            (_, Ty::Array(rd)) if is_scalar(lt) => {
                if !matches!(op, Add | Mul) {
                    return Err(err(
                        "scalar-on-the-left broadcast is only supported for + and * (write `a - scalar`, not `scalar - a`)",
                    ));
                }
                let s = self.coerce(l, lt, rd.ty());
                let dest = self.new_value(Ty::Array(rd));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    dtype: rd,
                    lhs: r,
                    rhs: s,
                    scalar: true,
                });
                Ok((dest, Ty::Array(rd)))
            }
            _ => Err(err(
                "unsupported array operand (element-wise ops take two arrays of the same dtype or an array and a numeric scalar)",
            )),
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

/// Process the common escape sequences in a (non-raw) string literal.
fn unescape(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => out.push(b'\n'),
            Some('t') => out.push(b'\t'),
            Some('r') => out.push(b'\r'),
            Some('0') => out.push(0),
            Some('\\') => out.push(b'\\'),
            Some('\'') => out.push(b'\''),
            Some('"') => out.push(b'"'),
            Some(other) => {
                out.push(b'\\');
                let mut buf = [0u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => out.push(b'\\'),
        }
    }
    out
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
