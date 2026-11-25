//! Statement + control-flow lowering, and exception handling (#1125).
//!
//! The statement half of AST→ZIR lowering: the `stmt` dispatcher, bind targets,
//! `spawn`/`scope` (structured concurrency), `raise`/`assert`/`with`/`arena`,
//! the `try`/`except`(+`try*`) exception machinery (owned-region unwinding,
//! cancel landing pads), `if`/`while`/`do-while`, and the full `for`/iterator /
//! comprehension family. Extracted verbatim from `lower.rs`; a child module of
//! `lower`, so these `impl super::FnLower` methods still see every private item
//! of `lower` (helpers like `throwing_call`, `err`, the elem-kind free fns, the
//! interners) for free. `use super::*` re-exposes exactly that parent scope.

use super::*;

impl<'a> FnLower<'a> {
    pub(super) fn stmt(&mut self, st: &Stmt) -> Result<(), LowerError> {
        match st {
            // `when target.*` conditional-compilation blocks are expanded by
            // `zapc_syntax::cfg::resolve_cfg` before lowering (the driver resolves
            // against the active target; sema re-resolves for the host). One
            // reaching lowering is an internal invariant violation (I#139).
            Stmt::Cfg(_) => Err(err(
                "internal error: unresolved `when target.*` block reached \
                 lowering (expected cfg resolution first)",
            )),
            // `select` (D-0031, #1002): channel multiplexing via `zap_select`.
            Stmt::Select { cases, .. } => self.lower_select(cases),
            Stmt::Let {
                ty, name, value, ..
            } => {
                // With a `fn(...)` annotation, a `lambda` initializer is lowered
                // against that type (I#163) — its parameter types come from the
                // annotation. Otherwise lower-then-coerce as usual.
                let (v, declared) = match ty {
                    Some(t) => {
                        let declared = resolve_ty(t, &self.ctx.tyenv())?;
                        (
                            self.expr_coerced(value, declared, zapc_diag::ErrorCode::ZAP4001)?,
                            declared,
                        )
                    }
                    None => {
                        let (v, vt) = self.expr(value)?;
                        (v, vt)
                    }
                };
                // Value structs are copied on binding (D-0051): `T b = a` makes
                // `b` an independent instance of the value aggregate.
                let v = self.copy_if_value(v, declared, value);
                // `decl_slot` is a plain alloca+declare outside a generator; in a
                // generator it records the local (discovery) or reuses the state
                // struct-backed per-call slot (emit), so the binding persists
                // across `yield`s (I#67).
                let slot = self.decl_slot(name, declared);
                self.store(slot, v);
                Ok(())
            }
            Stmt::Assign { target, value, .. } => {
                if let Expr::Name(n, span) = target {
                    // A local slot, or a module-level mutable global's slot
                    // (D-0112, #662) — stored into identically. Resolve the slot
                    // (and its declared type) first so the right-hand side is
                    // lowered *against* that type.
                    let (ptr, ty) = self
                        .name_lvalue(n)
                        .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
                    // Lower the value in the slot's type context via the shared
                    // `expr_coerced` chokepoint — the same path constructor
                    // fields, field assignment, index/key assignment, `.append`,
                    // call arguments and returns use. That is what boxes a bare
                    // payload `T` reassigned into an `Optional[T]` local as a
                    // tagged/boxed `Some(T)` (#1262's symmetric partner, #1362)
                    // and lowers a bare `None` to the slot's canonical empty
                    // option, instead of storing the raw scalar/pointer straight
                    // into the Optional slot (a later `.is_some()` / `.unwrap()`
                    // would otherwise dereference the raw bits as an Optional and
                    // SIGSEGV — a memory-safety violation in a checked program).
                    // `expr_coerced` also performs the `fn(...)` signature check
                    // (I#465) and the final `coerce`, so no behaviour is lost for
                    // the non-Optional cases this path already handled.
                    let v = self.expr_coerced(value, ty, zapc_diag::ErrorCode::ZAP4001)?;
                    // Value structs copy on assignment (D-0051).
                    let v = self.copy_if_value(v, ty, value);
                    self.store_name(n, ptr, ty, v, *span);
                    Ok(())
                } else if let Expr::Attr { base, name, span } = target {
                    let (bv, bty) = self.expr(base)?;
                    let Ty::Struct(sid) = bty else {
                        return Err(err("field assignment on a non-struct value"));
                    };
                    let (idx, fty) = self
                        .ctx
                        .field(sid, name)
                        .ok_or_else(|| err(format!("no field '{name}' on struct")))?;
                    // A `fn(...)` field accepts a lambda / bare function value
                    // (I#163), lowered against the field type.
                    let v = self.expr_coerced(value, fty, zapc_diag::ErrorCode::ZAP4001)?;
                    // Storing a value struct into a field copies it (D-0051), so
                    // the field holds an independent instance. The field is a
                    // pointer, so a by-value (inline-eligible) call/return result
                    // — physically a stack spill — is boxed onto the heap too, or
                    // it would dangle once the producing frame returns and a
                    // later read of the field would be corrupted (#1446, the
                    // field-reassignment twin of the #1400 construction/enum-
                    // payload/Optional-wrap stores already using this chokepoint).
                    let v = self.copy_field_value(v, fty, value);
                    // A field store on a null non-`Optional` reference raises a
                    // located `NullReferenceError` (#1489/#1493), the write twin
                    // of the field-read guard. Value structs are inline, not
                    // nullable, so they need no guard.
                    if !self.ctx.is_value_struct(sid) {
                        let owner = self.ctx.names[sid as usize].clone();
                        self.null_deref_guard(
                            bv,
                            NullDerefSite::FieldStore,
                            Some(name),
                            Some(&owner),
                            *span,
                        );
                    }
                    // --race (#684): a write of a shared heap struct field.
                    self.emit_race_field(bv, sid, idx, name, true, *span);
                    self.emit(Inst::FieldSet {
                        base: bv,
                        sid,
                        idx,
                        val: v,
                    });
                    Ok(())
                } else if let Expr::Index { base, index, span } = target {
                    self.lower_index_set(base, index, value, *span)
                } else if let Expr::Slice {
                    base, lo, hi, step, ..
                } = target
                {
                    // Sliced assignment target `xs[lo:hi:step] = ys` (#1005).
                    let (rv, rt) = self.expr(value)?;
                    self.store_slice(base, lo.as_deref(), hi.as_deref(), step.as_deref(), rv, rt)
                } else {
                    Err(err(
                        "only variable, field, index, and slice assignment are supported by \
                         codegen yet",
                    ))
                }
            }
            Stmt::AssignChain { targets, value, .. } => {
                // Chained assignment `t0 = t1 = ... = value` (#594). The RHS is
                // evaluated EXACTLY ONCE and then stored into each target, left
                // to right (Python order). Per-target `base`/`index` sub-
                // expressions are still evaluated at each store, so a subscript
                // target's own side effects happen in source order after the RHS.
                let (v, vt) = match targets.iter().find_map(|t| self.assignment_target_ty(t)) {
                    Some(ty) => self.expr_collection_context(value, ty)?,
                    None => self.expr(value)?,
                };
                for target in targets {
                    self.store_computed(target, v, vt, value)?;
                }
                Ok(())
            }
            Stmt::Del { target, .. } => {
                // `del d[k]` / `del xs[i]` (#595). The parser guarantees the
                // target is a subscript; removal reuses the dict/list element
                // operations (see `lower_index_del`).
                let Expr::Index { base, index, .. } = target else {
                    return Err(err("del requires a subscript target"));
                };
                self.lower_index_del(base, index)
            }
            Stmt::AugAssign {
                target, op, value, ..
            } => {
                if let Expr::Name(n, span) = target {
                    // A local, or a module-level mutable global (D-0112, #662):
                    // read-modify-write through the same slot.
                    let (ptr, ty) = self
                        .name_lvalue(n)
                        .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
                    let cur = self.load(ptr, ty);
                    let (rhs, rt) = self.expr(value)?;
                    // In-place augmented assignment (D-0047): if the LHS is a user
                    // struct that defines the matching in-place dunder, `a OP= b`
                    // calls `a.__iOP__(b)`, which mutates the operand in place and
                    // returns it — no fresh allocation as the binary `__OP__` form
                    // would need. Otherwise fall back to deriving from the binary
                    // operator (`a = a.__OP__(b)`). Reference structs share
                    // identity (D-0023), so the in-place mutation is the intended
                    // visible effect; the store-back is a no-op rebind for them and
                    // keeps the value-struct / scalar paths correct.
                    let (res, rty) = self.aug_combine(*op, cur, ty, rhs, rt)?;
                    let res = self.coerce(res, rty, ty);
                    self.store_name(n, ptr, ty, res, *span);
                    Ok(())
                } else if let Expr::Index { base, index, span } = target {
                    // `xs[i] OP= v` / `d[k] OP= v` (#750): the base and index are
                    // evaluated exactly once, then it is a read-modify-write of
                    // that one element.
                    self.lower_aug_index(base, index, *op, value, *span)
                } else if let Expr::Attr { base, name, span } = target {
                    // `obj.f OP= v` (#750): evaluate the base once, then
                    // read-modify-write the field.
                    self.lower_aug_field(base, name, *op, value, *span)
                } else {
                    Err(err(
                        "augmented assignment target must be a name, subscript, or field",
                    ))
                }
            }
            Stmt::Expr(e) => {
                // A statement-position `yield` drives the generator state machine
                // (I#67); every other position is deferred (see `expr`).
                if let Expr::Yield { value, .. } = e {
                    return self.lower_yield_stmt(value.as_deref());
                }
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
            // `comptime for` (E2/E3 surface, #1297) is unrolled by the comptime
            // engine (#1251/#1298), not lowered directly. Sema rejects it with
            // ZAP0002 until that wiring lands, so this is an honest backstop:
            // reaching lowering means the deferral was bypassed.
            Stmt::ComptimeFor { .. } => Err(err(
                "`comptime for` reached lowering but comptime evaluation is not \
                 wired yet (comptime/reflect surface epic #1296, #1298)",
            )),
            // Inside a generator, `return` ends iteration (D-0048/PEP 479): jump
            // to the shared exhaustion block (sema rejects `return <value>`).
            Stmt::Return { .. } if self.gen_entry.is_some() => {
                let done = self.gen_entry.as_ref().unwrap().done_block;
                self.set_term(Term::Br(done));
                Ok(())
            }
            Stmt::Return { value, .. } => {
                match value {
                    Some(e) => {
                        let ret = self.func.ret;
                        // `return <lambda>` in a `fn(...)`-returning function is
                        // lowered against the return type (I#163) — this is how a
                        // function builds and returns a closure over its locals.
                        let v = if ret == Ty::Void {
                            self.expr(e)?.0
                        } else {
                            self.expr_coerced(e, ret, zapc_diag::ErrorCode::ZAP4003)?
                        };
                        // Returning a value struct yields an instance independent
                        // of the callee's locals (D-0051). An inline-eligible
                        // aggregate is returned by value (register/sret), which is
                        // already that copy; a non-inline value struct is deep-
                        // copied here. See `copy_for_pass`.
                        let v = self.copy_for_pass(v, ret, e);
                        // Run `with`/`try-finally` cleanups before returning so
                        // `__exit__`/`finally` execute on the early-return path
                        // (I#185, D-0069). The return value is evaluated first
                        // (Python order).
                        self.run_cleanups(0)?;
                        if !self.terminated() {
                            self.set_term(Term::Ret(Some(v)));
                        }
                    }
                    None => {
                        self.run_cleanups(0)?;
                        if !self.terminated() {
                            self.set_term(Term::Ret(None));
                        }
                    }
                }
                Ok(())
            }
            Stmt::Pass(_) => {
                // `pass` is a no-op (#591): it lowers to no IR at all.
                Ok(())
            }
            Stmt::Break(_) => {
                let (brk, base) = {
                    let f = self
                        .loops
                        .last()
                        .ok_or_else(|| err("'break' outside a loop"))?;
                    (f.brk, f.cleanup_base)
                };
                self.run_cleanups(base)?;
                if !self.terminated() {
                    self.set_term(Term::Br(brk));
                }
                Ok(())
            }
            Stmt::Continue(_) => {
                let (cont, base) = {
                    let f = self
                        .loops
                        .last()
                        .ok_or_else(|| err("'continue' outside a loop"))?;
                    (f.cont, f.cleanup_base)
                };
                self.run_cleanups(base)?;
                if !self.terminated() {
                    self.set_term(Term::Br(cont));
                }
                Ok(())
            }
            Stmt::Raise { value, cause, .. } => self.lower_raise(value.as_ref(), cause.as_ref()),
            Stmt::Assert {
                cond,
                msg,
                cond_src,
                ..
            } => self.lower_assert(cond, msg.as_ref(), cond_src),
            Stmt::Try {
                body,
                excepts,
                finally,
                ..
            } => self.lower_try(body, excepts, finally.as_ref()),
            Stmt::Spawn { call, detached, .. } => self.lower_spawn(call, *detached),
            Stmt::Scope { opts, body, .. } => self.lower_scope(opts, body),
            // `unsafe:` (D-0200) has no runtime effect — it is a compile-time
            // permission checked in sema; lower its body as an ordinary block.
            Stmt::Unsafe { body, .. } => self.lower_block(body),
            Stmt::Match {
                scrutinee, arms, ..
            } => self.lower_match(scrutinee, arms),
            // Tuple-destructuring declaration / assignment (D-0049): lower the
            // value to its tuple aggregate, then bind each target via `FieldGet`.
            Stmt::LetTuple { target, value, .. } => {
                let (v, vt) = self.expr(value)?;
                self.lower_bind_target(target, v, vt, true)
            }
            Stmt::AssignTuple { target, value, .. } => {
                let (v, vt) = self.expr(value)?;
                self.lower_bind_target(target, v, vt, false)
            }
            Stmt::With { items, body, .. } => self.lower_with(items, body),
        }
    }

    /// Bind a destructuring target to a value (D-0049). `is_decl` chooses between
    /// introducing new locals (declaration) and storing into existing lvalues
    /// (reassignment). A `Tuple` target reads each element out of the aggregate
    /// with `FieldGet`; a `*rest` rebuilds the remaining elements into a sub-tuple.
    fn lower_bind_target(
        &mut self,
        t: &BindTarget,
        val: Value,
        ty: Ty,
        is_decl: bool,
    ) -> Result<(), LowerError> {
        match t {
            BindTarget::Elem(e) => self.lower_bind_leaf(e, val, ty, is_decl),
            BindTarget::Rest(_, _) => {
                Err(err("'*rest' is only valid as an element of a tuple target"))
            }
            BindTarget::Tuple(elems, _) => {
                let Ty::Struct(sid) = ty else {
                    return Err(err("cannot destructure a non-tuple value"));
                };
                let field_tys = self
                    .ctx
                    .tuples
                    .borrow()
                    .fields(sid)
                    .ok_or_else(|| err("cannot destructure a non-tuple value"))?;
                self.lower_bind_tuple(elems, val, sid, &field_tys, is_decl)
            }
        }
    }

    /// Bind the elements of a tuple target against the aggregate's fields,
    /// honoring at most one `*rest` (which captures the remaining fields as a
    /// freshly built sub-tuple). Arity is validated in sema.
    fn lower_bind_tuple(
        &mut self,
        elems: &[BindTarget],
        base: Value,
        sid: u32,
        field_tys: &[Ty],
        is_decl: bool,
    ) -> Result<(), LowerError> {
        let rest_pos = elems.iter().position(|e| matches!(e, BindTarget::Rest(..)));
        match rest_pos {
            None => {
                if elems.len() != field_tys.len() {
                    return Err(err(format!(
                        "tuple destructuring expects {} element(s), found {}",
                        field_tys.len(),
                        elems.len()
                    )));
                }
                for (idx, e) in elems.iter().enumerate() {
                    let fty = field_tys[idx];
                    let fv = self.field_get(base, sid, idx as u32, fty);
                    self.lower_bind_target(e, fv, fty, is_decl)?;
                }
                Ok(())
            }
            Some(r) => {
                let after = elems.len() - r - 1;
                if field_tys.len() + 1 < elems.len() {
                    return Err(err("tuple destructuring with '*rest' needs more elements"));
                }
                let tail_start = field_tys.len() - after;
                // Fixed elements before the rest.
                for (idx, e) in elems[..r].iter().enumerate() {
                    let fty = field_tys[idx];
                    let fv = self.field_get(base, sid, idx as u32, fty);
                    self.lower_bind_target(e, fv, fty, is_decl)?;
                }
                // `*rest`: rebuild the middle fields into a new sub-tuple.
                let rest_tys: Vec<Ty> = field_tys[r..tail_start].to_vec();
                let rest_sid = self.ctx.tuples.borrow_mut().intern(rest_tys.clone());
                let rest_val = self.new_value(Ty::Struct(rest_sid));
                self.emit(Inst::StructNew {
                    dest: rest_val,
                    sid: rest_sid,
                });
                for (j, fty) in rest_tys.iter().enumerate() {
                    let fv = self.field_get(base, sid, (r + j) as u32, *fty);
                    self.emit(Inst::FieldSet {
                        base: rest_val,
                        sid: rest_sid,
                        idx: j as u32,
                        val: fv,
                    });
                }
                if let BindTarget::Rest(inner, _) = &elems[r] {
                    self.lower_bind_target(inner, rest_val, Ty::Struct(rest_sid), is_decl)?;
                }
                // Fixed elements after the rest.
                for (k, e) in elems[r + 1..].iter().enumerate() {
                    let idx = tail_start + k;
                    let fty = field_tys[idx];
                    let fv = self.field_get(base, sid, idx as u32, fty);
                    self.lower_bind_target(e, fv, fty, is_decl)?;
                }
                Ok(())
            }
        }
    }

    /// Emit a `FieldGet` yielding field `idx` of tuple/struct `sid`.
    pub(super) fn field_get(&mut self, base: Value, sid: u32, idx: u32, fty: Ty) -> Value {
        let d = self.new_value(fty);
        self.emit(Inst::FieldGet {
            dest: d,
            base,
            sid,
            idx,
        });
        d
    }

    /// Bind one leaf of a destructuring target: a new local (declaration) or an
    /// existing lvalue (reassignment — a name or a struct field).
    fn lower_bind_leaf(
        &mut self,
        e: &Expr,
        val: Value,
        ty: Ty,
        is_decl: bool,
    ) -> Result<(), LowerError> {
        if is_decl {
            let Expr::Name(name, _) = e else {
                return Err(err("a declaration target must be a name"));
            };
            let slot = self.alloca(ty);
            self.store(slot, val);
            self.declare(name, slot, ty);
            return Ok(());
        }
        match e {
            Expr::Name(n, span) => {
                // A local, or a module-level mutable global (D-0112, #662).
                let (ptr, dt) = self
                    .name_lvalue(n)
                    .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
                let v = self.coerce(val, ty, dt);
                self.store_name(n, ptr, dt, v, *span);
                Ok(())
            }
            Expr::Attr { base, name, .. } => {
                let (bv, bty) = self.expr(base)?;
                let Ty::Struct(sid) = bty else {
                    return Err(err("field assignment on a non-struct value"));
                };
                let (idx, fty) = self
                    .ctx
                    .field(sid, name)
                    .ok_or_else(|| err(format!("no field '{name}' on struct")))?;
                let v = self.coerce(val, ty, fty);
                self.emit(Inst::FieldSet {
                    base: bv,
                    sid,
                    idx,
                    val: v,
                });
                Ok(())
            }
            _ => Err(err(
                "only name and field targets are supported in tuple destructuring assignment yet",
            )),
        }
    }

    /// Lower `spawn <call>` (ticket #25, D-0013; callable forms I#475). The
    /// operand must be a call; its arguments (and, for a method, the receiver)
    /// are evaluated eagerly here, in the spawning coroutine — so the capture
    /// happens-before the child runs (D-0034) — coerced to the callee's
    /// parameter types, copied for value-struct safety, and handed to
    /// [`Inst::Spawn`], which codegen turns into a GC-registered coroutine.
    /// Fire-and-forget: the spawn yields no handle; ownership is the innermost
    /// `scope:` (or the root scope, or nothing for `detached` — D-0044).
    ///
    /// Three callable forms are supported, all with **positional arguments
    /// only** (a deliberate v1 restriction, D-0118):
    /// 1. a named top-level function — `spawn worker(ch)`;
    /// 2. a concrete user-struct method — `spawn server.handle(conn)` — desugared
    ///    to a named spawn of the mangled method with the receiver as argument 0;
    /// 3. a first-class function value — `spawn f(x)` where `f` is a variable,
    ///    field, or expression of `fn(...)->...` type (a bound lambda, closure,
    ///    or function-value, I#163) — lowered through a generated per-signature
    ///    thunk that performs the indirect [`Inst::CallClosure`].
    ///
    /// A *bare lambda literal* callee (`spawn (lambda: ...)()`) is rejected
    /// (`ZAP5076`): a lambda has no type without a function-typed context, which
    /// a spawn site does not supply (the same limit as a normal call position);
    /// bind it to a name first. Keyword/`**`/`*` arguments, an arity mismatch, a
    /// non-struct method receiver, and a non-callable operand are also rejected.
    fn lower_spawn(&mut self, call: &Expr, detached: bool) -> Result<(), LowerError> {
        let Expr::Call { callee, args, .. } = call else {
            return Err(err("spawn operand must be a call expression (D-0013)"));
        };
        // Positional-only across every spawn form (D-0118): keyword and spread
        // arguments are not captured by `spawn` in v1.
        if args.iter().any(|a| a.name.is_some())
            || args
                .iter()
                .any(|a| !matches!(a.spread, ast::ArgSpread::None))
        {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                "spawn arguments must all be supplied positionally (no keyword or spread arguments)",
            ));
        }
        // Ownership (D-0044): `spawn detached` opts out of all scopes; otherwise
        // the innermost enclosing `scope:` owns it, falling back to the implicit
        // root scope at the top level.
        let owner = if detached {
            Owner::Detached
        } else if let Some(&s) = self.scope_stack.last() {
            Owner::Scope(s)
        } else {
            Owner::Root
        };

        // Form 1 — a named top-level function (the fast path): emit a direct
        // `Inst::Spawn` on the function symbol, no thunk. A name shadowed by a
        // local falls through to the function-value form below.
        if let Expr::Name(fname, _) = callee.as_ref() {
            if self.lookup(fname).is_none() {
                let (ptys, _ret) = self.sigs.get(fname).cloned().ok_or_else(|| {
                    err_code(
                        zapc_diag::ErrorCode::ZAP5076,
                        format!("spawn of unknown function '{fname}'"),
                    )
                })?;
                if args.len() != ptys.len() {
                    return Err(err_code(
                        zapc_diag::ErrorCode::ZAP5076,
                        format!(
                            "spawn of '{fname}' expects {} positional argument(s), found {}",
                            ptys.len(),
                            args.len()
                        ),
                    ));
                }
                let mut argvals = Vec::with_capacity(args.len());
                for (i, a) in args.iter().enumerate() {
                    let (v, vt) = self.expr(&a.value)?;
                    let target = *ptys.get(i).unwrap_or(&vt);
                    let v = self.coerce(v, vt, target);
                    // A value struct captured by a coroutine is copied (D-0051)
                    // so it outlives the spawning frame; an inline-eligible
                    // aggregate is stored by value into the GC closure (codegen),
                    // so no explicit copy is emitted. See `copy_for_pass`.
                    let v = self.copy_for_pass(v, target, &a.value);
                    argvals.push(v);
                }
                self.emit(Inst::Spawn {
                    func: fname.clone(),
                    args: argvals,
                    owner,
                    name: None,
                });
                return Ok(());
            }
        }

        // Form 2 — a concrete user-struct method call `recv.method(args)`.
        if let Expr::Attr { base, name, .. } = callee.as_ref() {
            return self.lower_spawn_method(base, name, args, owner);
        }

        // Form 3 — a first-class function value (bound lambda / closure /
        // function-value expression).
        self.lower_spawn_fnvalue(callee, args, owner)
    }

    /// Lower form 2 of `spawn` (I#475): a method call `recv.method(args)` on a
    /// concrete user struct. The receiver is captured as argument 0 — copied for
    /// value-struct safety exactly like any other captured argument, so a value
    /// receiver outlives the spawning frame and a reference receiver is a shared
    /// GC pointer the spawn env's precise descriptor traces. Only a plain,
    /// statically-resolved struct method qualifies; a built-in-container, string,
    /// Python, interface (dynamic dispatch), atomic, or generator receiver is
    /// rejected with `ZAP5076`.
    fn lower_spawn_method(
        &mut self,
        base: &Expr,
        name: &str,
        args: &[ast::Arg],
        owner: Owner,
    ) -> Result<(), LowerError> {
        let (recv, bty) = self.expr(base)?;
        let Ty::Struct(sid) = bty else {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                "spawn of a method call is only supported on a user-struct receiver",
            ));
        };
        if is_atomic_struct(self.ctx, sid) || self.ctx.iid_of_fat_sid(sid).is_some() {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                "spawn of a method on an atomic or interface value is not supported",
            ));
        }
        let mangled = format!("{}__{}", self.ctx.names[sid as usize], name);
        if self.ctx.generators.contains_key(&mangled) {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                format!("spawn of a generator method '{name}' is not supported"),
            ));
        }
        let (ptys, _ret) = self.sigs.get(&mangled).cloned().ok_or_else(|| {
            err_code(
                zapc_diag::ErrorCode::ZAP5076,
                format!(
                    "spawn of unknown method '{name}' on struct '{}'",
                    self.ctx.names[sid as usize]
                ),
            )
        })?;
        // `ptys[0]` is the implicit `self`; the explicit arguments fill the rest,
        // positionally and in full (no defaults — spawn is positional-only).
        if args.len() + 1 != ptys.len() {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                format!(
                    "spawn of '{}.{name}' expects {} positional argument(s), found {}",
                    self.ctx.names[sid as usize],
                    ptys.len().saturating_sub(1),
                    args.len()
                ),
            ));
        }
        let self_ty = *ptys.first().unwrap_or(&bty);
        let recv = self.copy_for_pass(recv, self_ty, base);
        let mut argvals = Vec::with_capacity(args.len() + 1);
        argvals.push(recv);
        for (i, a) in args.iter().enumerate() {
            let (v, vt) = self.expr(&a.value)?;
            let target = *ptys.get(i + 1).unwrap_or(&vt);
            let v = self.coerce(v, vt, target);
            let v = self.copy_for_pass(v, target, &a.value);
            argvals.push(v);
        }
        self.emit(Inst::Spawn {
            func: mangled,
            args: argvals,
            owner,
            name: Some(format!("{}.{name}", self.ctx.names[sid as usize])),
        });
        Ok(())
    }

    /// Lower form 3 of `spawn` (I#475): a first-class function value callee
    /// (`spawn f(x)` where `f : fn(...)->...`). The callee and arguments are
    /// evaluated eagerly in the parent; the call is performed in the child
    /// through a generated per-signature thunk `$spawnthunk$<sig>` that runs a
    /// single [`Inst::CallClosure`]. The thunk is a plain top-level function, so
    /// the existing named-spawn trampoline hosts it as the coroutine body and its
    /// catch-all landing pad routes a child failure to the owning scope exactly
    /// as for a named spawn. The closure record is the first captured value, a GC
    /// pointer the spawn env's precise descriptor traces.
    fn lower_spawn_fnvalue(
        &mut self,
        callee: &Expr,
        args: &[ast::Arg],
        owner: Owner,
    ) -> Result<(), LowerError> {
        if matches!(callee, Expr::Lambda { .. }) {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                "spawn of a bare lambda literal is not supported; bind it to a name first \
                 (e.g. `auto f = lambda: ...; spawn f()`)",
            ));
        }
        let (cv, ct) = self.expr(callee)?;
        let Ty::Func(sig_idx) = ct else {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                "spawn requires a named function, a user-struct method call, or a \
                 first-class function value",
            ));
        };
        let sig = self.ctx.fn_sigs.borrow().get(sig_idx).clone();
        if args.len() != sig.params.len() {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                format!(
                    "spawn of this function value expects {} positional argument(s), found {}",
                    sig.params.len(),
                    args.len()
                ),
            ));
        }
        let mut argvals = Vec::with_capacity(args.len() + 1);
        // The closure record is captured first (env field 0, a traced pointer).
        argvals.push(cv);
        for (i, a) in args.iter().enumerate() {
            let target = sig.params[i];
            let v = self.expr_coerced(&a.value, target, zapc_diag::ErrorCode::ZAP4021)?;
            let v = self.copy_for_pass(v, target, &a.value);
            argvals.push(v);
        }
        let thunk = self.spawn_thunk_for(sig_idx);
        let display = match callee {
            Expr::Name(n, _) => n.clone(),
            _ => "<function value>".to_string(),
        };
        self.emit(Inst::Spawn {
            func: thunk,
            args: argvals,
            owner,
            name: Some(display),
        });
        Ok(())
    }

    /// Generate (once per interned signature) the `spawn` thunk
    /// `$spawnthunk$<sig>(closure, args…)` whose body is a single indirect
    /// [`Inst::CallClosure`] on the captured closure record, then `Ret` (the
    /// result is discarded — `spawn` is fire-and-forget). The `CallClosure` has
    /// no landing pad so a raise propagates out of the thunk to the coroutine's
    /// trampoline, which records the failure on the owning scope (I#475). Returns
    /// the thunk's symbol, reused across every `spawn` of that signature.
    fn spawn_thunk_for(&self, sig_idx: u32) -> String {
        if let Some(w) = self.ctx.spawn_thunks.borrow().get(&sig_idx) {
            return w.clone();
        }
        let sig = self.ctx.fn_sigs.borrow().get(sig_idx).clone();
        let wname = format!("$spawnthunk${sig_idx}");
        let mut params_z = Vec::with_capacity(sig.params.len() + 1);
        params_z.push(Ty::Func(sig_idx));
        params_z.extend_from_slice(&sig.params);
        let mut lf = FnLower::new(
            wname.clone(),
            params_z,
            Ty::Void,
            false,
            false,
            self.sigs,
            self.ctx,
        );
        // ZIR value 0 is the closure record; values 1..=n are the call arguments.
        let callee: Value = 0;
        let argvals: Vec<Value> = (1..=sig.params.len() as u32).collect();
        lf.emit(Inst::CallClosure {
            dest: None,
            callee,
            args: argvals,
            params: sig.params.clone(),
            ret: sig.ret,
            unwind: None,
        });
        lf.set_term(Term::Ret(None));
        self.ctx.generated.borrow_mut().push(lf.func);
        self.ctx
            .spawn_thunks
            .borrow_mut()
            .insert(sig_idx, wname.clone());
        wname
    }

    /// Lower a `scope:` block (D-0044/D-0045, structured concurrency): a lexical
    /// region that owns every `spawn` inside it and joins — or, on failure/early
    /// exit, cancels then awaits — them at the dedent.
    ///
    /// Shape (mirrors `lower_try`'s landing-pad handling): a fresh scope handle is
    /// opened with `zap_scope_enter(on_error)`, pushed onto `scope_stack` so inner
    /// `spawn`s bind to it, and the body is lowered under a landing pad. On normal
    /// completion `zap_scope_exit` is the join barrier — it waits for all owned
    /// coroutines and re-raises their failure (a single exception, or an aggregate
    /// `ExceptionGroup` for several concurrent failures, D-0045). If the body
    /// itself raises (or leaves early by a `raise`), the landing pad calls
    /// `zap_scope_exit_cancel` to cancel and await the children, then re-raises the
    /// parent's in-flight exception (rule 4; the parent's failure wins over the
    /// children's, which are cancelled).
    fn lower_scope(&mut self, opts: &[ast::Arg], body: &ast::Block) -> Result<(), LowerError> {
        let on_error = self.scope_on_error(opts)?;
        let policy = self.iconst(Ty::I64, on_error);
        let handle = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(handle),
            func: "zap_scope_enter".to_string(),
            args: vec![policy],
        });

        if let Some(parent) = self.scope_stack.last().copied() {
            self.emit(Inst::Call {
                dest: None,
                func: "zap_scope_set_parent".to_string(),
                args: vec![handle, parent],
            });
        }
        let mut cached_grace = None;
        for option in opts {
            if option.name.as_deref() != Some("timeout") {
                continue;
            }
            // `timeout` is a nonnegative integer number of milliseconds (no unit
            // suffix; D-0094). Negative/overflow is rejected at runtime (ValueError).
            let (duration, ty) = self.expr(&option.value)?;
            if ty.int_width().is_none() {
                return Err(err(
                    "scope timeout requires an integer duration in milliseconds",
                ));
            }
            let duration = self.coerce(duration, ty, Ty::I64);
            let grace = match cached_grace {
                Some(value) => value,
                None => {
                    let value = match self.scope_grace(opts)? {
                        Some(value) => value,
                        None => self.iconst(Ty::I64, 0),
                    };
                    cached_grace = Some(value);
                    value
                }
            };
            self.throwing_call(None, "zap_scope_timeout", vec![handle, duration, grace]);
        }

        // `cancel_on=[Signal.X, ...]` + optional `grace=<ms>` (ticket #77,
        // D-0058): arm this scope so a listed signal cancels its tree, with an
        // optional hard-exit deadline. The subscription is removed inside
        // `zap_scope_exit`/`_cancel` (after the join), so it stays active while
        // the scope awaits its children.
        let cancel_mask = self.scope_cancel_mask(opts)?;
        if cancel_mask != 0 {
            let grace = match cached_grace {
                Some(value) => value,
                None => match self.scope_grace(opts)? {
                    Some(value) => value,
                    None => self.iconst(Ty::I64, 0),
                },
            };
            let maskv = self.iconst(Ty::I64, cancel_mask);
            self.emit(Inst::Call {
                dest: None,
                func: "zap_scope_cancel_on".to_string(),
                args: vec![handle, maskv, grace],
            });
        }

        let lp = self.new_block();
        let after = self.new_block();

        // --- owned region: a raise inside unwinds to `lp` (cancel + re-raise) ---
        self.landing_pads.push(lp);
        self.scope_stack.push(handle);
        self.cleanups.push(Cleanup::ScopeCancel(handle));
        self.push_scope();
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.pop_scope();
        self.scope_stack.pop();
        self.cleanups.pop();
        self.landing_pads.pop();

        // Normal completion: the join barrier. `zap_scope_exit` awaits the owned
        // coroutines and re-raises their failure, so it is a throwing call (an
        // aggregate may unwind to an enclosing `try`).
        if !self.terminated() {
            self.throwing_call(None, "zap_scope_exit", vec![handle]);
            self.ensure_br(after);
        }

        // If the body emitted no throwing call it cannot raise, so the cancel
        // landing pad is dead (LLVM requires a `landingpad` to have an `invoke`
        // predecessor) — skip it.
        if !self.used_lps.contains(&lp) {
            self.position(after);
            return Ok(());
        }

        // --- cancel landing pad: the body raised; cancel + await children, then
        // re-raise (rule 4). `zap_scope_exit_cancel_exc` returns which exception
        // surfaces: a recorded child failure outranks a block-level `ScopeTimeout`
        // or `Cancelled`, and when *this* scope's own deadline cancelled the body
        // the caller sees `ScopeTimeout` rather than the raw `Cancelled` (I#475).
        self.position(lp);
        let exc = self.new_value(Ty::Ptr);
        self.emit(Inst::LandingPad { dest: exc });
        let surfaced = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(surfaced),
            func: "zap_scope_exit_cancel_exc".to_string(),
            args: vec![handle, exc],
        });
        if !self.terminated() {
            self.emit_end_catch();
            self.throwing_call(None, "zap_raise", vec![surfaced]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
        }

        self.position(after);
        Ok(())
    }

    /// The `on_error` policy constant for a `scope(...)` (D-0044): `0` = Cancel
    /// (the default — a failure cancels siblings), `1` = Wait (a failure waits for
    /// siblings instead of cancelling). Validated in sema; re-checked here so a
    /// bad option is a lowering error rather than a silent default.
    fn scope_on_error(&self, opts: &[ast::Arg]) -> Result<i64, LowerError> {
        let mut policy = 0; // OnError.Cancel
        for a in opts {
            match a.name.as_deref() {
                Some("on_error") => {
                    if let Expr::Attr { base, name, .. } = &a.value {
                        if matches!(base.as_ref(), Expr::Name(n, _) if n == "OnError") {
                            policy = match name.as_str() {
                                "Cancel" => 0,
                                "Wait" => 1,
                                _ => {
                                    return Err(err(
                                        "scope on_error expects OnError.Cancel or OnError.Wait",
                                    ))
                                }
                            };
                            continue;
                        }
                    }
                    return Err(err("scope on_error expects OnError.Cancel or OnError.Wait"));
                }
                Some("timeout") => continue,
                // Handled separately by `scope_cancel_on` (ticket #77).
                Some("cancel_on") | Some("grace") => continue,
                Some(other) => return Err(err(format!("scope() has no option '{other}'"))),
                None => return Err(err("scope() options must be keyword arguments")),
            }
        }
        Ok(policy)
    }

    /// The `cancel_on` signal bitmask for a `scope(cancel_on=[Signal.X, ...])`
    /// (ticket #77, D-0058): bit N set for each listed signal number. Returns 0
    /// when the scope has no `cancel_on`. The `Signal` members are tier-1
    /// discriminants that ARE the OS signal numbers.
    fn scope_cancel_mask(&self, opts: &[ast::Arg]) -> Result<i64, LowerError> {
        let mut mask: i64 = 0;
        for a in opts {
            if a.name.as_deref() != Some("cancel_on") {
                continue;
            }
            let Expr::List(elems, _) = &a.value else {
                return Err(err(
                    "scope cancel_on expects a list, e.g. cancel_on=[Signal.SIGINT]",
                ));
            };
            for e in elems {
                let signo = match e {
                    Expr::Attr { base, name, .. } if matches!(base.as_ref(), Expr::Name(n, _) if n == "Signal") => {
                        self.ctx
                            .enums
                            .get("Signal")
                            .and_then(|en| en.member_value(name))
                            .ok_or_else(|| err(format!("unknown Signal member '{name}'")))?
                    }
                    _ => {
                        return Err(err(
                            "scope cancel_on expects Signal members, e.g. Signal.SIGINT",
                        ))
                    }
                };
                if (1..63).contains(&signo) {
                    mask |= 1i64 << signo;
                }
            }
        }
        Ok(mask)
    }

    /// The `grace=<ms>` hard-exit deadline expression for a `scope(...)`, lowered
    /// to an i64 value (ticket #77); `None` when absent (no deadline).
    fn scope_grace(&mut self, opts: &[ast::Arg]) -> Result<Option<Value>, LowerError> {
        for a in opts {
            if a.name.as_deref() == Some("grace") {
                let (v, vt) = self.expr(&a.value)?;
                return Ok(Some(self.coerce(v, vt, Ty::I64)));
            }
        }
        Ok(None)
    }

    /// Balance the `__cxa_begin_catch` performed on landing-pad entry. Called on
    /// each path that leaves a handler normally or re-raises.
    pub(super) fn emit_end_catch(&mut self) {
        self.emit(Inst::Call {
            dest: None,
            func: "zap_eh_end_catch".to_string(),
            args: Vec::new(),
        });
    }

    /// `raise EXPR` begins unwinding with a typed exception value. Raising a
    /// string uses the compatibility `zap_panic` entry, which wraps it in the
    /// base `Exception` type.
    fn lower_raise(
        &mut self,
        value: Option<&Expr>,
        cause: Option<&Expr>,
    ) -> Result<(), LowerError> {
        let Some(e) = value else {
            // Bare `raise` (#657, D-0116): re-raise the exception the
            // dynamically-enclosing `except` handler is handling, re-throwing
            // the *same* in-flight object so its type and identity are
            // preserved (traceback fidelity rides on the I#475 green-stack
            // unwinder and is a follow-up). A `from <cause>` is syntactically
            // impossible without a value, so there is none to attach.
            let Some(&exc) = self.active_exc.last() else {
                return Err(err_code(
                    zapc_diag::ErrorCode::ZAP6002,
                    "bare 'raise' is only valid inside an 'except' handler, \
                     where it re-raises the exception being handled; there is \
                     no active exception to re-raise here",
                ));
            };
            self.throwing_call(None, "zap_raise", vec![exc]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
            return Ok(());
        };
        // `raise StopIteration` / `raise StopIteration()` (the Python iterator
        // end-of-iteration signal, D-0048) lowers to a panic carrying a sentinel
        // message the iteration-protocol landing pad recognizes.
        if is_stop_iteration(e) {
            let msg = self.new_value(Ty::Ptr);
            self.emit(Inst::StrConst {
                dest: msg,
                bytes: b"StopIteration".to_vec(),
            });
            let ty = self.iconst(Ty::I64, 11);
            let exc = self.new_value(Ty::Ptr);
            self.emit(Inst::Call {
                dest: Some(exc),
                func: "zap_exception_new".to_string(),
                args: vec![ty, msg],
            });
            return self.finish_raise(exc, cause);
        }
        if let Expr::Call { callee, args, .. } = e {
            if let Expr::Name(name, _) = callee.as_ref() {
                let custom_exception = self.ctx.ids.contains_key(name)
                    && !self.ctx.enums.contains_key(name)
                    && !self.sigs.contains_key(name);
                // A package-defined `@exception` struct (#1432,
                // decisions/lang/1432-exception-fields.md). Construct the real
                // struct instance with the ordinary constructor machinery
                // (positional + keyword arguments, declared defaults, and the
                // `@validate`/`@range` field checks), so its declared fields —
                // of any type, including `Optional[T]` — are preserved. Derive
                // the exception's message (for `str(e)` and the uncaught
                // diagnostic) from the struct's message field, then attach the
                // instance to the exception object as a typed payload that a
                // later `except E as e` reads fields off (`e.error`,
                // `e.status`) without parsing the message. A raise with a
                // single positional message keeps working — it simply sets the
                // one field — so every #1278 message-only exception is
                // unchanged. A built-in exception name keeps the dedicated
                // message-only path below.
                if custom_exception {
                    if let Some(&sid) = self.ctx.ids.get(name) {
                        let type_id = custom_exception_type_id(name);
                        let (payload, _) = self.construct(sid, args)?;
                        let message = self.exception_message_value(sid, payload);
                        let id = self.iconst(Ty::I64, type_id);
                        let exc = self.new_value(Ty::Ptr);
                        self.emit(Inst::Call {
                            dest: Some(exc),
                            func: "zap_exception_new".to_string(),
                            args: vec![id, message],
                        });
                        self.emit(Inst::Call {
                            dest: None,
                            func: "zap_exception_set_payload".to_string(),
                            args: vec![exc, payload],
                        });
                        return self.finish_raise(exc, cause);
                    }
                }
                if let Some(type_id) = exception_type_id_name(name) {
                    if args.len() != 1 || args[0].name.is_some() {
                        return Err(err(format!("{name}() takes one positional message")));
                    }
                    let (message, ty) = self.expr(&args[0].value)?;
                    if ty != Ty::Ptr {
                        return Err(err(format!("{name}() message must be a str")));
                    }
                    let id = self.iconst(Ty::I64, type_id);
                    let exc = self.new_value(Ty::Ptr);
                    self.emit(Inst::Call {
                        dest: Some(exc),
                        func: "zap_exception_new".to_string(),
                        args: vec![id, message],
                    });
                    return self.finish_raise(exc, cause);
                }
            }
        }
        let (v, vt) = self.expr(e)?;
        if vt != Ty::Ptr {
            return Err(err(
                "raise currently expects a str value (the exception message)",
            ));
        }
        // `raise "message"` with no `from` keeps the legacy `zap_panic` shape
        // (the runtime wraps the string in a base `Exception`). A `from <cause>`
        // needs a concrete exception object to carry the cause slot, so build
        // one explicitly and route it through the shared raise path.
        if cause.is_none() {
            self.throwing_call(None, "zap_panic", vec![v]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
            return Ok(());
        }
        let id = self.iconst(Ty::I64, 2); // ZAP_EXC_EXCEPTION
        let exc = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(exc),
            func: "zap_exception_new".to_string(),
            args: vec![id, v],
        });
        self.finish_raise(exc, cause)
    }

    /// Attach the optional `from <cause>` chain to the exception object `exc`
    /// (#604, D-0103), then begin unwinding with it. The cause is stored in the
    /// exception's `cause` slot so the uncaught renderer can print the cause's
    /// own section above the new one; it is also readable from Zap as
    /// `<caught>.cause`.
    fn finish_raise(&mut self, exc: Value, cause: Option<&Expr>) -> Result<(), LowerError> {
        if let Some(c) = cause {
            let cobj = self.lower_cause(c)?;
            self.emit(Inst::Call {
                dest: None,
                func: "zap_exception_set_cause".to_string(),
                args: vec![exc, cobj],
            });
        }
        self.throwing_call(None, "zap_raise", vec![exc]);
        if !self.terminated() {
            self.set_term(Term::Unreachable);
        }
        Ok(())
    }

    /// Derive a package-defined exception's message string (#1432) from its
    /// already-constructed struct instance `payload` of type `sid`. The message
    /// is the value of a declared `str message` field when one exists (the
    /// convention every #1278 exception already follows), else the struct's
    /// first `str` field, else the empty string. This string populates the
    /// exception object's `message` slot, so `str(e)`, `print(e)`, and the
    /// uncaught diagnostic are identical to a message-only raise — the typed
    /// fields are additive and read off the payload, never the message.
    fn exception_message_value(&mut self, sid: u32, payload: Value) -> Value {
        let fields = &self.ctx.fields[sid as usize];
        let msg_idx = fields
            .iter()
            .position(|(n, t)| n == "message" && *t == Ty::Ptr)
            .or_else(|| fields.iter().position(|(_, t)| *t == Ty::Ptr));
        match msg_idx {
            Some(idx) => {
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::FieldGet {
                    dest: d,
                    base: payload,
                    sid,
                    idx: idx as u32,
                });
                d
            }
            None => {
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::StrConst {
                    dest: d,
                    bytes: Vec::new(),
                });
                d
            }
        }
    }

    /// Evaluate a `from <cause>` operand to an exception object. When the cause
    /// names a caught exception binding, its real in-flight exception object is
    /// reused (preserving the cause's own type id and chain); otherwise the
    /// operand must be a str message, which is wrapped in a base `Exception`.
    fn lower_cause(&mut self, cause: &Expr) -> Result<Value, LowerError> {
        if let Expr::Name(n, _) = cause {
            if let Some((obj_slot, _)) = self.lookup(&exc_obj_key(n)) {
                return Ok(self.load(obj_slot, Ty::Ptr));
            }
        }
        let (cv, ct) = self.expr(cause)?;
        if ct != Ty::Ptr {
            return Err(err(
                "raise ... from <cause>: the cause must be a caught exception or a str message",
            ));
        }
        let id = self.iconst(Ty::I64, 2); // ZAP_EXC_EXCEPTION
        let obj = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(obj),
            func: "zap_exception_new".to_string(),
            args: vec![id, cv],
        });
        Ok(obj)
    }

    /// `assert cond [, msg]` (D-0054). Always evaluated in v1: the condition is
    /// tested, and on failure a *power-assert* message — the condition's source
    /// text plus its operand values — is built and raised through the ordinary
    /// `zap_panic` path (so an enclosing `try`/`except`, and the `zap test`
    /// runner, catch it like any other exception).
    ///
    /// For a top-level comparison (`a == b`, `a < b`, …) each operand is
    /// evaluated exactly once and its value retained, so the diagnostic can show
    /// both sides without re-evaluating (no double side effects). Other
    /// conditions fall back to showing the whole expression's truthiness.
    fn lower_assert(
        &mut self,
        cond: &Expr,
        msg: Option<&Expr>,
        cond_src: &str,
    ) -> Result<(), LowerError> {
        // Decompose a comparison so both operand values can be reported.
        //
        // A `== None` / `!= None` comparison (#1184) is NOT decomposed: a bare
        // `None` operand has no standalone lowering (it only exists as the
        // null/empty-variant check intercepted in the `Expr::Binary` expr
        // lowerer), so evaluating it here with `self.expr` would bail with
        // ZAP6001. It falls through to the whole-expression path below, which
        // lowers the null check (`lower_is_none` → a real `IsNull` test) and
        // takes its truthiness — exactly as `assert x is None` already does.
        // The power-assert operand values ("left/right") are dropped for a null
        // check, where "right: None" would carry no information anyway.
        let decomposed = match cond {
            Expr::Binary { op, lhs, rhs, .. }
                if is_power_comparison(*op)
                    && !matches!(lhs.as_ref(), Expr::None(_))
                    && !matches!(rhs.as_ref(), Expr::None(_)) =>
            {
                let (lv, lt) = self.expr(lhs)?;
                let (rv, rt) = self.expr(rhs)?;
                let (bit, _) = self.binop(*op, lv, lt, rv, rt)?;
                Some((bit, (lv, lt), (rv, rt)))
            }
            _ => None,
        };
        let bit = match decomposed {
            Some((bit, ..)) => bit,
            None => {
                let (cv, ct) = self.expr(cond)?;
                self.truthy(cv, ct)
            }
        };

        let fail = self.new_block();
        let after = self.new_block();
        self.set_term(Term::BrIf {
            cond: bit,
            then_bb: after,
            else_bb: fail,
        });

        // --- failure path: build the message and raise it ---
        self.position(fail);
        let mut acc = self.strconst("assertion failed");
        if let Some(m) = msg {
            let (mv, mt) = self.expr(m)?;
            // A str message is used verbatim; anything else is stringified.
            let ms = if mt == Ty::Ptr {
                mv
            } else {
                self.value_to_str(mv, mt)
            };
            let sep = self.strconst(": ");
            acc = gen_concat(self, acc, sep);
            acc = gen_concat(self, acc, ms);
        }
        let srcline = self.strconst(&format!("\n  assert {cond_src}"));
        acc = gen_concat(self, acc, srcline);
        if let Some((_, (lv, lt), (rv, rt))) = decomposed {
            let ls = self.value_to_str(lv, lt);
            let rs = self.value_to_str(rv, rt);
            let lpfx = self.strconst("\n  left:  ");
            acc = gen_concat(self, acc, lpfx);
            acc = gen_concat(self, acc, ls);
            let rpfx = self.strconst("\n  right: ");
            acc = gen_concat(self, acc, rpfx);
            acc = gen_concat(self, acc, rs);
        }
        // `assert` raises a catchable AssertionError (#374/B7), so `except
        // AssertionError` resolves and the power-assert text is its message.
        let assertion_error = self.iconst(Ty::I64, 16); // ZAP_EXC_ASSERTION_ERROR (zaprt.c)
        self.throwing_call(None, "zap_panic_typed", vec![assertion_error, acc]);
        if !self.terminated() {
            self.set_term(Term::Unreachable);
        }
        self.position(after);
        Ok(())
    }

    /// Best-effort stringification of a value for power-assert diagnostics.
    /// Returns a `Ptr` (ZapStr). Never fails: an unsupported type renders as a
    /// `<type>` placeholder so `assert` always lowers.
    fn value_to_str(&mut self, v: Value, t: Ty) -> Value {
        match t {
            // Quote strings so empty/whitespace values are visible.
            Ty::Ptr => {
                let q = self.strconst("\"");
                let a = gen_concat(self, q, v);
                let q2 = self.strconst("\"");
                gen_concat(self, a, q2)
            }
            Ty::I1 => {
                let i64v = self.coerce(v, Ty::I1, Ty::I64);
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_bool_to_str".to_string(),
                    args: vec![i64v],
                });
                d
            }
            t if t.int_width().is_some() => {
                let i64v = self.coerce(v, t, Ty::I64);
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_i64_to_str".to_string(),
                    args: vec![i64v],
                });
                d
            }
            t if t.is_float() => {
                let f64v = self.coerce(v, t, Ty::F64);
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_f64_to_str".to_string(),
                    args: vec![f64v],
                });
                d
            }
            Ty::Struct(sid) => {
                // Use the struct's own `__str__`/`__repr__` when it has one;
                // otherwise fall back to a type-name placeholder (never fail).
                for dunder in ["__str__", "__repr__"] {
                    let mangled = format!("{}__{}", self.ctx.names[sid as usize], dunder);
                    if self.sigs.contains_key(&mangled) {
                        let d = self.new_value(Ty::Ptr);
                        self.throwing_call(Some(d), &mangled, vec![v]);
                        return d;
                    }
                }
                self.strconst(&format!("<{}>", self.ctx.names[sid as usize]))
            }
            _ => self.strconst("<value>"),
        }
    }

    /// `with items: BODY` (D-0027). Only the `with expect_raises(ErrType):`
    /// exception-assertion form is supported (D-0054): BODY must raise, or the
    /// `with` itself fails. The general context-manager protocol
    /// (`__enter__`/`__exit__`) is a documented follow-up.
    ///
    /// Desugars to the equivalent `try`/flag/`raise`:
    /// ```text
    /// int __er_N = 0
    /// try: BODY
    /// except: __er_N = 1
    /// if __er_N == 0: raise "expected <ErrType> ... but none was raised"
    /// ```
    /// so only a matching raise inside BODY is swallowed and a missing or
    /// mismatched one propagates (and is caught by an enclosing test).
    /// `with ITEM { , ITEM } : BODY` (D-0027, D-0069). The test-only
    /// `with expect_raises(ErrType):` (D-0054) keeps its bespoke desugaring;
    /// every other form drives the general `__enter__`/`__exit__` context-manager
    /// protocol (I#185). Multiple managers nest left-to-right (Python).
    fn lower_with(&mut self, items: &[ast::WithItem], body: &ast::Block) -> Result<(), LowerError> {
        if items.len() == 1 {
            if let Expr::Call { callee, .. } = &items[0].expr {
                if is_name(callee, "expect_raises") {
                    return self.lower_expect_raises(&items[0].expr, body);
                }
            }
        }
        self.push_scope();
        let r = self.lower_with_items(items, body, 0);
        self.pop_scope();
        r
    }

    /// The context-manager protocol for one `with` item (I#185, D-0069):
    /// evaluate the manager once, call `__enter__` (binding `as NAME` to its
    /// result), lower the rest (the next item, or `BODY` for the last), then run
    /// `__exit__` on every exit path — normal fall-through, panic/unwind (via a
    /// landing pad that re-raises, chaining to any enclosing manager), and the
    /// structured early exits (via the cleanup stack walked by
    /// `return`/`break`/`continue`).
    fn lower_with_items(
        &mut self,
        items: &[ast::WithItem],
        body: &ast::Block,
        idx: usize,
    ) -> Result<(), LowerError> {
        if idx == items.len() {
            for st in &body.stmts {
                self.stmt(st)?;
            }
            return Ok(());
        }
        let item = &items[idx];
        // Arena (#553, D-0158): `with arena(size=?, cap=?) as a:` lowers to the
        // runtime region primitive, not the generic `__enter__`/`__exit__` protocol
        // — the handle is `zap_arena_open(size, cap)`, and the deterministic
        // block-exit bulk free (`zap_arena_close`) rides the same cleanup +
        // landing-pad machinery as any `with` (normal / unwind / early-exit paths).
        if is_arena_call(&item.expr) {
            if let Some(&arena_sid) = self.ctx.ids.get("arena") {
                return self.lower_arena_with(items, body, idx, arena_sid);
            }
        }
        let (mgr, mty) = self.expr(&item.expr)?;
        let Ty::Struct(sid) = mty else {
            return Err(err(
                "`with` manager must be a value implementing the context-manager \
                 protocol (`__enter__`/`__exit__`)",
            ));
        };
        let name = self.ctx.names[sid as usize].clone();
        let enter_m = format!("{}__{}", name, "__enter__");
        let exit_m = format!("{}__{}", name, "__exit__");
        let Some((_p, enter_ret)) = self.sigs.get(&enter_m).cloned() else {
            return Err(err(format!(
                "type '{name}' is not a context manager: define '__enter__(self)' \
                 and '__exit__(self)' (I#185)"
            )));
        };
        if !self.sigs.contains_key(&exit_m) {
            return Err(err(format!(
                "context manager '{name}' defines '__enter__' but not '__exit__' (I#185)"
            )));
        }

        // `@scoped` owned-field teardown (#876, D-0159): the owner's `__exit__`
        // runs first, then each owned `@scoped` field's `__exit__` in reverse
        // declaration order. Empty for every manager that owns no `@scoped` field.
        let field_exits = self.ctx.scoped_field_exits(sid);

        // `NAME = mgr.__enter__()` on entry (result discarded without `as`).
        let entered = self.new_value(enter_ret);
        self.throwing_call(Some(entered), &enter_m, vec![mgr]);
        if let Some(alias) = &item.alias {
            let slot = self.alloca(enter_ret);
            self.store(slot, entered);
            self.declare(alias, slot, enter_ret);
        }

        let lp = self.new_block();
        let after = self.new_block();

        // Body: calls unwind to `lp`; an early exit runs `__exit__` via the stack.
        self.landing_pads.push(lp);
        self.cleanups.push(Cleanup::WithExit {
            mgr,
            mgr_sid: sid,
            exit_fn: exit_m.clone(),
            field_exits: field_exits.clone(),
        });
        self.push_scope();
        self.lower_with_items(items, body, idx + 1)?;
        self.pop_scope();
        self.cleanups.pop();
        self.landing_pads.pop();

        // Normal fall-through: run `__exit__`, the `@scoped` field drop-glue, then
        // continue past the `with`.
        if !self.terminated() {
            self.throwing_call(None, &exit_m, vec![mgr]);
            self.emit_scoped_field_teardown(mgr, sid, &field_exits);
            self.ensure_br(after);
        }

        // Body could not raise → no landing pad needed.
        if !self.used_lps.contains(&lp) {
            self.position(after);
            return Ok(());
        }

        // Unwind path: run `__exit__`, then re-raise. With `lp` already popped,
        // the re-raise (and an `__exit__` that itself raises) routes to the
        // enclosing manager/`try`, so cleanup chains on exceptions.
        self.position(lp);
        let exc = self.new_value(Ty::Ptr);
        self.emit(Inst::LandingPad { dest: exc });
        self.emit_end_catch();
        self.throwing_call(None, &exit_m, vec![mgr]);
        self.emit_scoped_field_teardown(mgr, sid, &field_exits);
        if !self.terminated() {
            self.throwing_call(None, "zap_raise", vec![exc]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
        }
        self.position(after);
        Ok(())
    }

    /// Lower a `with arena(size=?, cap=?) as a:` item (#553, D-0158). Mirrors
    /// `lower_with_items`' exit-path structure (normal fall-through, exception
    /// unwind, structured early-exit via the cleanup stack), but the manager is the
    /// runtime arena: entry is `zap_arena_open(size, cap)` and teardown is the
    /// deterministic bulk free `zap_arena_close(handle)` (a `Cleanup::ArenaClose`).
    /// The handle is bound as `Ty::Struct(arena_sid)` so an `a.alloc`/`a.new`
    /// method call dispatches through the arena receiver path; the underlying value
    /// is the opaque `ZapArena*`, which the precise marker ignores (not in the heap
    /// registry).
    fn lower_arena_with(
        &mut self,
        items: &[ast::WithItem],
        body: &ast::Block,
        idx: usize,
        arena_sid: u32,
    ) -> Result<(), LowerError> {
        let (size, cap) = self.lower_arena_open_args(&items[idx].expr)?;
        let aty = Ty::Struct(arena_sid);
        let handle = self.new_value(aty);
        self.emit(Inst::Call {
            dest: Some(handle),
            func: "zap_arena_open".to_string(),
            args: vec![size, cap],
        });
        // A NULL open (runtime `size > cap`, or heap OOM reserving the hint) raises
        // before the block is entered; no arena exists yet to tear down, so this
        // guard precedes the cleanup/landing-pad setup below.
        self.arena_oom_guard(handle);
        if let Some(alias) = &items[idx].alias {
            let alias = alias.clone();
            let slot = self.alloca(aty);
            self.store(slot, handle);
            self.declare(&alias, slot, aty);
        }

        let lp = self.new_block();
        let after = self.new_block();
        self.landing_pads.push(lp);
        self.cleanups.push(Cleanup::ArenaClose(handle));
        self.push_scope();
        self.lower_with_items(items, body, idx + 1)?;
        self.pop_scope();
        self.cleanups.pop();
        self.landing_pads.pop();

        // Normal fall-through: bulk-free, then continue past the `with`.
        if !self.terminated() {
            self.emit(Inst::Call {
                dest: None,
                func: "zap_arena_close".to_string(),
                args: vec![handle],
            });
            self.ensure_br(after);
        }

        // Body could not raise → no landing pad needed.
        if !self.used_lps.contains(&lp) {
            self.position(after);
            return Ok(());
        }

        // Unwind path: bulk-free, then re-raise (chaining to the enclosing
        // manager/`try`, `lp` already popped).
        self.position(lp);
        let exc = self.new_value(Ty::Ptr);
        self.emit(Inst::LandingPad { dest: exc });
        self.emit_end_catch();
        self.emit(Inst::Call {
            dest: None,
            func: "zap_arena_close".to_string(),
            args: vec![handle],
        });
        if !self.terminated() {
            self.throwing_call(None, "zap_raise", vec![exc]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
        }
        self.position(after);
        Ok(())
    }

    /// Lower the `size` / `cap` arguments of an `arena(...)` head to two `i64`
    /// values, each defaulting to `0` (0 size = one region; 0 cap = unbounded).
    /// Accepts them positionally or by keyword; unknown keywords are ignored (sema
    /// has already validated the call).
    fn lower_arena_open_args(&mut self, e: &Expr) -> Result<(Value, Value), LowerError> {
        let mut size: Option<Value> = None;
        let mut cap: Option<Value> = None;
        if let Expr::Call { args, .. } = e {
            let mut positional = 0usize;
            for a in args {
                let (v, _) = self.expr(&a.value)?;
                match a.name.as_deref() {
                    Some("size") => size = Some(v),
                    Some("cap") => cap = Some(v),
                    Some(_) => {}
                    None => {
                        if positional == 0 {
                            size = Some(v);
                        } else if positional == 1 {
                            cap = Some(v);
                        }
                        positional += 1;
                    }
                }
            }
        }
        let size = size.unwrap_or_else(|| self.iconst(Ty::I64, 0));
        let cap = cap.unwrap_or_else(|| self.iconst(Ty::I64, 0));
        Ok((size, cap))
    }

    /// `with expect_raises(ErrType):` (D-0054) — the test-framework assertion
    /// that `BODY` raises. Desugars to a flag + `try/except` + post-check,
    /// reusing the existing try/if/raise lowering.
    fn lower_expect_raises(&mut self, call: &Expr, body: &ast::Block) -> Result<(), LowerError> {
        let Expr::Call { args, .. } = call else {
            return Err(err("expect_raises must be called"));
        };
        let ty_name = args
            .first()
            .map(|a| ast::render_expr(&a.value))
            .unwrap_or_else(|| "an exception".to_string());
        let caught_ty = args.first().and_then(|a| match &a.value {
            Expr::Name(name, span) => Some(ast::Type::Named {
                name: name.clone(),
                args: Vec::new(),
                span: *span,
            }),
            _ => None,
        });

        // Build the desugared statements as AST and lower them, reusing the
        // existing try / if / raise lowering.
        let sp = body.span;
        let flag = format!("__er_{}", self.new_with_id());
        let miss = format!("expected {ty_name} to be raised, but the with-body returned normally");

        let decl = Stmt::Let {
            is_const: false,
            ty: None,
            name: flag.clone(),
            value: Expr::Int("0".to_string(), sp),
            span: sp,
        };
        let set_flag = Stmt::Assign {
            target: Expr::Name(flag.clone(), sp),
            value: Expr::Int("1".to_string(), sp),
            span: sp,
        };
        let try_stmt = Stmt::Try {
            body: body.clone(),
            excepts: vec![ast::ExceptClause {
                ty: caught_ty,
                binding: None,
                body: ast::Block {
                    stmts: vec![set_flag],
                    span: sp,
                },
                is_star: false,
                span: sp,
            }],
            finally: None,
            span: sp,
        };
        let check = Stmt::If {
            branches: vec![(
                Expr::Binary {
                    op: AstBin::Eq,
                    lhs: Box::new(Expr::Name(flag.clone(), sp)),
                    rhs: Box::new(Expr::Int("0".to_string(), sp)),
                    span: sp,
                },
                ast::Block {
                    stmts: vec![Stmt::Raise {
                        value: Some(Expr::Str {
                            value: miss,
                            raw: true,
                            bytes: false,
                            fstring: false,
                            span: sp,
                            fstring_fields: Vec::new(),
                        }),
                        cause: None,
                        span: sp,
                    }],
                    span: sp,
                },
            )],
            else_block: None,
            span: sp,
        };

        self.push_scope();
        self.stmt(&decl)?;
        self.stmt(&try_stmt)?;
        self.stmt(&check)?;
        self.pop_scope();
        Ok(())
    }

    /// A monotonically increasing id so each `with` desugaring uses a distinct
    /// flag variable (safe even when `with`s nest).
    fn new_with_id(&mut self) -> u32 {
        let id = self.with_counter;
        self.with_counter += 1;
        id
    }

    /// `try: BODY except [Type [as e]]: HANDLER finally: FIN` with typed,
    /// source-ordered dispatch. Calls in BODY become `invoke`s whose unwind edge
    /// targets a landing-pad block. Every clause gets a chance to match before
    /// the exception is propagated; `finally` runs on normal, handled, and
    /// unmatched paths.
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
        // Register `finally` as a cleanup so an early `return`/`break`/`continue`
        // inside the body runs it before jumping (I#185, D-0069 — the gap I#22
        // deferred). The unwind and normal paths emit `finally` explicitly below.
        let has_finally_cleanup = finally.is_some();
        if let Some(f) = finally {
            self.cleanups.push(Cleanup::Finally(f.clone()));
        }
        self.push_scope();
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.pop_scope();
        if has_finally_cleanup {
            self.cleanups.pop();
        }
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

        // `except*` group handling (#605, D-0129) takes a distinct path: every
        // matching clause runs (unlike plain `except`, where only the first
        // match runs), each peeling its sub-exceptions out of the in-flight
        // group, and the unmatched remainder auto-re-raises after all clauses.
        // sema has already enforced that the clauses are all-`except*` (so this
        // branch covers every clause) and that each carries a type.
        if excepts.first().is_some_and(|c| c.is_star) {
            return self.lower_try_star(exc, excepts, finally, after);
        }

        let mut has_catch_all = false;
        for clause in excepts {
            let matched = self.new_block();
            let type_id = clause.ty.as_ref().and_then(exception_type_id);
            let next = type_id.map(|_| self.new_block());
            if let Some(type_id) = type_id {
                let expected = self.iconst(Ty::I64, type_id);
                let is_a = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(is_a),
                    func: "zap_exception_is_a".to_string(),
                    args: vec![exc, expected],
                });
                let cond = self.new_value(Ty::I1);
                let zero = self.iconst(Ty::I64, 0);
                self.emit(Inst::ICmp {
                    dest: cond,
                    pred: CmpPred::Ne,
                    lhs: is_a,
                    rhs: zero,
                });
                self.set_term(Term::BrIf {
                    cond,
                    then_bb: matched,
                    else_bb: next.unwrap(),
                });
            } else {
                self.set_term(Term::Br(matched));
                has_catch_all = true;
            }

            self.position(matched);
            self.push_scope();
            if let Some(bind) = &clause.binding {
                let message = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(message),
                    func: "zap_exception_message".to_string(),
                    args: vec![exc],
                });
                let slot = self.alloca(Ty::Ptr);
                self.store(slot, message);
                self.declare(bind, slot, Ty::Ptr);
                // Stash the exception *object* under a scoped, non-typable key so
                // `e.cause` and `raise ... from e` can reach the real exception
                // (its type id and chain), not just the message string (#604).
                let obj_slot = self.alloca(Ty::Ptr);
                self.store(obj_slot, exc);
                self.declare(&exc_obj_key(bind), obj_slot, Ty::Ptr);
                // If this clause catches a package-defined `@exception` struct
                // (#1432), record its struct id against the stashed object slot
                // so `e.<field>` reads declared fields off the exception's
                // payload (the struct instance built at the raise site). A
                // built-in / message-only / catch-all clause records nothing, so
                // `e` stays the plain message string. The typed fields are those
                // of the clause's static type; for a concrete catch (every
                // direct catch, incl. the OAuth use case) the payload is exactly
                // that type, so the read is exact.
                if let Some(ast::Type::Named { name, args, .. }) = &clause.ty {
                    if args.is_empty() && !self.ctx.enums.contains_key(name) {
                        if let Some(&sid) = self.ctx.ids.get(name) {
                            self.exc_payload_sid.insert(obj_slot, sid);
                        }
                    }
                }
            }
            let handler_lp = self.new_block();
            self.landing_pads.push(handler_lp);
            let cleanup_depth = self.cleanups.len();
            if let Some(f) = finally {
                self.cleanups.push(Cleanup::Finally(f.clone()));
            }
            self.cleanups.push(Cleanup::EndCatch);
            // Make the caught exception the active one for the duration of this
            // handler body, so a bare `raise` (re-raise, #657) re-throws it.
            // Nested constructs lowered inline (an inner `try`/`finally`) keep
            // this entry visible, matching Python's dynamic "currently handled
            // exception"; a sibling `else`/`finally` is lowered outside this
            // window, where the stack is already popped.
            self.active_exc.push(exc);
            for st in &clause.body.stmts {
                self.stmt(st)?;
            }
            self.active_exc.pop();
            self.landing_pads.pop();
            let handler_throws = self.used_lps.contains(&handler_lp);
            self.cleanups.truncate(cleanup_depth);
            self.pop_scope();
            if !self.terminated() {
                self.emit_end_catch();
                if let Some(f) = finally {
                    self.lower_block(f)?;
                }
                self.ensure_br(after);
            }
            if handler_throws {
                // A new exception raised by the handler replaces the caught one,
                // but `finally` still runs. Close both active C++ catches before
                // executing cleanup so a failure in `finally` propagates cleanly.
                self.position(handler_lp);
                let replacement = self.new_value(Ty::Ptr);
                self.emit(Inst::LandingPad { dest: replacement });
                self.emit_end_catch();
                self.emit_end_catch();
                if let Some(f) = finally {
                    self.lower_block(f)?;
                }
                if !self.terminated() {
                    self.throwing_call(None, "zap_raise", vec![replacement]);
                    if !self.terminated() {
                        self.set_term(Term::Unreachable);
                    }
                }
            }
            if has_catch_all {
                break;
            }
            self.position(next.unwrap());
        }

        // A finally-only try, or an exception for which no typed clause matched:
        // run cleanup and rethrow the same typed exception value.
        if !has_catch_all {
            self.emit_end_catch();
            if let Some(f) = finally {
                self.lower_block(f)?;
            }
            self.throwing_call(None, "zap_raise", vec![exc]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
        }

        self.position(after);
        Ok(())
    }

    /// Lower the `except*` clauses of a `try` (#605, D-0129). The landing pad has
    /// already run `begin_catch`, binding the in-flight exception object to
    /// `exc`. Unlike plain `except` (first-match-only), **every** `except*`
    /// clause whose type matches some sub-exception runs: each peels its matches
    /// out of a shared mutable "remaining" working set into a sub-group, binds
    /// its `as NAME` to that group, and runs its handler; control then falls to
    /// the *next* clause (not out of the `try`). After the last clause, whatever
    /// is left in `remaining` auto-re-raises — the single remaining exception
    /// bare (so a plain enclosing `except E` catches it, keeping Zap's
    /// bare-for-single rule), or an `ExceptionGroup` when two or more remain.
    ///
    /// The one `begin_catch` is balanced by exactly one `end_catch` on the way
    /// out, on both the re-raise and the clean-exit edge (and by the per-clause
    /// `Cleanup::EndCatch` on any early `return`/`break`/`raise` inside a
    /// handler). A handler that itself raises replaces the in-flight exception
    /// and abandons the remainder, mirroring the plain-`except` handler-throw
    /// path (its landing pad closes both catches, runs `finally`, and re-raises).
    fn lower_try_star(
        &mut self,
        exc: Value,
        excepts: &[ast::ExceptClause],
        finally: Option<&ast::Block>,
        after: BlockId,
    ) -> Result<(), LowerError> {
        // The mutable working set of still-unhandled sub-exceptions: the group's
        // children, or a fresh one-element list `[exc]` for a bare exception (so
        // a single failure matches `except* E`, treating it as a one-element
        // group — D-0129). Defined in the landing pad, which dominates every
        // clause block, so the value flows to each `split` below.
        let remaining = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(remaining),
            func: "zap_eg_remaining".to_string(),
            args: vec![exc],
        });

        for clause in excepts {
            // sema guarantees an `except*` clause names a built-in or user
            // exception type; recover its runtime type id for the split.
            let Some(type_id) = clause.ty.as_ref().and_then(exception_type_id) else {
                return Err(err(
                    "'except*' requires a named exception type (#605)".to_string()
                ));
            };
            // Peel every remaining sub-exception matching this clause's type into
            // a fresh sub-group (always a group, even for a single match), and
            // remove them from `remaining`. A null result means nothing matched,
            // so this clause's handler is skipped.
            let id = self.iconst(Ty::I64, type_id);
            let matched = self.new_value(Ty::Ptr);
            self.emit(Inst::Call {
                dest: Some(matched),
                func: "zap_eg_split".to_string(),
                args: vec![remaining, id],
            });
            let no_match = self.new_value(Ty::I1);
            self.emit(Inst::IsNull {
                dest: no_match,
                val: matched,
            });
            let body_bb = self.new_block();
            let next_bb = self.new_block();
            self.set_term(Term::BrIf {
                cond: no_match,
                then_bb: next_bb,
                else_bb: body_bb,
            });

            self.position(body_bb);
            self.push_scope();
            if let Some(bind) = &clause.binding {
                // `except* E as eg` binds `eg` to the sub-GROUP. Its value is the
                // group's rendered message string (like `except E as e`); the
                // group object is stashed under the non-typable key so
                // `eg.exceptions` can iterate the real sub-exceptions (#605),
                // exactly as `e.cause` reaches the stashed object (#604).
                let message = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(message),
                    func: "zap_exception_message".to_string(),
                    args: vec![matched],
                });
                let slot = self.alloca(Ty::Ptr);
                self.store(slot, message);
                self.declare(bind, slot, Ty::Ptr);
                let obj_slot = self.alloca(Ty::Ptr);
                self.store(obj_slot, matched);
                self.declare(&exc_obj_key(bind), obj_slot, Ty::Ptr);
            }
            // A raise inside the handler unwinds to `handler_lp`. Register
            // `finally` (if any) then `end_catch` as cleanups so an early
            // `return`/`break`/`continue` runs `end_catch` first, then `finally`
            // — the single `begin_catch` is closed exactly once on every exit.
            let handler_lp = self.new_block();
            self.landing_pads.push(handler_lp);
            let cleanup_depth = self.cleanups.len();
            if let Some(f) = finally {
                self.cleanups.push(Cleanup::Finally(f.clone()));
            }
            self.cleanups.push(Cleanup::EndCatch);
            // The matched sub-group is the active exception for the handler, so a
            // bare `raise` (#657) re-throws it.
            self.active_exc.push(matched);
            for st in &clause.body.stmts {
                self.stmt(st)?;
            }
            self.active_exc.pop();
            self.landing_pads.pop();
            let handler_throws = self.used_lps.contains(&handler_lp);
            self.cleanups.truncate(cleanup_depth);
            self.pop_scope();
            // Normal completion falls through to the NEXT clause's split (not out
            // of the `try`): another `except*` clause may still match.
            self.ensure_br(next_bb);
            if handler_throws {
                // A new exception raised by the handler replaces the caught group
                // and abandons the remainder. Close the handler's catch and the
                // outer `try` catch, run `finally`, then re-raise.
                self.position(handler_lp);
                let replacement = self.new_value(Ty::Ptr);
                self.emit(Inst::LandingPad { dest: replacement });
                self.emit_end_catch();
                self.emit_end_catch();
                if let Some(f) = finally {
                    self.lower_block(f)?;
                }
                if !self.terminated() {
                    self.throwing_call(None, "zap_raise", vec![replacement]);
                    if !self.terminated() {
                        self.set_term(Term::Unreachable);
                    }
                }
            }
            self.position(next_bb);
        }

        // After all clauses: re-raise whatever is still unhandled. `finalize`
        // returns null (nothing left), the single remaining exception (re-raised
        // bare), or a fresh `ExceptionGroup` of the two-or-more remainder.
        let rem = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(rem),
            func: "zap_eg_finalize_remainder".to_string(),
            args: vec![remaining],
        });
        let nothing_left = self.new_value(Ty::I1);
        self.emit(Inst::IsNull {
            dest: nothing_left,
            val: rem,
        });
        let reraise_bb = self.new_block();
        let done_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: nothing_left,
            then_bb: done_bb,
            else_bb: reraise_bb,
        });

        // Unhandled remainder: close the catch, run `finally`, re-raise.
        self.position(reraise_bb);
        self.emit_end_catch();
        if let Some(f) = finally {
            self.lower_block(f)?;
        }
        if !self.terminated() {
            self.throwing_call(None, "zap_raise", vec![rem]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
        }

        // Clean exit: close the catch, run `finally`, continue past the `try`.
        self.position(done_bb);
        self.emit_end_catch();
        if let Some(f) = finally {
            self.lower_block(f)?;
        }
        self.ensure_br(after);

        self.position(after);
        Ok(())
    }

    pub(super) fn lower_block(&mut self, b: &ast::Block) -> Result<(), LowerError> {
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
        self.push_loop(cond_bb, end_bb);
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
        self.push_loop(cond_bb, end_bb);
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

    /// Resolve the iterator for a struct-typed iterable (D-0048): call
    /// `__iter__` (the `Iterable` interface) to get a fresh iterator, or, if the
    /// value is itself an iterator (it defines `__next__` but no `__iter__`), use
    /// it directly. Returns the iterator value and its struct id.
    fn setup_iter(&mut self, obj: Value, sid: u32) -> Result<(Value, u32), LowerError> {
        // A generator expression's state struct (#596) is its own iterator (it
        // has a `__next__` but no `__iter__`), and its sid is tuple-interned so
        // it is not in `ctx.names`. Recognize it before the name lookup.
        if self.ctx.genexpr_iters.borrow().contains_key(&sid) {
            return Ok((obj, sid));
        }
        let name = self.ctx.names[sid as usize].clone();
        let iter_m = format!("{}__{}", name, "__iter__");
        if let Some((_p, ret)) = self.sigs.get(&iter_m).cloned() {
            let Ty::Struct(iter_sid) = ret else {
                return Err(err(format!(
                    "'{name}.__iter__' must return an iterator struct (the 'Iterable' \
                     interface, D-0048)"
                )));
            };
            let d = self.new_value(ret);
            self.throwing_call(Some(d), &iter_m, vec![obj]);
            Ok((d, iter_sid))
        } else if self.sigs.contains_key(&format!("{}__{}", name, "__next__")) {
            Ok((obj, sid)) // the value is already an iterator
        } else {
            Err(err(format!(
                "type '{name}' is not iterable: define '__iter__' (the 'Iterable' \
                 interface, D-0048)"
            )))
        }
    }

    /// Classify an iterator's `__next__` (D-0048) by its return signature: the
    /// comma-ok form `-> (T, bool)` (preferred) versus the Python/`StopIteration`
    /// form `-> T`. Returns the element type and, for comma-ok, the `(T, bool)`
    /// tuple's struct id.
    fn next_form(&self, iter_sid: u32) -> Result<NextForm, LowerError> {
        // Generator expressions (#596) carry their `__next__` signature in the
        // side table rather than `sigs`/`ctx.names`; they are always the comma-ok
        // form `-> (T, bool)`.
        if let Some(gx) = self.ctx.genexpr_iters.borrow().get(&iter_sid) {
            return Ok(NextForm::CommaOk {
                tuple_sid: gx.next_tuple_sid,
                elem: gx.elem,
            });
        }
        let name = &self.ctx.names[iter_sid as usize];
        let m = format!("{}__{}", name, "__next__");
        let (_p, ret) = self.sigs.get(&m).cloned().ok_or_else(|| {
            err(format!(
                "iterator '{name}' must define '__next__' (the 'Iterator' interface, D-0048)"
            ))
        })?;
        if let Ty::Struct(tsid) = ret {
            if let Some(fields) = self.ctx.tuples.borrow().fields(tsid) {
                if fields.len() == 2 && fields[1] == Ty::I1 {
                    return Ok(NextForm::CommaOk {
                        tuple_sid: tsid,
                        elem: fields[0],
                    });
                }
            }
        }
        Ok(NextForm::StopIter { elem: ret })
    }

    /// Drive the iteration protocol (D-0048) over struct-typed iterable `obj`
    /// (`sid`): obtain the iterator via [`setup_iter`], then loop calling
    /// `__next__`, invoking `body` with each element value/type. `body` owns the
    /// loop variable / accumulation; a `for` body's `break`/`continue` target the
    /// loop end / the next `__next__`.
    pub(super) fn drive_iteration(
        &mut self,
        obj: Value,
        sid: u32,
        mut body: impl FnMut(&mut FnLower<'a>, Value, Ty) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        self.push_scope();
        let (it, iter_sid) = self.setup_iter(obj, sid)?;
        // A genexpr iterator (#596) resolves its `__next__` through the side
        // table, not the `ctx.names`-based mangling (its sid is tuple-interned).
        let next_m = match self.ctx.genexpr_iters.borrow().get(&iter_sid) {
            Some(gx) => gx.next_fn.clone(),
            None => format!("{}__{}", self.ctx.names[iter_sid as usize], "__next__"),
        };
        match self.next_form(iter_sid)? {
            NextForm::CommaOk { tuple_sid, elem } => {
                let head = self.new_block();
                let body_bb = self.new_block();
                let end = self.new_block();
                self.set_term(Term::Br(head));
                self.position(head);
                let res = self.new_value(Ty::Struct(tuple_sid));
                self.throwing_call(Some(res), &next_m, vec![it]);
                let v = self.new_value(elem);
                self.emit(Inst::FieldGet {
                    dest: v,
                    base: res,
                    sid: tuple_sid,
                    idx: 0,
                });
                let ok = self.new_value(Ty::I1);
                self.emit(Inst::FieldGet {
                    dest: ok,
                    base: res,
                    sid: tuple_sid,
                    idx: 1,
                });
                self.set_term(Term::BrIf {
                    cond: ok,
                    then_bb: body_bb,
                    else_bb: end,
                });
                self.position(body_bb);
                self.push_loop(head, end);
                body(self, v, elem)?;
                self.loops.pop();
                self.ensure_br(head);
                self.position(end);
            }
            NextForm::StopIter { elem } => {
                // The Python form: `__next__` returns the element directly and
                // raises `StopIteration` at exhaustion. Guard the call with a
                // landing pad that ends the loop on `StopIteration` and re-raises
                // anything else (D-0048; zero-cost table EH, D-0007).
                let head = self.new_block();
                let body_bb = self.new_block();
                let lp = self.new_block();
                let reraise = self.new_block();
                let end = self.new_block();
                self.set_term(Term::Br(head));
                self.position(head);
                self.landing_pads.push(lp);
                let v = self.new_value(elem);
                self.throwing_call(Some(v), &next_m, vec![it]);
                self.landing_pads.pop();
                self.set_term(Term::Br(body_bb));
                self.position(body_bb);
                self.push_loop(head, end);
                body(self, v, elem)?;
                self.loops.pop();
                self.ensure_br(head);
                // Landing pad: match the typed StopIteration signal.
                self.position(lp);
                let exc = self.new_value(Ty::Ptr);
                self.emit(Inst::LandingPad { dest: exc });
                let stop_id = self.iconst(Ty::I64, 11);
                let eq = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(eq),
                    func: "zap_exception_is_a".to_string(),
                    args: vec![exc, stop_id],
                });
                let zero = self.iconst(Ty::I64, 0);
                let is_stop = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: is_stop,
                    pred: CmpPred::Ne,
                    lhs: eq,
                    rhs: zero,
                });
                self.emit_end_catch();
                self.set_term(Term::BrIf {
                    cond: is_stop,
                    then_bb: end,
                    else_bb: reraise,
                });
                self.position(reraise);
                self.throwing_call(None, "zap_raise", vec![exc]);
                if !self.terminated() {
                    self.set_term(Term::Unreachable);
                }
                self.position(end);
            }
        }
        self.pop_scope();
        Ok(())
    }

    /// `iter(x)` (#1382 phase 2, RFC #1417): obtain an explicit D-0048 iterator.
    /// Phase 2 accepts a value that already satisfies the protocol — a generator,
    /// a generator expression, or a user type with `__iter__`/`__next__` (all
    /// `Ty::Struct`). [`setup_iter`] calls `__iter__` (an `Iterable`) or returns
    /// the value itself (an `Iterator`, so `iter` is idempotent). A reified
    /// per-container iterator (`iter([1, 2])`, `iter("ab")`) is #1382 phase 3; a
    /// non-struct value is rejected here (sema already reports it precisely).
    pub(super) fn lower_iter_builtin(&mut self, arg: &Expr) -> Result<(Value, Ty), LowerError> {
        let (v, vt) = self.expr(arg)?;
        // A value that is already a protocol iterator (a generator / genexpr /
        // user `__iter__`/`__next__` struct) keeps phase-2's idempotent behavior:
        // `setup_iter` returns the same iterator (or calls `__iter__`), preserving
        // its identity as a live view.
        if let Ty::Struct(sid) = vt {
            let (it, iter_sid) = self.setup_iter(v, sid)?;
            return Ok((it, Ty::Struct(iter_sid)));
        }
        // A built-in container (#1382 phase 3): the reified iterator is a typed
        // synthesized generator carrying the precise element type. `for x in c`
        // already lowers every container kind (list/set/dict/str/bytes/range/
        // array/…) through the D-0048 protocol, so `iter(c)` is exactly the
        // generator expression `(x for x in c)` — reusing the genexpr pipeline
        // (`lower_genexpr`), which computes the precise `(T, bool)` element type
        // and registers the result as its own iterator. The already-lowered
        // container value is bound to a temporary local so the genexpr captures it
        // by value (evaluated once, like Python's `iter(x)`), then iterates it.
        let span = zapc_syntax::span::Span::new(0, 0);
        let src = self.alloca(vt);
        self.store(src, v);
        self.push_scope();
        self.declare("$iter_src", src, vt);
        let clause = CompClause {
            var: "$iter_v".to_string(),
            iter: Box::new(Expr::Name("$iter_src".to_string(), span)),
            ifs: Vec::new(),
            span,
        };
        let elem_e = Expr::Name("$iter_v".to_string(), span);
        let result = self.lower_genexpr(&elem_e, std::slice::from_ref(&clause));
        self.pop_scope();
        result
    }

    /// `next(it)` / `next(it, default)` (#1382 phase 2): step an iterator one
    /// element over the D-0048 protocol. Built on the comma-ok `__next__ ->
    /// (T, bool)` internally, so the iteration hot path stays exception-free
    /// (D-0048): `next(it)` constructs and raises a catchable `StopIteration`
    /// only on an explicit step past the end with no default; `next(it, default)`
    /// returns `default` at exhaustion and never raises. For the Python-form
    /// `__next__ -> T` iterator (raises `StopIteration` itself), the no-default
    /// call simply propagates the signal and the default form catches it. The
    /// `default` is evaluated eagerly (it is a call argument), matching Python.
    pub(super) fn lower_next_builtin(
        &mut self,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let (it, vt) = self.expr(&args[0].value)?;
        let Ty::Struct(iter_sid) = vt else {
            return Err(err(
                "next() expects an iterator value — a generator, a generator \
                 expression, or a type with a '__next__' method (D-0048); stepping \
                 an interface-typed Iterator[T] value is #1382 phase 3",
            ));
        };
        // Resolve `__next__` the same way `drive_iteration` does: a genexpr
        // iterator carries its emitted `__next__` name in the side table (its sid
        // is tuple-interned, not in `ctx.names`); any other iterator mangles it.
        let next_m = match self.ctx.genexpr_iters.borrow().get(&iter_sid) {
            Some(gx) => gx.next_fn.clone(),
            None => format!("{}__{}", self.ctx.names[iter_sid as usize], "__next__"),
        };
        let has_default = args.len() >= 2;
        match self.next_form(iter_sid)? {
            NextForm::CommaOk { tuple_sid, elem } => {
                // Evaluate the default eagerly (Python evaluates the argument),
                // coerced to the element type, before stepping.
                let default_val = if has_default {
                    let (dv, dt) = self.expr(&args[1].value)?;
                    Some(self.coerce(dv, dt, elem))
                } else {
                    None
                };
                let res = self.new_value(Ty::Struct(tuple_sid));
                self.throwing_call(Some(res), &next_m, vec![it]);
                let val = self.new_value(elem);
                self.emit(Inst::FieldGet {
                    dest: val,
                    base: res,
                    sid: tuple_sid,
                    idx: 0,
                });
                let ok = self.new_value(Ty::I1);
                self.emit(Inst::FieldGet {
                    dest: ok,
                    base: res,
                    sid: tuple_sid,
                    idx: 1,
                });
                let out = self.alloca(elem);
                let then_bb = self.new_block();
                let else_bb = self.new_block();
                let end_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: ok,
                    then_bb,
                    else_bb,
                });
                // A fresh element: yield it.
                self.position(then_bb);
                self.store(out, val);
                self.set_term(Term::Br(end_bb));
                // Exhausted: return the default, or raise StopIteration.
                self.position(else_bb);
                if let Some(dv) = default_val {
                    self.store(out, dv);
                    self.set_term(Term::Br(end_bb));
                } else {
                    let stop = self.iconst(Ty::I64, 11); // ZAP_EXC_STOP_ITERATION
                    let msg = self.strconst("");
                    self.throwing_call(None, "zap_panic_typed", vec![stop, msg]);
                    if !self.terminated() {
                        self.set_term(Term::Unreachable);
                    }
                }
                self.position(end_bb);
                let loaded = self.load(out, elem);
                Ok((loaded, elem))
            }
            NextForm::StopIter { elem } => {
                if !has_default {
                    // The iterator's `__next__` raises `StopIteration` at
                    // exhaustion; with no default we simply let it propagate.
                    let val = self.new_value(elem);
                    self.throwing_call(Some(val), &next_m, vec![it]);
                    return Ok((val, elem));
                }
                // With a default, evaluate it eagerly, guard the step with a
                // landing pad, and on `StopIteration` yield the default (re-raise
                // anything else) — mirroring `drive_iteration`'s StopIter guard.
                let (dv, dt) = self.expr(&args[1].value)?;
                let default_val = self.coerce(dv, dt, elem);
                let out = self.alloca(elem);
                let lp = self.new_block();
                let dflt_bb = self.new_block();
                let reraise = self.new_block();
                let end_bb = self.new_block();
                self.landing_pads.push(lp);
                let val = self.new_value(elem);
                self.throwing_call(Some(val), &next_m, vec![it]);
                self.landing_pads.pop();
                self.store(out, val);
                self.set_term(Term::Br(end_bb));
                // Landing pad: a typed StopIteration ends with the default.
                self.position(lp);
                let exc = self.new_value(Ty::Ptr);
                self.emit(Inst::LandingPad { dest: exc });
                let stop_id = self.iconst(Ty::I64, 11);
                let eq = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(eq),
                    func: "zap_exception_is_a".to_string(),
                    args: vec![exc, stop_id],
                });
                let zero = self.iconst(Ty::I64, 0);
                let is_stop = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: is_stop,
                    pred: CmpPred::Ne,
                    lhs: eq,
                    rhs: zero,
                });
                self.emit_end_catch();
                self.set_term(Term::BrIf {
                    cond: is_stop,
                    then_bb: dflt_bb,
                    else_bb: reraise,
                });
                self.position(dflt_bb);
                self.store(out, default_val);
                self.set_term(Term::Br(end_bb));
                self.position(reraise);
                self.throwing_call(None, "zap_raise", vec![exc]);
                if !self.terminated() {
                    self.set_term(Term::Unreachable);
                }
                self.position(end_bb);
                let loaded = self.load(out, elem);
                Ok((loaded, elem))
            }
        }
    }

    /// `sum(iterable)` over anything that is not a numeric `array` (#596): a
    /// lazy generator / generator expression (a `Ty::Struct` iterator) or a
    /// `list`. Drains the iterable once through the D-0048 protocol, summing
    /// into an accumulator of the element type — so a genexpr builds no
    /// intermediate list. The already-lowered iterable value `v` (type `vt`) is
    /// reused; `arg` is kept only for diagnostics.
    pub(super) fn lower_sum_iterable(
        &mut self,
        v: Value,
        vt: Ty,
        start: Option<(Value, Ty)>,
    ) -> Result<(Value, Ty), LowerError> {
        let elem_ty = match vt {
            Ty::Struct(sid) => match self.next_form(sid)? {
                NextForm::CommaOk { elem, .. } => elem,
                NextForm::StopIter { elem } => elem,
            },
            Ty::List(e) => e.ty(),
            // `sum(bytes)` reduces the byte values as `int64`s (#1355), the same
            // elements `for b in data` yields; iterated via `iterate_bytes`.
            Ty::Bytes => Ty::I64,
            other => {
                return Err(err(format!(
                    "sum() expects an array, list, iterator, or bytes value, found {other:?}"
                )))
            }
        };
        let acc = self.alloca(elem_ty);
        // Seed the accumulator with the start value (coerced to the element
        // type) when given (#1299), else the additive identity 0.
        let init = match start {
            Some((sv, st)) => self.coerce(sv, st, elem_ty),
            None => {
                if elem_ty.is_float() {
                    self.fconst(elem_ty, 0.0)
                } else {
                    self.iconst(elem_ty, 0)
                }
            }
        };
        self.store(acc, init);
        let add = move |s: &mut FnLower<'a>, ev: Value, et: Ty| -> Result<(), LowerError> {
            let cur = s.load(acc, elem_ty);
            let ev = s.coerce(ev, et, elem_ty);
            let next = s.new_value(elem_ty);
            s.emit(Inst::Bin {
                dest: next,
                op: BinOp::Add,
                lhs: cur,
                rhs: ev,
            });
            s.store(acc, next);
            Ok(())
        };
        match vt {
            Ty::Struct(sid) => self.drive_iteration(v, sid, add)?,
            Ty::List(e) => self.iterate_list(v, e, add)?,
            Ty::Bytes => self.iterate_bytes(v, add)?,
            _ => unreachable!("element type already classified above"),
        }
        let result = self.load(acc, elem_ty);
        Ok((result, elem_ty))
    }

    /// Iterate any supported iterable `iter`, calling `body(self, value, ty)` for
    /// each element: `range(...)`, a `list`, a `set`, or a user struct via the
    /// D-0048 protocol. Used by comprehensions (and the struct case of `for`).
    pub(super) fn iterate(
        &mut self,
        iter: &Expr,
        mut body: impl FnMut(&mut FnLower<'a>, Value, Ty) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        // `range(...)`: a counting loop over `[start, stop)` by `step`.
        if let Expr::Call { callee, args, .. } = iter {
            if is_name(callee, "range") {
                let mut vs = Vec::new();
                for a in args {
                    let (v, t) = self.expr(&a.value)?;
                    vs.push(self.coerce(v, t, Ty::I64));
                }
                let (start, stop, step, dynamic) = match vs.len() {
                    1 => (
                        self.iconst(Ty::I64, 0),
                        vs[0],
                        self.iconst(Ty::I64, 1),
                        false,
                    ),
                    2 => (vs[0], vs[1], self.iconst(Ty::I64, 1), false),
                    3 => (vs[0], vs[1], vs[2], true),
                    n => return Err(err(format!("range() takes 1-3 arguments, got {n}"))),
                };
                // A 3-arg range has a runtime step (#374/#759): a zero step is a
                // catchable `ValueError` (CPython), and the loop must count DOWN
                // when the step is negative. The 1-/2-arg forms always have a
                // literal step of 1, so they keep the simple ascending test.
                if dynamic {
                    let zero = self.iconst(Ty::I64, 0);
                    let is_zero = self.new_value(Ty::I1);
                    self.emit(Inst::ICmp {
                        dest: is_zero,
                        pred: CmpPred::Eq,
                        lhs: step,
                        rhs: zero,
                    });
                    let raise_bb = self.new_block();
                    let ok_bb = self.new_block();
                    self.set_term(Term::BrIf {
                        cond: is_zero,
                        then_bb: raise_bb,
                        else_bb: ok_bb,
                    });
                    self.position(raise_bb);
                    let msg = self.strconst("range() arg 3 must not be zero");
                    let verr = self.iconst(Ty::I64, 3); // ZAP_EXC_VALUE_ERROR (zaprt.c)
                    self.throwing_call(None, "zap_panic_typed", vec![verr, msg]);
                    if !self.terminated() {
                        self.set_term(Term::Unreachable);
                    }
                    self.position(ok_bb);
                }
                self.push_scope();
                let var = self.alloca(Ty::I64);
                self.store(var, start);
                let cond_bb = self.new_block();
                let body_bb = self.new_block();
                let incr_bb = self.new_block();
                let end_bb = self.new_block();
                self.set_term(Term::Br(cond_bb));
                self.position(cond_bb);
                let cur = self.load(var, Ty::I64);
                let c = self.new_value(Ty::I1);
                if dynamic {
                    // Count up while `cur < stop` for a positive step, down while
                    // `cur > stop` for a negative one: `c = (step > 0) ? lt : gt`.
                    let zero = self.iconst(Ty::I64, 0);
                    let lt = self.new_value(Ty::I1);
                    self.emit(Inst::ICmp {
                        dest: lt,
                        pred: CmpPred::Lt,
                        lhs: cur,
                        rhs: stop,
                    });
                    let gt = self.new_value(Ty::I1);
                    self.emit(Inst::ICmp {
                        dest: gt,
                        pred: CmpPred::Gt,
                        lhs: cur,
                        rhs: stop,
                    });
                    let pos = self.new_value(Ty::I1);
                    self.emit(Inst::ICmp {
                        dest: pos,
                        pred: CmpPred::Gt,
                        lhs: step,
                        rhs: zero,
                    });
                    self.emit(Inst::Select {
                        dest: c,
                        cond: pos,
                        a: lt,
                        b: gt,
                    });
                } else {
                    self.emit(Inst::ICmp {
                        dest: c,
                        pred: CmpPred::Lt,
                        lhs: cur,
                        rhs: stop,
                    });
                }
                self.set_term(Term::BrIf {
                    cond: c,
                    then_bb: body_bb,
                    else_bb: end_bb,
                });
                self.position(body_bb);
                let cur_v = self.load(var, Ty::I64);
                self.push_loop(incr_bb, end_bb);
                body(self, cur_v, Ty::I64)?;
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
                return Ok(());
            }
        }
        let (val, ty) = self.expr(iter)?;
        self.iterate_value(val, ty, body)
    }

    /// Drive `body` over an already-evaluated iterable `val` of type `ty` — the
    /// value-level core of [`Self::iterate`]. Separated out so a caller that must
    /// inspect the operand's type before iterating (the `dict(...)` constructor
    /// lowering, which distinguishes a dict-copy from a pairs iterable) can reuse
    /// the exact per-container dispatch instead of re-evaluating the expression.
    pub(super) fn iterate_value(
        &mut self,
        val: Value,
        ty: Ty,
        body: impl FnMut(&mut FnLower<'a>, Value, Ty) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        match ty {
            Ty::List(elem) => self.iterate_list(val, elem, body),
            // A variadic `tuple[T]` (#1461) is a frozen list buffer: iterate it
            // element-wise exactly like a list.
            Ty::VarTuple(elem) => self.iterate_list(val, elem, body),
            Ty::Set(elem) => {
                let items = self.new_value(Ty::List(elem));
                self.emit(Inst::Call {
                    dest: Some(items),
                    func: "zap_set_items".to_string(),
                    args: vec![val],
                });
                self.iterate_list(items, elem, body)
            }
            Ty::Heap(elem) => {
                // Iteration yields elements in priority (min-first) order without
                // draining the heap: `zap_heap_items` returns a sorted snapshot
                // list, which the list iterator then walks (D-0057).
                let items = self.new_value(Ty::List(elem));
                self.emit(Inst::Call {
                    dest: Some(items),
                    func: "zap_heap_items".to_string(),
                    args: vec![val],
                });
                self.iterate_list(items, elem, body)
            }
            // `deque`/`oset` (#983): iterate a left-to-right / insertion-order
            // snapshot list, exactly like `set`/`heap`.
            Ty::Deque(elem) => {
                let items = self.new_value(Ty::List(elem));
                self.emit(Inst::Call {
                    dest: Some(items),
                    func: "zap_deque_items".to_string(),
                    args: vec![val],
                });
                self.iterate_list(items, elem, body)
            }
            Ty::OSet(elem) => {
                let items = self.new_value(Ty::List(elem));
                self.emit(Inst::Call {
                    dest: Some(items),
                    func: "zap_oset_items".to_string(),
                    args: vec![val],
                });
                self.iterate_list(items, elem, body)
            }
            // A `dict` iterates its keys (Python semantics, #299, #1154):
            // materialize the key list via `zap_dict_keys_*` — exactly as the
            // statement-`for` path does in `lower_for_dict` — and walk it with
            // the list iterator. The element type is the key type `K`;
            // iteration order is unspecified (#1066). This is what lets a
            // comprehension (`[k for k in d]` / `{k for k in d}` / a genexpr)
            // and the `set(...)`/`oset(...)` constructors accept a dict operand.
            Ty::Dict(key, _val) => {
                let keys = self.dict_keys(val, key);
                self.iterate_list(keys, key, body)
            }
            // A `str` (a pointer handle in ZIR) iterates its codepoints as
            // `char`s (D-0025), walking the UTF-8 buffer once — so comprehensions
            // and `set(...)`/`set[T](...)` accept a string operand.
            Ty::Ptr => self.iterate_str(val, body),
            // `bytes` iterates its raw bytes as unsigned `int64`s in 0..=255
            // (#1349), byte-wise — so comprehensions, `sum(...)`, the
            // `set(...)`/`list(...)` constructors, and `*bytes` splats accept a
            // `bytes` operand, matching the statement-`for` path
            // (`lower_for_bytes`). Distinct from `str`'s codepoint iteration.
            Ty::Bytes => self.iterate_bytes(val, body),
            Ty::Struct(sid) => self.drive_iteration(val, sid, body),
            other => Err(err(format!(
                "value of type {other:?} is not iterable (D-0048)"
            ))),
        }
    }

    /// Iterate a `str`'s codepoints, invoking `body` with each `char` (an `i32`
    /// codepoint). The closure form of [`Self::lower_for_str`]: it walks the
    /// UTF-8 buffer by byte cursor, re-reading the length each step. Used by the
    /// general iteration protocol ([`Self::iterate`]) for comprehensions and the
    /// `set` constructors over a string.
    fn iterate_str(
        &mut self,
        s: Value,
        mut body: impl FnMut(&mut FnLower<'a>, Value, Ty) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        self.push_scope();
        let off = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(off, zero);
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(off, Ty::I64);
        let nbytes = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(nbytes),
            func: "zap_str_len".to_string(),
            args: vec![s],
        });
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: nbytes,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(body_bb);
        let cur_off = self.load(off, Ty::I64);
        // A codepoint read from a string is a `char` (#709), physically an i32.
        let cp = self.new_value(Ty::Char);
        self.emit(Inst::Call {
            dest: Some(cp),
            func: "zap_str_cp_at_byte".to_string(),
            args: vec![s, cur_off],
        });
        self.push_loop(incr_bb, end_bb);
        body(self, cp, Ty::Char)?;
        self.loops.pop();
        self.ensure_br(incr_bb);
        self.position(incr_bb);
        let cur2 = self.load(off, Ty::I64);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(next),
            func: "zap_str_adv_byte".to_string(),
            args: vec![s, cur2],
        });
        self.store(off, next);
        self.set_term(Term::Br(cond_bb));
        self.position(end_bb);
        self.pop_scope();
        Ok(())
    }

    /// Iterate a `bytes` value by byte index, invoking `body` with each byte as
    /// an unsigned `int64` in 0..=255 (#1349). The closure form of
    /// [`Self::lower_for_bytes`]: length via `zap_str_len` (the byte count),
    /// element via `zap_bytes_index` (unsigned, so `0x00`/`0xFF` read back as
    /// `0`/`255`). Used by the general iteration protocol ([`Self::iterate`])
    /// for comprehensions, `sum(...)`, and the `set`/`list` constructors over a
    /// `bytes` operand. Byte-wise, not UTF-8-decoded — distinct from
    /// [`Self::iterate_str`].
    pub(super) fn iterate_bytes(
        &mut self,
        data: Value,
        mut body: impl FnMut(&mut FnLower<'a>, Value, Ty) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        self.push_scope();
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let nbytes = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(nbytes),
            func: "zap_str_len".to_string(),
            args: vec![data],
        });
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: nbytes,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let byte = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(byte),
            func: "zap_bytes_index".to_string(),
            args: vec![data, i],
        });
        self.push_loop(incr_bb, end_bb);
        body(self, byte, Ty::I64)?;
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

    /// Iterate a `list[T]` value by index, re-reading the length each iteration.
    pub(super) fn iterate_list(
        &mut self,
        lst: Value,
        elem: Elem,
        mut body: impl FnMut(&mut FnLower<'a>, Value, Ty) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        self.push_scope();
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
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
        // A nested-collection element reads back as its real type (#625).
        //
        // This yields the element's PHYSICAL word (`elem_ty`): a `bool`/`char`
        // element comes through as its `i64` storage word, not narrowed to
        // `i1`/`char`. Several internal callers (the container renderer in
        // `repr.rs`, `extend`, dict copy/merge) feed this value straight into an
        // `i64`-word runtime entry point without re-coercing, so narrowing here
        // would miscompile them. The user-facing read-backs that must round-trip
        // through the element-kind check (#1241) narrow at their own sites —
        // `lower_for_list` (statement `for`, which the set/dict/heap/deque/oset
        // loops route through) and `lower_index_get` (`xs[i]`).
        let ety = self.elem_ty(elem);
        let e = self.new_value(ety);
        self.emit(Inst::Call {
            dest: Some(e),
            func: format!("zap_list_get_{}", elem.val_suffix()),
            args: vec![lst, i],
        });
        self.push_loop(incr_bb, end_bb);
        body(self, e, ety)?;
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

    /// Desugar a comprehension clause list into nested loops (#599). Each clause
    /// iterates its source through the general protocol, binds its loop variable
    /// in a fresh scope, and applies its `if` filters there (so each filter
    /// constrains the clauses to its left) before recursing into the remaining
    /// clauses; `emit` runs once at the innermost level per surviving tuple of
    /// bindings. A single clause with one filter reproduces the pre-#599 shape
    /// (one `iterate` wrapping one `comp_guard`).
    fn lower_comp_clauses(
        &mut self,
        clauses: &[CompClause],
        emit: &mut dyn FnMut(&mut FnLower<'a>) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        match clauses.split_first() {
            None => emit(self),
            Some((clause, rest)) => self.iterate(&clause.iter, |s, v, ty| {
                s.push_scope();
                let slot = s.alloca(ty);
                s.store(slot, v);
                s.declare(&clause.var, slot, ty);
                s.guard_all(&clause.ifs, &mut |s| s.lower_comp_clauses(rest, emit))?;
                s.pop_scope();
                Ok(())
            }),
        }
    }

    /// Run `work` guarded by each of `conds` as nested `if` filters (#599): the
    /// filters are evaluated outer-to-inner, so `work` runs only when all of
    /// them hold. An empty `conds` runs `work` unconditionally.
    fn guard_all(
        &mut self,
        conds: &[Expr],
        work: &mut dyn FnMut(&mut FnLower<'a>) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        match conds.split_first() {
            None => work(self),
            Some((c, rest)) => self.comp_guard(Some(c), |s| s.guard_all(rest, work)),
        }
    }

    /// `[elem for CLAUSES]` (D-0048, #599): build a fresh list, desugar the
    /// clauses to nested loops, and push `elem` at the innermost level.
    pub(super) fn lower_list_comp(
        &mut self,
        elem_e: &Expr,
        clauses: &[CompClause],
    ) -> Result<(Value, Ty), LowerError> {
        self.lower_list_comp_expected(elem_e, clauses, None)
    }

    /// `[elem for CLAUSES]` with an optional expected element kind (#1080).
    ///
    /// When the comprehension lands in an annotated destination
    /// (`list[Optional[T]] xs = [None for ...]`), `expected` carries that
    /// element kind so the element expression lowers in a type-contextual
    /// position — a bare `None` then materializes `Optional[T].None` via
    /// `expr_collection_context` instead of hitting the generic bail. With no
    /// expected kind the element kind is inferred from the first element, exactly
    /// as before. The `coerce` + `box_elem_for_store` mirror the list-literal
    /// element path so both build identical lists.
    pub(super) fn lower_list_comp_expected(
        &mut self,
        elem_e: &Expr,
        clauses: &[CompClause],
        expected: Option<Elem>,
    ) -> Result<(Value, Ty), LowerError> {
        let list = self.new_value(Ty::List(Elem::I64));
        self.emit(Inst::Call {
            dest: Some(list),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let kind = std::cell::Cell::new(expected);
        let mut emit = |s: &mut FnLower<'a>| -> Result<(), LowerError> {
            let (ev, et) = match kind.get() {
                Some(k) => s.expr_collection_context(elem_e, s.elem_ty(k))?,
                None => s.expr(elem_e)?,
            };
            let ek = match kind.get() {
                Some(k) => {
                    // A concrete value whose representation differs from the
                    // declared element is accepted only when the element is an
                    // interface and the value structurally conforms (#829);
                    // `coerce` boxes it into the fat pointer below. Any other
                    // mismatch is a representation error — same rule as a list
                    // literal element.
                    let actual = list_elem_kind(et, &s.ctx.nested)?;
                    if actual != k && !s.conforms_to_iface_elem(et, k, actual) {
                        return Err(err(format!(
                            "list element expects `{}`, found `{}`",
                            s.render_ty(&s.elem_ty(k)),
                            s.render_ty(&et)
                        )));
                    }
                    k
                }
                None => {
                    let k = list_elem_kind(et, &s.ctx.nested)?;
                    kind.set(Some(k));
                    k
                }
            };
            let ev = s.coerce(ev, et, ek.ty());
            let ev = s.box_elem_for_store(ev, ek.ty());
            s.emit(Inst::Call {
                dest: None,
                func: format!("zap_list_push_{}", ek.val_suffix()),
                args: vec![list, ev],
            });
            Ok(())
        };
        // Record the scope enclosing this comprehension so a walrus target in a
        // filter binds there (#600), surviving the per-clause scopes.
        self.comp_enclosing.push(self.scopes.len());
        let comp_result = self.lower_comp_clauses(clauses, &mut emit);
        self.comp_enclosing.pop();
        comp_result?;
        let elem = kind.get().unwrap_or(Elem::I64);
        self.func.value_types[list as usize] = Ty::List(elem);
        Ok((list, Ty::List(elem)))
    }

    /// `{elem for CLAUSES}` (D-0048, #599): like [`lower_list_comp`] into a fresh
    /// set.
    pub(super) fn lower_set_comp(
        &mut self,
        elem_e: &Expr,
        clauses: &[CompClause],
    ) -> Result<(Value, Ty), LowerError> {
        let set = self.new_value(Ty::Set(Elem::I64));
        self.emit(Inst::Call {
            dest: Some(set),
            func: "zap_set_new".to_string(),
            args: vec![],
        });
        let kind = std::cell::Cell::new(None);
        let mut emit = |s: &mut FnLower<'a>| -> Result<(), LowerError> {
            let (ev, et) = s.expr(elem_e)?;
            let ek = elem_kind(et, &s.ctx.nested)?;
            kind.set(Some(ek));
            let ev = s.coerce(ev, et, ek.ty());
            let ev = s.box_if_value(ev, ek.ty());
            let kfns = s.key_fn_args(ek)?;
            let mut args = vec![set, ev];
            args.extend_from_slice(&kfns);
            s.key_rt_call(None, format!("zap_set_add_{}", ek.key_suffix()), args, ek);
            Ok(())
        };
        // Record the scope enclosing this comprehension so a walrus target in a
        // filter binds there (#600), surviving the per-clause scopes.
        self.comp_enclosing.push(self.scopes.len());
        let comp_result = self.lower_comp_clauses(clauses, &mut emit);
        self.comp_enclosing.pop();
        comp_result?;
        let elem = kind.get().unwrap_or(Elem::I64);
        self.func.value_types[set as usize] = Ty::Set(elem);
        Ok((set, Ty::Set(elem)))
    }

    /// `(elem for CLAUSES)` — a bare **generator expression** (#596, epic #614
    /// D4). Unlike the comprehensions above it does not materialize a
    /// collection: it evaluates to a lazy `Iterator` (D-0048) built from the
    /// existing generator state-machine machinery (I#67).
    ///
    /// Because a genexpr captures its enclosing free variables — whose types are
    /// only known here, while lowering the enclosing body — its state struct
    /// cannot be a named struct registered in the module pre-pass. Instead the
    /// genexpr is desugared on the fly into an anonymous generator function
    /// whose parameters are exactly the captured free variables, which is then
    /// fed through the shared [`discover_generator`]/[`lower_generator_next`]:
    ///
    /// ```text
    /// def $genexpr$N(cap0, cap1, ...):
    ///     for v0 in iter0:
    ///         if filt0: ... for vk in iterk: ... yield elem
    /// ```
    ///
    /// The result value is its state struct, constructed with the captured
    /// values stored into the parameter fields and `$state == 0` — exactly like
    /// a generator call. The struct id is tuple-interned (not named) and forced
    /// non-inline/heap (GC-traced, like a closure environment); a side table
    /// ([`StructCtx::genexpr_iters`]) lets the iteration hooks treat it as its
    /// own iterator so `for`/`sum`/etc. consume it through the ordinary D-0048
    /// protocol. The loop variables stay inside the generator (they never leak
    /// to the enclosing scope), matching comprehensions and Python.
    pub(super) fn lower_genexpr(
        &mut self,
        elem_e: &Expr,
        clauses: &[CompClause],
    ) -> Result<(Value, Ty), LowerError> {
        let dummy = zapc_syntax::span::Span::new(0, 0);
        // 1. Free names of the genexpr, with the loop variables excluded.
        let free = {
            let mut bound: HashSet<String> = HashSet::new();
            let mut out: Vec<String> = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            let _fresh = collect_free_names_comp_clauses(clauses, &mut bound, &mut out, &mut seen);
            collect_free_names(elem_e, &mut bound, &mut out, &mut seen);
            out
        };
        // 2. Keep the free names bound in the enclosing scope; those are the
        //    captured parameters. Names resolved elsewhere (top-level functions,
        //    `range`, builtins) are left to resolve inside the generator body.
        let mut cap_names: Vec<String> = Vec::new();
        let mut cap_tys: Vec<Ty> = Vec::new();
        let mut cap_slots: Vec<Value> = Vec::new();
        for name in &free {
            if let Some((slot, ty)) = self.lookup(name) {
                cap_names.push(name.clone());
                cap_tys.push(ty);
                cap_slots.push(slot);
            }
        }
        // 3. Synthesize the anonymous generator function.
        let fname = {
            let mut c = self.ctx.genexpr_counter.borrow_mut();
            let n = *c;
            *c += 1;
            format!("$genexpr${n}")
        };
        let params: Vec<ast::Param> = cap_names
            .iter()
            .map(|n| ast::Param {
                decorators: Vec::new(),
                ty: None,
                name: n.clone(),
                default: None,
                variadic: false,
                kwargs: false,
                keyword_only: false,
                positional_only: false,
                span: dummy,
            })
            .collect();
        let yield_stmt = Stmt::Expr(Expr::Yield {
            value: Some(Box::new(elem_e.clone())),
            span: dummy,
        });
        let body = build_genexpr_body(clauses, yield_stmt, dummy);
        let synth = ast::Function {
            decorators: Vec::new(),
            name: fname.clone(),
            generics: Vec::new(),
            params,
            ret: None,
            body,
            span: dummy,
        };
        // 4. Discover body locals + element type through the shared generator
        //    machinery, with a signature table that knows the captured params.
        let mut local_sigs = self.sigs.clone();
        local_sigs.insert(fname.clone(), (cap_tys.clone(), Ty::Void));
        let (locals, elem_ty) = discover_generator(&synth, &fname, &local_sigs, self.ctx)?;
        // 5. Build the state struct: `$state` + captured params + body locals.
        let mut vars: Vec<(String, Ty)> = cap_names
            .iter()
            .cloned()
            .zip(cap_tys.iter().cloned())
            .collect();
        let n_params = vars.len();
        vars.extend(locals);
        let mut layout = vec![Ty::I64];
        layout.extend(vars.iter().map(|(_, t)| *t));
        let sid = self.ctx.tuples.borrow_mut().push_layout(layout);
        // Force the state struct to a non-inline reference (heap) struct — it
        // outlives the constructing frame and is mutated in place across
        // `__next__` calls, exactly like a generator state struct / closure env.
        self.ctx.closure_env_sids.borrow_mut().push(sid);
        let next_tuple_sid = self.ctx.tuples.borrow_mut().intern(vec![elem_ty, Ty::I1]);
        let gi = GeneratorInfo {
            name: fname.clone(),
            sid,
            elem: elem_ty,
            next_tuple_sid,
            vars,
            n_params,
        };
        // 6. Emit the state machine `__next__`; register the iterator side entry.
        let next_func = lower_generator_next(&synth, &gi, &local_sigs, self.ctx)?;
        self.ctx.generated.borrow_mut().push(next_func);
        let next_fn = format!(
            "{}____next__",
            zapc_syntax::genanalysis::gen_state_name(&fname)
        );
        self.ctx.genexpr_iters.borrow_mut().insert(
            sid,
            GenExprIter {
                next_fn,
                next_tuple_sid,
                elem: elem_ty,
            },
        );
        // 7. Construct the iterator, capturing the free-variable values into the
        //    parameter fields (`$state` defaults to 0 from `StructNew`).
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        for (i, (slot, ty)) in cap_slots.iter().zip(cap_tys.iter()).enumerate() {
            let v = self.load(*slot, *ty);
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: gen_var_field(i),
                val: v,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    /// `{key: value for CLAUSES}` (D-0048, #599): like [`lower_list_comp`] into a
    /// fresh dict.
    pub(super) fn lower_dict_comp(
        &mut self,
        key_e: &Expr,
        val_e: &Expr,
        clauses: &[CompClause],
    ) -> Result<(Value, Ty), LowerError> {
        let dict = self.new_value(Ty::Dict(Elem::I64, Elem::I64));
        self.emit(Inst::Call {
            dest: Some(dict),
            func: "zap_dict_new".to_string(),
            args: vec![],
        });
        let kinds = std::cell::Cell::new(None);
        let mut emit = |s: &mut FnLower<'a>| -> Result<(), LowerError> {
            let (kv, kt) = s.expr(key_e)?;
            let (vv, vt) = s.expr(val_e)?;
            let kk = elem_kind(kt, &s.ctx.nested)?;
            let vk = value_elem_kind(vt, &s.ctx.nested)?;
            kinds.set(Some((kk, vk)));
            let kv = s.coerce(kv, kt, kk.ty());
            let vv = s.coerce(vv, vt, vk.ty());
            let kv = s.box_if_value(kv, kk.ty());
            let vv = s.box_if_value(vv, vk.ty());
            let kfns = s.key_fn_args(kk)?;
            let mut args = vec![dict, kv, vv];
            args.extend_from_slice(&kfns);
            s.key_rt_call(
                None,
                format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix()),
                args,
                kk,
            );
            Ok(())
        };
        // Record the scope enclosing this comprehension so a walrus target in a
        // filter binds there (#600), surviving the per-clause scopes.
        self.comp_enclosing.push(self.scopes.len());
        let comp_result = self.lower_comp_clauses(clauses, &mut emit);
        self.comp_enclosing.pop();
        comp_result?;
        let (kk, vk) = kinds.get().unwrap_or((Elem::I64, Elem::I64));
        self.func.value_types[dict as usize] = Ty::Dict(kk, vk);
        Ok((dict, Ty::Dict(kk, vk)))
    }

    /// Emit `work` either unconditionally or guarded by `if cond:` (the optional
    /// comprehension filter).
    fn comp_guard(
        &mut self,
        cond: Option<&Expr>,
        work: impl FnOnce(&mut FnLower<'a>) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        match cond {
            None => work(self),
            Some(c) => {
                let (cv, cty) = self.expr(c)?;
                let cc = self.truthy(cv, cty);
                let then_bb = self.new_block();
                let cont_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: cc,
                    then_bb,
                    else_bb: cont_bb,
                });
                self.position(then_bb);
                work(self)?;
                self.ensure_br(cont_bb);
                self.position(cont_bb);
                Ok(())
            }
        }
    }

    fn lower_for(
        &mut self,
        target: &Expr,
        iter: &Expr,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let (start, stop, step, dynamic) = match iter {
            Expr::Call { callee, args, .. } if is_name(callee, "range") => {
                let mut vs = Vec::new();
                for a in args {
                    let (v, t) = self.expr(&a.value)?;
                    vs.push(self.coerce(v, t, Ty::I64));
                }
                match vs.len() {
                    1 => (
                        self.iconst(Ty::I64, 0),
                        vs[0],
                        self.iconst(Ty::I64, 1),
                        false,
                    ),
                    2 => (vs[0], vs[1], self.iconst(Ty::I64, 1), false),
                    3 => (vs[0], vs[1], vs[2], true),
                    n => return Err(err(format!("range() takes 1-3 arguments, got {n}"))),
                }
            }
            _ => {
                // `for i, x in enumerate(xs)` / `for a, b in zip(p, q)` (#1302):
                // special-cased here (like `range`) so no intermediate list of
                // tuples is materialized.
                if let Expr::Call { callee, args, .. } = iter {
                    if is_name(callee, "enumerate") {
                        return self.lower_for_enumerate(target, args, body);
                    }
                    if is_name(callee, "zip") {
                        return self.lower_for_zip(target, args, body);
                    }
                }
                if let Expr::Name(n, _) = iter {
                    if let Some(info) = self.ctx.enums.get(n).cloned() {
                        if !info.is_sum {
                            return self.lower_for_enum(target, &info, body);
                        }
                    }
                }
                // `for x in <list>` — iterate by index over the list's length.
                let (lst, lty) = self.expr(iter)?;
                // Iterating a null built-in reference (a non-`Optional`
                // `list`/`str`/`dict`/… field left unset, then looped over)
                // raises a located `NullReferenceError` instead of a SIGSEGV
                // (#1489/#1493). The guard is on the iterable value, before the
                // per-type iteration setup. A user iterable's `__iter__` receiver
                // (`Ty::Struct`) is excluded — the method-receiver follow-up.
                if let Some(owner) = Self::builtin_ref_owner(&lty) {
                    self.null_deref_guard(
                        lst,
                        NullDerefSite::IterBase,
                        None,
                        Some(owner),
                        expr_span(iter),
                    );
                }
                if let Ty::List(elem) = lty {
                    return self.lower_for_list(target, lst, elem, body);
                }
                // A variadic `tuple[T]` (#1461) is a frozen list buffer: the
                // statement `for x in t` walks it exactly like a list.
                if let Ty::VarTuple(elem) = lty {
                    return self.lower_for_list(target, lst, elem, body);
                }
                if let Ty::Set(elem) = lty {
                    return self.lower_for_set(target, lst, elem, body);
                }
                if let Ty::Dict(kk, _vk) = lty {
                    return self.lower_for_dict(target, lst, kk, body);
                }
                if let Ty::Heap(elem) = lty {
                    return self.lower_for_heap(target, lst, elem, body);
                }
                if let Ty::Deque(elem) = lty {
                    return self.lower_for_deque(target, lst, elem, body);
                }
                if let Ty::OSet(elem) = lty {
                    return self.lower_for_oset(target, lst, elem, body);
                }
                // `for c in s` where `s : str` — iterate codepoints (`char`),
                // walking the UTF-8 buffer once (D-0025).
                if lty == Ty::Ptr {
                    return self.lower_for_str(target, lst, body);
                }
                // `for b in data` where `data : bytes` — iterate the raw bytes by
                // index, binding each as an unsigned `int64` in 0..=255 (#1349).
                // Distinct from `str`'s codepoint iteration: byte-wise, not
                // UTF-8-decoded, and the loop variable is an integer, not a char.
                if lty == Ty::Bytes {
                    return self.lower_for_bytes(target, lst, body);
                }
                // `for x in <user iterable>` — the general D-0048 protocol.
                if let Ty::Struct(sid) = lty {
                    let name = match target {
                        Expr::Name(n, _) => n.clone(),
                        _ => {
                            return Err(err(
                                "only a simple loop variable is supported by codegen yet",
                            ))
                        }
                    };
                    return self.drive_iteration(lst, sid, |s, v, ty| {
                        let slot = s.alloca(ty);
                        s.store(slot, v);
                        s.declare(&name, slot, ty);
                        for st in &body.stmts {
                            s.stmt(st)?;
                        }
                        Ok(())
                    });
                }
                return Err(err(
                    "only `for ... in range(...)`, `for ... in <list>`, `for ... in <set>`, \
                     `for ... in <dict>`, and `for ... in <user iterable>` are supported by \
                     codegen yet",
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

        // A 3-arg range has a runtime step (#374/#759): a zero step is a
        // catchable `ValueError` (CPython), raised before the loop runs rather
        // than looping forever. The 1-/2-arg forms always step by a literal 1.
        if dynamic {
            let zero = self.iconst(Ty::I64, 0);
            let is_zero = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: is_zero,
                pred: CmpPred::Eq,
                lhs: step,
                rhs: zero,
            });
            let raise_bb = self.new_block();
            let ok_bb = self.new_block();
            self.set_term(Term::BrIf {
                cond: is_zero,
                then_bb: raise_bb,
                else_bb: ok_bb,
            });
            self.position(raise_bb);
            let msg = self.strconst("range() arg 3 must not be zero");
            let verr = self.iconst(Ty::I64, 3); // ZAP_EXC_VALUE_ERROR (zaprt.c)
            self.throwing_call(None, "zap_panic_typed", vec![verr, msg]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
            self.position(ok_bb);
        }

        self.push_scope();
        // The loop variable *is* the induction counter here; hoist it so it
        // survives a `yield` in the body, and spill the `stop`/`step` bounds —
        // loop-invariant SSA values used in the cond/incr blocks, which the
        // resume re-entry does not dominate (I#453). Outside a generator this
        // is a plain alloca and the bounds stay SSA.
        let var = self.decl_slot(&name, Ty::I64);
        self.store(var, start);
        let stop_slot = self.spill_invariant(stop, Ty::I64);
        let step_slot = self.spill_invariant(step, Ty::I64);

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(var, Ty::I64);
        let stop_c = self.reload_invariant(stop_slot, stop, Ty::I64);
        let c = self.new_value(Ty::I1);
        if dynamic {
            // Count up while `cur < stop` for a positive step, down while
            // `cur > stop` for a negative one: `c = (step > 0) ? lt : gt`.
            let step_c = self.reload_invariant(step_slot, step, Ty::I64);
            let zero = self.iconst(Ty::I64, 0);
            let lt = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: lt,
                pred: CmpPred::Lt,
                lhs: cur,
                rhs: stop_c,
            });
            let gt = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: gt,
                pred: CmpPred::Gt,
                lhs: cur,
                rhs: stop_c,
            });
            let pos = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: pos,
                pred: CmpPred::Gt,
                lhs: step_c,
                rhs: zero,
            });
            self.emit(Inst::Select {
                dest: c,
                cond: pos,
                a: lt,
                b: gt,
            });
        } else {
            self.emit(Inst::ICmp {
                dest: c,
                pred: CmpPred::Lt,
                lhs: cur,
                rhs: stop_c,
            });
        }
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        self.push_loop(incr_bb, end_bb);
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.loops.pop();
        self.ensure_br(incr_bb);

        self.position(incr_bb);
        let cur2 = self.load(var, Ty::I64);
        let step_i = self.reload_invariant(step_slot, step, Ty::I64);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: step_i,
        });
        self.store(var, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        self.pop_scope();
        Ok(())
    }

    /// Lower `for member in Enum` as an ordered list of the enum's canonical
    /// discriminants. Duplicate discriminants are aliases and are omitted from
    /// iteration, matching the first-declaration canonical name used by `.name`.
    fn lower_for_enum(
        &mut self,
        target: &Expr,
        info: &EnumLow,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let elem = Elem::Enum(info.enum_id);
        let list_ty = Ty::List(elem);
        let list = self.new_value(list_ty);
        self.emit(Inst::Call {
            dest: Some(list),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let mut seen = HashSet::new();
        for (_, value) in &info.members {
            if seen.insert(*value) {
                let member = self.iconst(Ty::Enum(info.enum_id), *value);
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_list_push_i64".to_string(),
                    args: vec![list, member],
                });
            }
        }
        self.lower_for_list(target, list, elem, body)
    }

    /// Resolve a tier-1 enum discriminant to its canonical source name. Explicit
    /// integer-to-enum conversions can produce unknown discriminants, which
    /// raise a catchable runtime fault instead of returning a fabricated name.
    pub(super) fn lower_enum_name(
        &mut self,
        value: Value,
        enum_id: u32,
    ) -> Result<(Value, Ty), LowerError> {
        let info = self
            .ctx
            .enums
            .values()
            .find(|info| !info.is_sum && info.enum_id == enum_id)
            .cloned()
            .ok_or_else(|| err("enum identity is not registered"))?;
        let result_slot = self.alloca(Ty::Ptr);
        let merge = self.new_block();
        let mut check = self.cur;
        let mut seen = HashSet::new();
        for (name, discriminant) in &info.members {
            if !seen.insert(*discriminant) {
                continue;
            }
            self.position(check);
            let expected = self.iconst(Ty::Enum(enum_id), *discriminant);
            let matches = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: matches,
                pred: CmpPred::Eq,
                lhs: value,
                rhs: expected,
            });
            let found = self.new_block();
            let next = self.new_block();
            self.set_term(Term::BrIf {
                cond: matches,
                then_bb: found,
                else_bb: next,
            });
            self.position(found);
            let string = self.new_value(Ty::Ptr);
            self.emit(Inst::StrConst {
                dest: string,
                bytes: name.as_bytes().to_vec(),
            });
            self.store(result_slot, string);
            self.ensure_br(merge);
            check = next;
        }
        self.position(check);
        let message = self.new_value(Ty::Ptr);
        self.emit(Inst::StrConst {
            dest: message,
            bytes: format!("invalid value for enum {}", info.enum_name).into_bytes(),
        });
        self.throwing_call(None, "zap_panic", vec![message]);
        if !self.terminated() {
            self.set_term(Term::Unreachable);
        }
        self.position(merge);
        Ok((self.load(result_slot, Ty::Ptr), Ty::Ptr))
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
        // The loop target is a simple name, or a tuple destructured from a tuple
        // element (`for k, v in d.items()`, ticket #299). A name binds once to a
        // hoisted slot stored each iteration; a tuple rebinds its names from the
        // element's fields inside the loop body.
        match target {
            Expr::Name(_, _) | Expr::Tuple(_, _) => {}
            _ => {
                return Err(err(
                    "only a simple loop variable is supported by codegen yet",
                ))
            }
        }

        self.push_scope();
        // Inside a generator body the induction index and the iterable handle
        // must live in hoisted state-struct slots so they survive a `yield` in
        // the loop body (I#453): otherwise the index is a per-call alloca that
        // reads uninitialized on resume, and the handle — an SSA value defined
        // here at loop setup — does not dominate the resume re-entry. Outside a
        // generator these stay a plain index alloca and the SSA handle.
        let idx = self.for_control_slot(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let lst_slot = self.spill_invariant(lst, Ty::List(elem));
        // A nested-collection element binds with its real type (#625) so the loop
        // body dispatches on the true `list`/`dict`/... rather than a `str` pointer.
        // The element is physically read at `elem_ty` (the `i64`-word ABI for a
        // `bool`/`char`), then narrowed to its logical read-back type `read_ty`
        // (`bool`→`i1`, `char`→`char`) so the loop variable carries its true
        // scalar identity and round-trips through the element-kind check — e.g.
        // `for value in list[bool]: copied.append(value)` (#1241).
        let ety = self.elem_ty(elem);
        let read_ty = self.elem_read_ty(elem);
        let single_var = if let Expr::Name(n, _) = target {
            // The loop variable is live across a `yield` placed after its use in
            // the body, so hoist it too (a plain alloca outside a generator).
            Some(self.decl_slot(n, read_ty))
        } else {
            None
        };

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let len = self.new_value(Ty::I64);
        let lst_c = self.reload_invariant(lst_slot, lst, Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(len),
            func: "zap_list_len".to_string(),
            args: vec![lst_c],
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
        let e = self.new_value(ety);
        let lst_b = self.reload_invariant(lst_slot, lst, Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(e),
            func: format!("zap_list_get_{}", elem.val_suffix()),
            args: vec![lst_b, i],
        });
        let (e, read_ty) = self.narrow_read_elem(e, elem);
        match target {
            Expr::Name(_, _) => self.store(single_var.expect("name slot"), e),
            Expr::Tuple(elems, _) => self.bind_tuple_loop_target(elems, e, read_ty)?,
            _ => unreachable!("validated above"),
        }
        self.push_loop(incr_bb, end_bb);
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

    /// Bind a tuple loop target (`for k, v in ...`, ticket #299) to the current
    /// element: the element must be a statically-known tuple of matching arity,
    /// and each sub-target a simple name. Each name gets a fresh slot per
    /// iteration carrying its field's value (and precise type).
    fn bind_tuple_loop_target(
        &mut self,
        elems: &[Expr],
        elem_val: Value,
        elem_ty: Ty,
    ) -> Result<(), LowerError> {
        let Ty::Struct(sid) = elem_ty else {
            return Err(err(
                "a tuple loop target requires a tuple element (e.g. `for k, v in d.items()`)",
            ));
        };
        let field_tys = self
            .ctx
            .tuples
            .borrow()
            .fields(sid)
            .ok_or_else(|| err("a tuple loop target requires a tuple element"))?;
        if elems.len() != field_tys.len() {
            return Err(err(format!(
                "tuple loop target expects {} variable(s), found {}",
                field_tys.len(),
                elems.len()
            )));
        }
        for (idx, e) in elems.iter().enumerate() {
            let Expr::Name(n, _) = e else {
                return Err(err(
                    "only simple names are supported in a tuple loop target",
                ));
            };
            let fty = field_tys[idx];
            let fv = self.field_get(elem_val, sid, idx as u32, fty);
            let slot = self.alloca(fty);
            self.store(slot, fv);
            self.declare(n, slot, fty);
        }
        Ok(())
    }

    /// `for i, x in enumerate(xs[, start])` (#1302): iterate a list by index,
    /// binding the running index (`start` + position, default 0) and the element.
    /// A `i, x` tuple target binds the two names directly; a single-name target
    /// binds the `(int64, T)` pair as a tuple. No intermediate list is built.
    fn lower_for_enumerate(
        &mut self,
        target: &Expr,
        args: &[ast::Arg],
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        if args.is_empty() || args.len() > 2 || args.iter().any(|a| a.name.is_some()) {
            return Err(err(
                "enumerate(xs[, start]) takes one or two positional arguments",
            ));
        }
        let (lst, lty) = self.expr(&args[0].value)?;
        let Ty::List(elem) = lty else {
            return Err(err("enumerate() expects a list"));
        };
        let start = if args.len() == 2 {
            let (sv, st) = self.expr(&args[1].value)?;
            self.coerce(sv, st, Ty::I64)
        } else {
            self.iconst(Ty::I64, 0)
        };
        // Target: a two-name tuple `(i, x)` or a single name.
        let pair = match target {
            Expr::Tuple(elems, _) if elems.len() == 2 => match (&elems[0], &elems[1]) {
                (Expr::Name(a, _), Expr::Name(b, _)) => Some((a.clone(), b.clone())),
                _ => return Err(err("enumerate() loop target must be two simple names")),
            },
            Expr::Name(n, _) => {
                return self.lower_for_enumerate_single(n.clone(), lst, elem, start, body)
            }
            _ => {
                return Err(err(
                    "enumerate() loop target must be `i, x` or a single name",
                ))
            }
        };
        let (iname, xname) = pair.expect("pair target");

        self.push_scope();
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let ety = self.elem_ty(elem);

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
        let e = self.new_value(ety);
        self.emit(Inst::Call {
            dest: Some(e),
            func: format!("zap_list_get_{}", elem.val_suffix()),
            args: vec![lst, i],
        });
        let (e, read_ty) = self.narrow_read_elem(e, elem);
        // index = start + position.
        let counter = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: counter,
            op: BinOp::Add,
            lhs: start,
            rhs: i,
        });
        let islot = self.alloca(Ty::I64);
        self.store(islot, counter);
        self.declare(&iname, islot, Ty::I64);
        let xslot = self.alloca(read_ty);
        self.store(xslot, e);
        self.declare(&xname, xslot, read_ty);
        self.push_loop(incr_bb, end_bb);
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

    /// `for e in enumerate(xs)` with a single loop variable: `e` binds the
    /// `(int64, T)` pair as a tuple each iteration.
    fn lower_for_enumerate_single(
        &mut self,
        name: String,
        lst: Value,
        elem: Elem,
        start: Value,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        self.push_scope();
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let ety = self.elem_ty(elem);
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
        let e = self.new_value(ety);
        self.emit(Inst::Call {
            dest: Some(e),
            func: format!("zap_list_get_{}", elem.val_suffix()),
            args: vec![lst, i],
        });
        let (e, read_ty) = self.narrow_read_elem(e, elem);
        let counter = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: counter,
            op: BinOp::Add,
            lhs: start,
            rhs: i,
        });
        let sid = self.ctx.tuples.borrow_mut().intern(vec![Ty::I64, read_ty]);
        let tup = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest: tup, sid });
        self.emit(Inst::FieldSet {
            base: tup,
            sid,
            idx: 0,
            val: counter,
        });
        self.emit(Inst::FieldSet {
            base: tup,
            sid,
            idx: 1,
            val: e,
        });
        let slot = self.alloca(Ty::Struct(sid));
        self.store(slot, tup);
        self.declare(&name, slot, Ty::Struct(sid));
        self.push_loop(incr_bb, end_bb);
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

    /// `for a, b in zip(p, q, …)` (#1302): iterate up to the SHORTEST input's
    /// length (Python's default `zip`), binding one element from each list per
    /// iteration. A tuple target binds the names directly; a single-name target
    /// binds the tuple. No intermediate list is built.
    fn lower_for_zip(
        &mut self,
        target: &Expr,
        args: &[ast::Arg],
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        if args.len() < 2 || args.iter().any(|a| a.name.is_some()) {
            return Err(err(
                "zip(a, b, …) takes two or more positional list arguments",
            ));
        }
        let mut lists: Vec<(Value, Elem)> = Vec::with_capacity(args.len());
        for a in args {
            let (v, t) = self.expr(&a.value)?;
            let Ty::List(e) = t else {
                return Err(err("zip() expects list arguments"));
            };
            lists.push((v, e));
        }
        // Loop-variable names: a tuple of N names, or a single name (tuple bind).
        let names: Option<Vec<String>> = match target {
            Expr::Tuple(elems, _) if elems.len() == lists.len() => {
                let mut ns = Vec::with_capacity(elems.len());
                for e in elems {
                    let Expr::Name(n, _) = e else {
                        return Err(err("zip() loop target must be simple names"));
                    };
                    ns.push(n.clone());
                }
                Some(ns)
            }
            Expr::Tuple(..) => {
                return Err(err(
                    "zip() loop target arity does not match the number of iterables",
                ))
            }
            Expr::Name(_, _) => None,
            _ => return Err(err("zip() loop target must be names or a single name")),
        };

        self.push_scope();
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        // Minimum length across all inputs (computed once).
        let mut minlen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(minlen),
            func: "zap_list_len".to_string(),
            args: vec![lists[0].0],
        });
        for (v, _) in &lists[1..] {
            let l = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(l),
                func: "zap_list_len".to_string(),
                args: vec![*v],
            });
            let lt = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: lt,
                pred: CmpPred::Le,
                lhs: minlen,
                rhs: l,
            });
            let m = self.new_value(Ty::I64);
            self.emit(Inst::Select {
                dest: m,
                cond: lt,
                a: minlen,
                b: l,
            });
            minlen = m;
        }

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: minlen,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        // Read one element from each list; collect (value, read_ty).
        let mut elems: Vec<(Value, Ty)> = Vec::with_capacity(lists.len());
        for (v, e) in &lists {
            let ev = self.new_value(self.elem_ty(*e));
            self.emit(Inst::Call {
                dest: Some(ev),
                func: format!("zap_list_get_{}", e.val_suffix()),
                args: vec![*v, i],
            });
            let (ev, rty) = self.narrow_read_elem(ev, *e);
            elems.push((ev, rty));
        }
        match &names {
            Some(ns) => {
                for (n, (ev, rty)) in ns.iter().zip(elems.iter()) {
                    let slot = self.alloca(*rty);
                    self.store(slot, *ev);
                    self.declare(n, slot, *rty);
                }
            }
            None => {
                let Expr::Name(n, _) = target else {
                    unreachable!()
                };
                let tys: Vec<Ty> = elems.iter().map(|(_, t)| *t).collect();
                let sid = self.ctx.tuples.borrow_mut().intern(tys);
                let tup = self.new_value(Ty::Struct(sid));
                self.emit(Inst::StructNew { dest: tup, sid });
                for (k, (ev, _)) in elems.iter().enumerate() {
                    self.emit(Inst::FieldSet {
                        base: tup,
                        sid,
                        idx: k as u32,
                        val: *ev,
                    });
                }
                let slot = self.alloca(Ty::Struct(sid));
                self.store(slot, tup);
                self.declare(n, slot, Ty::Struct(sid));
            }
        }
        self.push_loop(incr_bb, end_bb);
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

    /// `for c in s` where `s : str` (lowered to `Ptr`). Walks the UTF-8 buffer
    /// by byte offset, yielding each codepoint as a `char` (`i32`) — one pass,
    /// O(n) regardless of the ASCII flag (D-0025).
    fn lower_for_str(
        &mut self,
        target: &Expr,
        s: Value,
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
        // Byte cursor into the UTF-8 buffer, and the total byte length. Inside a
        // generator the cursor, the string handle, and the length must survive a
        // `yield` in the body, so the cursor is a hoisted slot and the handle /
        // length are spilled and reloaded (I#453); outside a generator the
        // cursor is a plain alloca and both stay SSA.
        let off = self.for_control_slot(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(off, zero);
        let nbytes = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(nbytes),
            func: "zap_str_len".to_string(),
            args: vec![s],
        });
        let nbytes_slot = self.spill_invariant(nbytes, Ty::I64);
        let s_slot = self.spill_invariant(s, Ty::Ptr);
        // Iterating a string binds each codepoint as a `char` (#709).
        let var = self.decl_slot(&name, Ty::Char);

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(off, Ty::I64);
        let nbytes_c = self.reload_invariant(nbytes_slot, nbytes, Ty::I64);
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: nbytes_c,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        let cur_off = self.load(off, Ty::I64);
        let s_b = self.reload_invariant(s_slot, s, Ty::Ptr);
        let cp = self.new_value(Ty::Char);
        self.emit(Inst::Call {
            dest: Some(cp),
            func: "zap_str_cp_at_byte".to_string(),
            args: vec![s_b, cur_off],
        });
        self.store(var, cp);
        self.push_loop(incr_bb, end_bb);
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.loops.pop();
        self.ensure_br(incr_bb);

        self.position(incr_bb);
        let cur2 = self.load(off, Ty::I64);
        let s_i = self.reload_invariant(s_slot, s, Ty::Ptr);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(next),
            func: "zap_str_adv_byte".to_string(),
            args: vec![s_i, cur2],
        });
        self.store(off, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        self.pop_scope();
        Ok(())
    }

    /// `for b in data` where `data : bytes` — iterate the raw bytes by index,
    /// binding each byte as an unsigned `int64` in 0..=255 (#1349). Mirrors
    /// [`Self::lower_for_list`]'s indexed walk (length via `zap_str_len`, the
    /// byte count; element via `zap_bytes_index`, which returns the unsigned
    /// byte so `0x00`/`0xFF` read back as `0`/`255`, never sign-extended),
    /// except the element type is a fixed `int64` rather than a collection
    /// element kind. Empty `bytes` takes zero iterations; order is preserved.
    /// Byte-wise, not UTF-8-decoded — distinct from [`Self::lower_for_str`],
    /// whose loop variable is a codepoint `char`.
    fn lower_for_bytes(
        &mut self,
        target: &Expr,
        data: Value,
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
        // Byte index into the buffer, and the total byte length. Inside a
        // generator the index, the bytes handle, and the length must survive a
        // `yield` in the body, so the index is a hoisted slot and the handle /
        // length are spilled and reloaded (I#453); outside a generator the
        // index is a plain alloca and both stay SSA. (Mirrors `lower_for_str`.)
        let idx = self.for_control_slot(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let nbytes = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(nbytes),
            func: "zap_str_len".to_string(),
            args: vec![data],
        });
        let nbytes_slot = self.spill_invariant(nbytes, Ty::I64);
        let data_slot = self.spill_invariant(data, Ty::Bytes);
        // Each byte binds as an unsigned `int64` in 0..=255 (#1349).
        let var = self.decl_slot(&name, Ty::I64);

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let nbytes_c = self.reload_invariant(nbytes_slot, nbytes, Ty::I64);
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: nbytes_c,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let data_b = self.reload_invariant(data_slot, data, Ty::Bytes);
        let byte = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(byte),
            func: "zap_bytes_index".to_string(),
            args: vec![data_b, i],
        });
        self.store(var, byte);
        self.push_loop(incr_bb, end_bb);
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

    /// Materialize a dict's keys as a fresh `list[K]` via `zap_dict_keys_*` —
    /// the shared key-snapshot primitive behind both statement `for k in d`
    /// ([`Self::lower_for_dict`]) and dict comprehensions ([`Self::iterate`]).
    /// Iterating a dict yields its keys (Python semantics, #299); the snapshot
    /// list is a GC pointer that the list-iteration machinery then roots, and
    /// its order is unspecified (#1066).
    fn dict_keys(&mut self, dict_val: Value, key: Elem) -> Value {
        let keys = self.new_value(Ty::List(key));
        self.emit(Inst::Call {
            dest: Some(keys),
            func: format!("zap_dict_keys_{}", key.val_suffix()),
            args: vec![dict_val],
        });
        keys
    }

    /// `for k in d` where `d : dict[K, V]` (ticket #299). Snapshots the dict's
    /// keys into a fresh list (order unspecified — hash order) and iterates that
    /// list, exactly like `set`. `for k, v in d.items()` is the key/value form.
    fn lower_for_dict(
        &mut self,
        target: &Expr,
        dict_val: Value,
        key: Elem,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let keys = self.dict_keys(dict_val, key);
        self.lower_for_list(target, keys, key, body)
    }

    /// `for x in <heap>` (D-0057): snapshot the heap into a list in priority
    /// (root-first) order via `zap_heap_items`, then iterate that list. The
    /// snapshot leaves the live heap untouched.
    fn lower_for_heap(
        &mut self,
        target: &Expr,
        heap_val: Value,
        elem: Elem,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let items = self.new_value(Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(items),
            func: "zap_heap_items".to_string(),
            args: vec![heap_val],
        });
        self.lower_for_list(target, items, elem, body)
    }

    /// `for x in <deque>` (#983): iterate a left-to-right snapshot list, like
    /// `heap`.
    fn lower_for_deque(
        &mut self,
        target: &Expr,
        deque_val: Value,
        elem: Elem,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let items = self.new_value(Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(items),
            func: "zap_deque_items".to_string(),
            args: vec![deque_val],
        });
        self.lower_for_list(target, items, elem, body)
    }

    /// `for x in <oset>` (#983): iterate an insertion-order snapshot list.
    fn lower_for_oset(
        &mut self,
        target: &Expr,
        oset_val: Value,
        elem: Elem,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let items = self.new_value(Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(items),
            func: "zap_oset_items".to_string(),
            args: vec![oset_val],
        });
        self.lower_for_list(target, items, elem, body)
    }
}
