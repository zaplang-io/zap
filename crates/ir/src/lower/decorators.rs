//! Decorators & reflection (ticket #16, D-0036).
//!
//! String-constant interning, the reflection-registry bootstrap, param/signal
//! registration emission, and the `reflect.*` query lowerings. The decorator
//! *collection* machinery (`DecoratorPlan`, `DerivedTrait`) stays in `lower.rs`
//! with the struct-derive flow that drives it; this submodule holds only the
//! emission/query methods under the original marker. Extracted verbatim from
//! `lower.rs` (#1127); `use super::*` re-exposes the parent scope.

use super::*;

impl<'a> FnLower<'a> {
    //
    // This section is self-contained: it builds a string constant object, emits
    // the reflection-registry bootstrap, and lowers the reflection queries. All
    // of it is item-level/runtime-support work, isolated from expression and
    // numeric lowering.

    /// A fresh string object holding `s` (via the runtime string constructor).
    pub(super) fn strconst(&mut self, s: &str) -> Value {
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::StrConst {
            dest: d,
            bytes: s.as_bytes().to_vec(),
        });
        d
    }

    /// Emit `zap_reflect_register(type, name)` for every (type, decorator/
    /// annotation) pair. Called once at the top of `main`.
    pub(super) fn emit_reflect_registrations(&mut self, meta: &[(String, String)]) {
        for (ty, name) in meta {
            let tv = self.strconst(ty);
            let nv = self.strconst(name);
            self.emit(Inst::Call {
                dest: None,
                func: "zap_reflect_register".to_string(),
                args: vec![tv, nv],
            });
        }
    }

    /// Runtime data-race detector bootstrap (`--race`, #678, epic #665, D-0114).
    /// Emitted once at the top of `main`, ONLY under `--race` (`ctx.race`): so a
    /// non-`--race` build's `main` prologue is byte-identical to today, exactly
    /// like the slice-1 access hooks (every call carries the `__zap_race_`
    /// prefix, so the "race ZIR minus the race hooks == off ZIR" invariant
    /// still holds). Two calls:
    ///
    /// - `__zap_race_enable()` arms the happens-before engine (its spawn /
    ///   `Mutex` release-acquire hooks live in the runtime and are dormant until
    ///   this runs — before any `spawn` in the user body).
    /// - `__zap_race_register_global(slot_idx, name)` records each module
    ///   global's name under its slot index, so a race report can resolve the
    ///   `name_id` the access hooks pass (slice 1) back to a human name. Every
    ///   global is registered (the id is its declaration index), not only the
    ///   racy ones, so any id a hook passes resolves.
    pub(super) fn emit_race_bootstrap(&mut self) {
        self.emit(Inst::Call {
            dest: None,
            func: "__zap_race_enable".to_string(),
            args: vec![],
        });
        // Snapshot (idx, name) first so the `globals` borrow is released before
        // `strconst`/`emit` (which do not touch it, but keep the borrow tight).
        let names: Vec<(usize, String)> = self
            .ctx
            .globals
            .borrow()
            .iter()
            .enumerate()
            .map(|(i, g)| (i, g.name.clone()))
            .collect();
        for (idx, name) in names {
            let id = self.iconst(Ty::I64, idx as i64);
            let nv = self.strconst(&name);
            self.emit(Inst::Call {
                dest: None,
                func: "__zap_race_register_global".to_string(),
                args: vec![id, nv],
            });
        }
    }

    /// Emit the signature-reflection registry population (I#458, D-0089): for
    /// every reflectable function/method, one `zap_reflect_param_register` per
    /// parameter (in declaration order) plus one `zap_reflect_sig_register` for
    /// the precomputed readable signature. Called once at the top of `main`.
    pub(super) fn emit_param_reflect_registrations(&mut self, meta: &[FnReflect]) {
        for fr in meta {
            let owner = self.strconst(&fr.owner);
            for p in &fr.params {
                let name = self.strconst(&p.name);
                let tyd = self.strconst(&p.type_disp);
                let hasdef = self.iconst(Ty::I64, if p.has_default { 1 } else { 0 });
                let annos = self.strconst(&p.annos.join(", "));
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_reflect_param_register".to_string(),
                    args: vec![owner, name, tyd, hasdef, annos],
                });
            }
            let sig = self.strconst(&fr.sig);
            self.emit(Inst::Call {
                dest: None,
                func: "zap_reflect_sig_register".to_string(),
                args: vec![owner, sig],
            });
        }
    }

    /// Emit `zap_signal_on(signo, &handler)` for every `@on_signal(Signal.X)`
    /// handler (ticket #77). Called once at the top of `main`. The runtime
    /// subscribes to the signal and spawns a detached coroutine that invokes the
    /// handler on each delivery (so it runs in full Zap context).
    pub(super) fn emit_on_signal_registrations(&mut self, meta: &[(i64, String)]) {
        for (signo, fn_name) in meta {
            let s = self.iconst(Ty::I64, *signo);
            let fv = self.new_value(Ty::Ptr);
            self.emit(Inst::FuncAddr {
                dest: fv,
                func: fn_name.clone(),
            });
            self.emit(Inst::Call {
                dest: None,
                func: "zap_signal_on".to_string(),
                args: vec![s, fv],
            });
        }
    }

    /// Lower `reflect_has(type, name) -> bool` / `reflect_names(type) -> str`.
    pub(super) fn lower_reflect_query(
        &mut self,
        fname: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        // Built-in `@shareable` reflection (#664, D-0113): the concurrency
        // primitives and concurrent collections carry the marker implicitly, so
        // they have no runtime registry entry. When both arguments are string
        // literals naming such a type and the `"shareable"` marker, fold the
        // query to a compile-time `true`. This keeps `reflect_has(Atomic,
        // "shareable")` answerable without changing the `main` prologue for
        // programs that use no reflection (golden-stable). Any other query —
        // including a user `@shareable` struct, which IS in the runtime registry
        // — falls through to the normal runtime call below.
        if fname == "reflect_has" && args.len() == 2 {
            if let (ast::Expr::Str { value: ty, .. }, ast::Expr::Str { value: name, .. }) =
                (&args[0].value, &args[1].value)
            {
                if name == zapc_syntax::SHAREABLE_MARKER
                    && zapc_syntax::BUILTIN_SHAREABLE_TYPES.contains(&ty.as_str())
                {
                    return Ok((self.iconst(Ty::I1, 1), Ty::I1));
                }
            }
        }
        let mut argvals = Vec::new();
        for a in args {
            let (v, vt) = self.expr(&a.value)?;
            if vt != Ty::Ptr {
                return Err(err(format!("{fname}() expects str arguments")));
            }
            argvals.push(v);
        }
        if fname == "reflect_names" {
            if argvals.len() != 1 {
                return Err(err("reflect_names(type) takes exactly one argument"));
            }
            let d = self.new_value(Ty::Ptr);
            self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_reflect_names".to_string(),
                args: argvals,
            });
            return Ok((d, Ty::Ptr));
        }
        // reflect_has: the runtime returns 1/0 as i64; normalize to a bool.
        if argvals.len() != 2 {
            return Err(err("reflect_has(type, name) takes exactly two arguments"));
        }
        let word = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(word),
            func: "zap_reflect_has".to_string(),
            args: argvals,
        });
        let zero = self.iconst(Ty::I64, 0);
        let d = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: d,
            pred: CmpPred::Ne,
            lhs: word,
            rhs: zero,
        });
        Ok((d, Ty::I1))
    }

    /// Lower the signature-reflection queries (I#458, D-0089):
    /// `reflect_params(owner)`, `reflect_sig(owner)`, `reflect_param_type(owner,
    /// param)`, `reflect_param_annotations(owner, param)` (all `-> str`), and
    /// `reflect_param_has_default(owner, param) -> bool`. Each forwards string
    /// arguments to its runtime entry point; the only one returning an `i64` is
    /// normalized to a `bool`.
    pub(super) fn lower_reflect_param_query(
        &mut self,
        fname: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let arity = if fname == "reflect_params" || fname == "reflect_sig" {
            1
        } else {
            2
        };
        if args.len() != arity {
            return Err(err(format!("{fname}() takes exactly {arity} argument(s)")));
        }
        let mut argvals = Vec::new();
        for a in args {
            let (v, vt) = self.expr(&a.value)?;
            if vt != Ty::Ptr {
                return Err(err(format!("{fname}() expects str arguments")));
            }
            argvals.push(v);
        }
        let rtfn = match fname {
            "reflect_params" => "zap_reflect_params",
            "reflect_sig" => "zap_reflect_sig",
            "reflect_param_type" => "zap_reflect_param_type",
            "reflect_param_annotations" => "zap_reflect_param_annotations",
            "reflect_param_has_default" => "zap_reflect_param_has_default",
            _ => return Err(err(format!("unknown reflection query '{fname}'"))),
        };
        if fname == "reflect_param_has_default" {
            // The runtime returns 1/0 as i64; normalize to a bool.
            let word = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(word),
                func: rtfn.to_string(),
                args: argvals,
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
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(d),
            func: rtfn.to_string(),
            args: argvals,
        });
        Ok((d, Ty::Ptr))
    }
}
