//! First-class function values lowering (I#163).
//!
//! Closures and function values: the shared captureless closure-record struct,
//! per-capture env struct interning, lambda lowering and the lifted-lambda
//! function builder, plus the `fnval`/wrapper plumbing a bare function name
//! lowers to. Extracted verbatim from `lower.rs` (#1126); a child module of
//! `lower`, so these `impl super::FnLower` methods still see its private items
//! for free (`use super::*` re-exposes the parent scope).

use super::*;

impl<'a> FnLower<'a> {
    /// The shared `{code_ptr}` closure-record struct id for a captureless
    /// function value, interned once and reused.
    fn captureless_env_sid(&self) -> u32 {
        if let Some(sid) = *self.ctx.captureless_env.borrow() {
            return sid;
        }
        let sid = self.ctx.tuples.borrow_mut().push_layout(vec![Ty::Ptr]);
        self.ctx.closure_env_sids.borrow_mut().push(sid);
        *self.ctx.captureless_env.borrow_mut() = Some(sid);
        sid
    }

    /// The closure-record struct id `{code_ptr, cap_tys…}` for a value capturing
    /// `cap_tys` (the captureless one when empty).
    fn env_sid_for(&self, cap_tys: &[Ty]) -> u32 {
        if cap_tys.is_empty() {
            return self.captureless_env_sid();
        }
        let mut fields = Vec::with_capacity(cap_tys.len() + 1);
        fields.push(Ty::Ptr);
        fields.extend_from_slice(cap_tys);
        let sid = self.ctx.tuples.borrow_mut().push_layout(fields);
        self.ctx.closure_env_sids.borrow_mut().push(sid);
        sid
    }

    /// A fresh unique name for a compiler-generated closure function (I#163).
    fn fresh_closure_name(&self) -> String {
        let mut c = self.ctx.closure_counter.borrow_mut();
        let n = *c;
        *c += 1;
        format!("$closure${n}")
    }

    /// Build a first-class function value from a bare top-level function
    /// (I#163): generate (once) a wrapper whose code pointer takes the implicit
    /// env and calls the function, then emit the captureless closure record.
    pub(super) fn fnval_of_name(&mut self, fname: &str) -> Result<(Value, Ty), LowerError> {
        let (params, ret) = self
            .sigs
            .get(fname)
            .cloned()
            .ok_or_else(|| err(format!("unknown function '{fname}'")))?;
        let sig_idx = self.ctx.fn_sigs.borrow_mut().intern(params.clone(), ret);
        let (wname, env_sid) = self.make_fnval_wrapper(fname, &params, ret);
        let dest = self.new_value(Ty::Func(sig_idx));
        self.emit(Inst::ClosureMake {
            dest,
            func: wname,
            env_sid,
            captures: Vec::new(),
        });
        Ok((dest, Ty::Func(sig_idx)))
    }

    /// Generate (once per source function) the wrapper `<f>$fnval(env, args…)`
    /// that ignores the env and calls `f(args…)` — the code pointer for a bare
    /// top-level function used as a value. Returns `(wrapper_name, env_sid)`.
    fn make_fnval_wrapper(&self, fname: &str, params: &[Ty], ret: Ty) -> (String, u32) {
        let env_sid = self.captureless_env_sid();
        if let Some(w) = self.ctx.fnval_wrappers.borrow().get(fname) {
            return (w.clone(), env_sid);
        }
        let wname = format!("{fname}$fnval");
        let mut params_z = Vec::with_capacity(params.len() + 1);
        params_z.push(Ty::Struct(env_sid));
        params_z.extend_from_slice(params);
        let mut lf = FnLower::new(
            wname.clone(),
            params_z,
            ret,
            false,
            false,
            self.sigs,
            self.ctx,
        );
        // Values 1..=n are the incoming args (value 0 is the ignored env).
        let argvals: Vec<Value> = (1..=params.len() as u32).collect();
        let dest = if ret == Ty::Void {
            None
        } else {
            Some(lf.new_value(ret))
        };
        lf.emit(Inst::Call {
            dest,
            func: fname.to_string(),
            args: argvals,
        });
        lf.set_term(Term::Ret(dest));
        self.ctx.generated.borrow_mut().push(lf.func);
        self.ctx
            .fnval_wrappers
            .borrow_mut()
            .insert(fname.to_string(), wname.clone());
        (wname, env_sid)
    }

    /// Lower a `lambda` to a first-class function value (I#163), given the
    /// expected signature `sig_idx` (the lambda's parameter types come from it —
    /// Zap lambdas are untyped on the surface, like Python's). Free variables
    /// bound in the enclosing scope are captured into a GC-managed closure
    /// record; the body becomes a lifted top-level function that reloads them.
    pub(super) fn lower_lambda(
        &mut self,
        lam_params: &[String],
        body: &Expr,
        sig_idx: u32,
    ) -> Result<(Value, Ty), LowerError> {
        let sig = self.ctx.fn_sigs.borrow().get(sig_idx).clone();
        if lam_params.len() != sig.params.len() {
            return Err(err(format!(
                "this lambda has {} parameter(s) but its expected type takes {}",
                lam_params.len(),
                sig.params.len()
            )));
        }
        // Capture the free variables that are bound in the current scope; names
        // resolved elsewhere (top-level functions, imports, builtins) are left
        // to resolve directly inside the lifted body.
        let free = free_names_of(lam_params, body);
        let mut names = Vec::new();
        let mut cap_vals = Vec::new();
        let mut cap_tys = Vec::new();
        for name in &free {
            if let Some((slot, ty)) = self.lookup(name) {
                names.push(name.clone());
                cap_vals.push(self.load(slot, ty));
                cap_tys.push(ty);
            }
        }
        let env_sid = self.env_sid_for(&cap_tys);
        let fname = self.fresh_closure_name();
        let lifted = self.build_lifted_lambda(
            &fname,
            env_sid,
            &names,
            &cap_tys,
            lam_params,
            &sig.params,
            sig.ret,
            body,
        )?;
        self.ctx.generated.borrow_mut().push(lifted);
        let dest = self.new_value(Ty::Func(sig_idx));
        self.emit(Inst::ClosureMake {
            dest,
            func: fname,
            env_sid,
            captures: cap_vals,
        });
        Ok((dest, Ty::Func(sig_idx)))
    }

    /// Build the lifted top-level function for a lambda body (I#163): its ZIR
    /// param 0 is the closure record, from which the captured variables are
    /// reloaded; params 1.. are the lambda's parameters. The body expression is
    /// lowered and returned (coerced to the declared return type).
    #[allow(clippy::too_many_arguments)]
    fn build_lifted_lambda(
        &self,
        name: &str,
        env_sid: u32,
        captured: &[String],
        cap_tys: &[Ty],
        lam_params: &[String],
        param_tys: &[Ty],
        ret: Ty,
        body: &Expr,
    ) -> Result<Func, LowerError> {
        let mut params_z = Vec::with_capacity(param_tys.len() + 1);
        params_z.push(Ty::Struct(env_sid));
        params_z.extend_from_slice(param_tys);
        let mut lf = FnLower::new(
            name.to_string(),
            params_z,
            ret,
            false,
            false,
            self.sigs,
            self.ctx,
        );
        // Reload captured free variables from the closure record (param 0,
        // fields 1..) into fresh locals under their source names.
        let env_val: Value = 0;
        for (i, (cname, cty)) in captured.iter().zip(cap_tys).enumerate() {
            let got = lf.new_value(*cty);
            lf.emit(Inst::FieldGet {
                dest: got,
                base: env_val,
                sid: env_sid,
                idx: (i + 1) as u32,
            });
            let slot = lf.alloca(*cty);
            lf.store(slot, got);
            lf.declare(cname, slot, *cty);
        }
        // Bind the lambda parameters (ZIR values 1..=n).
        for (i, pname) in lam_params.iter().enumerate() {
            let pty = param_tys[i];
            let slot = lf.alloca(pty);
            lf.store(slot, (i + 1) as Value);
            lf.declare(pname, slot, pty);
        }
        let (v, vt) = lf.expr(body)?;
        if ret == Ty::Void {
            lf.set_term(Term::Ret(None));
        } else {
            let rv = lf.coerce(v, vt, ret);
            lf.set_term(Term::Ret(Some(rv)));
        }
        Ok(lf.func)
    }
}
