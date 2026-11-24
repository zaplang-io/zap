//! Enum + match/pattern lowering (ticket #51, D-0018/D-0046).
//!
//! Tier-1 enums (unboxed backing `i64`, member access → discriminant constant)
//! and tier-3 enums (GC tagged struct), variant construction, and the full
//! `match`/`select` + pattern machinery (pattern conditions, guards, field
//! binding) that branches on the backing integer or tag. Extracted verbatim
//! from `lower.rs` (#1126); `use super::*` re-exposes the parent scope.

use super::*;

impl<'a> FnLower<'a> {
    /// Lower a qualified member/variant access with no call: `Color.RED`
    /// (tier-1 constant) or `Shape.Unit` (tier-3 payload-free variant).
    pub(super) fn lower_enum_member(
        &mut self,
        enum_name: &str,
        member: &str,
    ) -> Result<(Value, Ty), LowerError> {
        let info = self
            .ctx
            .enums
            .get(enum_name)
            .cloned()
            .expect("caller checked this is an enum");
        if !info.is_sum {
            let val = info
                .member_value(member)
                .ok_or_else(|| err(format!("enum '{enum_name}' has no member '{member}'")))?;
            let ty = Ty::Enum(info.enum_id);
            return Ok((self.iconst(ty, val), ty));
        }
        // Tier-3: a bare variant must be payload-free (data-carrying variants are
        // constructed through `call`).
        let v = info
            .variant(member)
            .ok_or_else(|| err(format!("enum '{enum_name}' has no variant '{member}'")))?
            .clone();
        if !v.fields.is_empty() {
            return Err(err(format!(
                "variant '{enum_name}.{member}' carries data; construct it with \
                 '{enum_name}.{member}(...)'"
            )));
        }
        let sid = info.sid.expect("tier-3 enum has a struct id");
        // Niche optimization (I#154, D-0064 #4): the payload-free variant of a
        // niche enum is the null pointer (`Optional.None`), typed as the enum so
        // method dispatch still resolves.
        if info.niche.is_some() {
            let dest = self.new_value(Ty::Struct(sid));
            self.emit(Inst::NullConst { dest });
            return Ok((dest, Ty::Struct(sid)));
        }
        self.build_variant(sid, &v, &[])
    }

    /// Lower a qualified variant construction `Enum.Variant(args)` (D-0046):
    /// allocate the tagged struct, store the tag, and store each payload field
    /// (positional or named). Returns the enum value (a `Struct` pointer).
    pub(super) fn lower_variant_construct(
        &mut self,
        enum_name: &str,
        variant: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let info = self
            .ctx
            .enums
            .get(enum_name)
            .cloned()
            .expect("caller checked this is an enum");
        if !info.is_sum {
            return Err(err(format!(
                "'{enum_name}' is a tier-1 enum; '{enum_name}.{variant}(...)' is not a \
                 constructor"
            )));
        }
        let v = info
            .variant(variant)
            .ok_or_else(|| err(format!("enum '{enum_name}' has no variant '{variant}'")))?
            .clone();
        let sid = info.sid.expect("tier-3 enum has a struct id");
        // Niche optimization (I#154, D-0064 #4): the one-field variant of a niche
        // enum (`Optional.Some(p)`) IS the payload pointer — no allocation, no tag.
        // The value keeps the enum's ZIR type so method dispatch resolves; it is
        // physically the payload pointer (both are opaque pointers to codegen).
        if let Some(n) = &info.niche {
            let payload = n.payload;
            if variant == n.some_variant {
                if args.len() != 1 {
                    return Err(err(format!(
                        "niche variant '{enum_name}.{variant}' takes exactly one value"
                    )));
                }
                let (val, vt) = self.expr(&args[0].value)?;
                let val = self.coerce(val, vt, payload);
                return Ok((val, Ty::Struct(sid)));
            }
        }
        self.build_variant(sid, &v, args)
    }

    /// Allocate a tier-3 tagged struct for `variant` and fill the tag slot and
    /// its payload fields from `args` (named or positional, declaration order).
    fn build_variant(
        &mut self,
        sid: u32,
        variant: &VariantLow,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        // Tag slot (field 0).
        let tag = self.iconst(Ty::I32, variant.tag);
        self.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: 0,
            val: tag,
        });
        // Payload fields.
        let mut pos = 0usize;
        for a in args {
            let (fidx, fty) = match &a.name {
                Some(fname) => variant
                    .fields
                    .iter()
                    .find(|(n, _, _)| n == fname)
                    .map(|(_, idx, t)| (*idx, *t))
                    .ok_or_else(|| {
                        err(format!("variant '{}' has no field '{fname}'", variant.name))
                    })?,
                None => {
                    let f = variant
                        .fields
                        .get(pos)
                        .ok_or_else(|| err("too many fields in variant constructor"))?;
                    pos += 1;
                    (f.1, f.2)
                }
            };
            let (val, vt) = self.expr(&a.value)?;
            let val = self.coerce(val, vt, fty);
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: fidx,
                val,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    /// Lower a `match` statement (D-0040/D-0046). The scrutinee is evaluated
    /// once; arms are tested top-to-bottom like an `if`/`elif` chain. The test
    /// compares the backing integer (tier-1 enum / literal) or the tag field
    /// (tier-3 enum); a matching arm binds its captures/payload fields, checks
    /// any guard, and runs its body. Exhaustiveness is already enforced by sema.
    /// Lower a `select` statement (D-0031, #1002). Each channel case's operand
    /// (and a send value) is evaluated once, in source order; the per-case data
    /// is passed to the variadic `zap_select` as individual arguments — a
    /// pointer send value is thus a direct pointer argument, kept rooted by this
    /// frame across the park, exactly as a plain `ch <- v` roots `v`. The call
    /// returns the chosen case index (or -1 for `default`); a dispatch chain
    /// branches to each case body, binding a receive's value (and comma-ok flag)
    /// from the out-slots.
    pub(super) fn lower_select(&mut self, cases: &[ast::SelectCase]) -> Result<(), LowerError> {
        // Out-slots for the chosen receive's delivered word + comma-ok flag.
        let outval = self.alloca(Ty::I64);
        let outok = self.alloca(Ty::I64);

        // Evaluate the channel cases (source order) and build the call args.
        // Each channel case records (is_recv, value_name, ok_name, elem_ty) for
        // the dispatch; the `default` body is remembered separately.
        struct ChanCase<'a> {
            recv: Option<(&'a str, Option<&'a str>, Ty)>, // (value, ok, elem) for recv
            body: &'a ast::Block,
        }
        let mut chan: Vec<ChanCase> = Vec::new();
        let mut default_body: Option<&ast::Block> = None;

        let mut args: Vec<Value> = vec![outval, outok];
        // `n` and `has_default` are filled once the counts are known.
        let n_idx = args.len();
        args.push(self.iconst(Ty::I64, 0));
        args.push(self.iconst(Ty::I64, 0));

        for case in cases {
            match &case.op {
                ast::SelectOp::Default => {
                    default_body = Some(&case.body);
                }
                ast::SelectOp::Recv {
                    value,
                    ok,
                    chan: ch,
                } => {
                    let (cv, cty) = self.expr(ch)?;
                    let Ty::Chan(elem) = cty else {
                        return Err(err("a select receive case needs a channel operand"));
                    };
                    let ety = self.elem_ty(elem);
                    // dir bits: bit0 = send (0 here), bit1 = ptr elem, bit2 = f64.
                    let dir = (((elem.val_suffix() == "ptr") as i64) << 1)
                        | (((elem.val_suffix() == "f64") as i64) << 2);
                    args.push(cv);
                    args.push(self.iconst(Ty::I64, dir));
                    args.push(self.iconst(Ty::I64, 0)); // recv: no send value (dummy)
                    chan.push(ChanCase {
                        recv: Some((value, ok.as_deref(), ety)),
                        body: &case.body,
                    });
                }
                ast::SelectOp::Send { chan: ch, value } => {
                    let (cv, cty) = self.expr(ch)?;
                    let Ty::Chan(elem) = cty else {
                        return Err(err("a select send case needs a channel operand"));
                    };
                    let ety = self.elem_ty(elem);
                    let vv = self.expr_coerced(value, ety, zapc_diag::ErrorCode::ZAP4032)?;
                    // dir bits: bit0 = send, bit1 = ptr elem, bit2 = f64 elem.
                    let dir = 1
                        | (((elem.val_suffix() == "ptr") as i64) << 1)
                        | (((elem.val_suffix() == "f64") as i64) << 2);
                    args.push(cv);
                    args.push(self.iconst(Ty::I64, dir));
                    args.push(vv);
                    chan.push(ChanCase {
                        recv: None,
                        body: &case.body,
                    });
                }
            }
        }

        // Patch n / has_default now that both are known.
        let n = chan.len() as i64;
        let has_default = default_body.is_some() as i64;
        args[n_idx] = self.iconst(Ty::I64, n);
        args[n_idx + 1] = self.iconst(Ty::I64, has_default);

        let chosen = self.new_value(Ty::I64);
        self.throwing_call(Some(chosen), "zap_select", args);

        // Dispatch on the chosen index: 0..n for channel cases, -1 for default.
        let merge = self.new_block();
        for (ci, cc) in chan.iter().enumerate() {
            let want = self.iconst(Ty::I64, ci as i64);
            let (cond, _) = self.binop(AstBin::Eq, chosen, Ty::I64, want, Ty::I64)?;
            let then_bb = self.new_block();
            let next_bb = self.new_block();
            self.set_term(Term::BrIf {
                cond,
                then_bb,
                else_bb: next_bb,
            });
            self.position(then_bb);
            self.push_scope();
            if let Some((value, ok, ety)) = cc.recv {
                let w = self.load(outval, ety);
                let slot = self.decl_slot(value, ety);
                self.store(slot, w);
                if let Some(okn) = ok {
                    let okb = self.load(outok, Ty::I1);
                    let okslot = self.decl_slot(okn, Ty::I1);
                    self.store(okslot, okb);
                }
            }
            self.lower_block(cc.body)?;
            self.ensure_br(merge);
            self.pop_scope();
            self.position(next_bb);
        }
        // Reached only when no channel case matched (chosen == -1): the
        // `default` body, if any. A blocking select always returns a valid
        // index, so with no default this block is unreachable.
        if let Some(def) = default_body {
            self.push_scope();
            self.lower_block(def)?;
            self.pop_scope();
        }
        self.ensure_br(merge);
        self.position(merge);
        Ok(())
    }

    pub(super) fn lower_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[ast::MatchArm],
    ) -> Result<(), LowerError> {
        let (sv, sty) = self.expr(scrutinee)?;
        let merge = self.new_block();
        for arm in arms {
            // Elide an arm whose pattern can never match this scrutinee type
            // (#1326). The branch would be a compile-time-constant `false`, yet
            // its body is still lowered — and in a generic specialization that
            // body may read nominal-struct fields off a value of an unrelated
            // type (e.g. `case Date() as day:` bound to an `i64` in
            // `promote$int64`), emitting `FieldGet` instructions the backend
            // cannot lower (it would GEP into a scalar) and crashing LLVM. A
            // statically-dead arm never executes at runtime, so dropping it is
            // behaviour-preserving and keeps the invalid field reads out of the
            // IR entirely. This generalises the earlier tuple-only shape check.
            if self.pattern_statically_dead(sty, &arm.pattern) {
                continue;
            }
            let (cond, _catch_all) = self.pattern_cond(sv, sty, &arm.pattern)?;
            let then_bb = self.new_block();
            let next_bb = self.new_block();
            self.set_term(Term::BrIf {
                cond,
                then_bb,
                else_bb: next_bb,
            });
            self.position(then_bb);
            self.push_scope();
            self.bind_pattern(sv, sty, &arm.pattern)?;
            if let Some(g) = &arm.guard {
                // A guard runs after binding; on failure fall through to the
                // next arm (it does not count toward exhaustiveness — sema).
                let (gv, gty) = self.expr(g)?;
                let gc = self.truthy(gv, gty);
                let body_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: gc,
                    then_bb: body_bb,
                    else_bb: next_bb,
                });
                self.position(body_bb);
            }
            self.lower_block(&arm.body)?;
            self.ensure_br(merge);
            self.pop_scope();
            self.position(next_bb);
        }
        // Fall-through (no arm matched): for a non-enum scrutinee this is the
        // Python no-op; for an exhaustive enum sema guarantees this is dead.
        self.ensure_br(merge);
        self.position(merge);
        Ok(())
    }

    /// Whether `pat` can be statically proven never to match a scrutinee of
    /// type `sty`, so the whole arm is dead at this specialization and must not
    /// be lowered (#1326). This mirrors the compile-time-constant `false`
    /// conditions `pattern_cond` already produces for type-incompatible
    /// scrutinees, but decides it up front so the arm's bindings and body — and
    /// any invalid `FieldGet`s they would emit against a mismatched value — are
    /// never generated. It only reports `true` when the mismatch is certain;
    /// anything runtime-decidable (literals, enum discriminants, nested
    /// field/element tests) stays live and is handled by `pattern_cond`.
    fn pattern_statically_dead(&self, sty: Ty, pat: &Pattern) -> bool {
        match pat {
            // `<pat> as name` matches exactly when its inner pattern does.
            Pattern::As { pattern, .. } => self.pattern_statically_dead(sty, pattern),
            // #1374: a comptime-generated variant arm is expanded by
            // monomorphization before lowering, so it is never seen as dead here.
            Pattern::ComptimeVariants { .. } => false,
            // An or-pattern is dead only when every alternative is dead.
            Pattern::Or(patterns, _) => {
                !patterns.is_empty()
                    && patterns
                        .iter()
                        .all(|p| self.pattern_statically_dead(sty, p))
            }
            // A nominal struct pattern `S(...)` matches only a value of struct
            // `S`; against any other type (a scalar, a different struct) it is
            // dead. An unknown name is left live so `variant_cond` reports it.
            Pattern::Struct { path, .. } if path.len() == 1 => match self.ctx.ids.get(&path[0]) {
                Some(&sid) => sty != Ty::Struct(sid),
                None => false,
            },
            // A tuple pattern needs a tuple struct of compatible arity; this is
            // the original tuple-shape check, now folded into the predicate.
            Pattern::Tuple(patterns, _) => {
                let fields = if let Ty::Struct(sid) = sty {
                    self.ctx.tuples.borrow().fields(sid)
                } else {
                    None
                };
                let rest = patterns.iter().any(|p| matches!(p, Pattern::Rest { .. }));
                fields.map_or(true, |fields| {
                    if rest {
                        fields.len() + 1 < patterns.len()
                    } else {
                        fields.len() != patterns.len()
                    }
                })
            }
            // A sequence pattern matches only a list value.
            Pattern::Sequence(..) => !matches!(sty, Ty::List(_)),
            _ => false,
        }
    }

    /// Emit the test for one pattern against the scrutinee and return its `i1`
    /// condition plus whether the pattern is an irrefutable catch-all.
    fn pattern_cond(
        &mut self,
        sv: Value,
        sty: Ty,
        pat: &Pattern,
    ) -> Result<(Value, bool), LowerError> {
        match pat {
            Pattern::Wildcard(_) | Pattern::Capture { .. } => Ok((self.bconst(true), true)),
            Pattern::Literal(e) => {
                let (lv, lt) = self.expr(e)?;
                let (c, _) = self.binop(AstBin::Eq, sv, sty, lv, lt)?;
                Ok((c, false))
            }
            Pattern::Or(ps, _) => {
                let mut acc: Option<Value> = None;
                let mut catch_all = false;
                for sub in ps {
                    let (c, ca) = self.pattern_cond(sv, sty, sub)?;
                    catch_all |= ca;
                    acc = Some(match acc {
                        None => c,
                        Some(prev) => {
                            let d = self.new_value(Ty::I1);
                            self.emit(Inst::Bin {
                                dest: d,
                                op: BinOp::Or,
                                lhs: prev,
                                rhs: c,
                            });
                            d
                        }
                    });
                }
                Ok((acc.unwrap_or_else(|| self.bconst(false)), catch_all))
            }
            Pattern::As { pattern, .. } => {
                // `<pattern> as name` tests exactly the inner pattern; the
                // binding is added in `bind_pattern` (D11, #603).
                self.pattern_cond(sv, sty, pattern)
            }
            Pattern::ComptimeVariants { .. } => Err(err(
                "internal error: a comptime-variants match arm reached lowering; \
                 monomorphization should have expanded it (#1374)",
            )),
            Pattern::Struct {
                path,
                args,
                span: _,
            } => self.variant_cond(sv, sty, path, args.as_deref()),
            Pattern::Tuple(patterns, _) => {
                let Ty::Struct(sid) = sty else {
                    return Ok((self.bconst(false), false));
                };
                let fields = self.ctx.tuples.borrow().fields(sid);
                let rest = patterns
                    .iter()
                    .position(|p| matches!(p, Pattern::Rest { .. }));
                let exact = rest.is_none();
                let Some(fields) = fields.filter(|fields| {
                    if exact {
                        fields.len() == patterns.len()
                    } else {
                        fields.len() + 1 >= patterns.len()
                    }
                }) else {
                    return Ok((self.bconst(false), false));
                };
                let required = patterns.len() - usize::from(rest.is_some());
                let len_ok = self.bconst(if exact {
                    fields.len() == required
                } else {
                    fields.len() >= required
                });
                let mut result = self.bconst(true);
                for (index, pattern) in patterns.iter().enumerate() {
                    if matches!(pattern, Pattern::Rest { .. }) {
                        continue;
                    }
                    let field_index = if rest.is_some_and(|r| index > r) {
                        fields.len() - (patterns.len() - index)
                    } else {
                        index
                    };
                    let field_ty = fields[field_index];
                    let value = self.new_value(field_ty);
                    self.emit(Inst::FieldGet {
                        dest: value,
                        base: sv,
                        sid,
                        idx: field_index as u32,
                    });
                    let (condition, _) = self.pattern_cond(value, field_ty, pattern)?;
                    let combined = self.new_value(Ty::I1);
                    self.emit(Inst::Bin {
                        dest: combined,
                        op: BinOp::And,
                        lhs: result,
                        rhs: condition,
                    });
                    result = combined;
                }
                let combined = self.new_value(Ty::I1);
                self.emit(Inst::Bin {
                    dest: combined,
                    op: BinOp::And,
                    lhs: result,
                    rhs: len_ok,
                });
                result = combined;
                Ok((result, false))
            }
            Pattern::Sequence(patterns, _) => {
                let Ty::List(elem) = sty else {
                    return Ok((self.bconst(false), false));
                };
                let rest = patterns
                    .iter()
                    .position(|p| matches!(p, Pattern::Rest { .. }));
                let len = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(len),
                    func: "zap_list_len".to_string(),
                    args: vec![sv],
                });
                let needed = self.iconst(
                    Ty::I64,
                    (patterns.len() - usize::from(rest.is_some())) as i64,
                );
                let len_ok = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: len_ok,
                    pred: if rest.is_some() {
                        CmpPred::Ge
                    } else {
                        CmpPred::Eq
                    },
                    lhs: len,
                    rhs: needed,
                });
                let result = self.guarded_pattern(len_ok, |this| {
                    let mut result = this.bconst(true);
                    for (index, pattern) in patterns.iter().enumerate() {
                        if matches!(pattern, Pattern::Rest { .. }) {
                            continue;
                        }
                        let idx = if rest.is_some_and(|r| index > r) {
                            let off = patterns.len() - index;
                            let n = this.iconst(Ty::I64, off as i64);
                            let v = this.new_value(Ty::I64);
                            this.emit(Inst::Bin {
                                dest: v,
                                op: BinOp::Sub,
                                lhs: len,
                                rhs: n,
                            });
                            v
                        } else {
                            this.iconst(Ty::I64, index as i64)
                        };
                        let ety = this.elem_ty(elem);
                        let value = this.new_value(ety);
                        this.emit(Inst::Call {
                            dest: Some(value),
                            func: format!("zap_list_get_{}", elem.val_suffix()),
                            args: vec![sv, idx],
                        });
                        let (condition, _) = this.pattern_cond(value, ety, pattern)?;
                        let combined = this.new_value(Ty::I1);
                        this.emit(Inst::Bin {
                            dest: combined,
                            op: BinOp::And,
                            lhs: result,
                            rhs: condition,
                        });
                        result = combined;
                    }
                    Ok(result)
                })?;
                Ok((result, false))
            }
            Pattern::Rest { .. } => Ok((self.bconst(true), true)),
        }
    }

    /// Evaluate a nested test only after its enclosing shape/length matched.
    fn guarded_pattern(
        &mut self,
        condition: Value,
        test: impl FnOnce(&mut Self) -> Result<Value, LowerError>,
    ) -> Result<Value, LowerError> {
        let slot = self.alloca(Ty::I1);
        let no = self.bconst(false);
        self.store(slot, no);
        let yes_bb = self.new_block();
        let end = self.new_block();
        self.set_term(Term::BrIf {
            cond: condition,
            then_bb: yes_bb,
            else_bb: end,
        });
        self.position(yes_bb);
        let result = test(self)?;
        self.store(slot, result);
        self.ensure_br(end);
        self.position(end);
        Ok(self.load(slot, Ty::I1))
    }

    fn pattern_fields(
        &self,
        sid: u32,
        args: &[PatArg],
    ) -> Result<Vec<(u32, Ty, Pattern)>, LowerError> {
        let mut pos = 0;
        args.iter()
            .map(|a| {
                let (idx, ty) = if let Some(name) = &a.name {
                    self.ctx
                        .field(sid, name)
                        .ok_or_else(|| err(format!("unknown pattern field '{name}'")))?
                } else {
                    let idx = pos;
                    pos += 1;
                    let ty = self.ctx.fields[sid as usize]
                        .get(idx)
                        .ok_or_else(|| err("too many pattern fields"))?
                        .1;
                    (idx as u32, ty)
                };
                Ok((idx, ty, a.pattern.clone()))
            })
            .collect()
    }

    fn pattern_field_conditions(
        &mut self,
        sv: Value,
        sid: u32,
        fields: &[(u32, Ty, Pattern)],
    ) -> Result<Value, LowerError> {
        let mut result = self.bconst(true);
        for (idx, ty, pattern) in fields {
            result = self.guarded_pattern(result, |this| {
                let value = this.field_get(sv, sid, *idx, *ty);
                Ok(this.pattern_cond(value, *ty, pattern)?.0)
            })?;
        }
        Ok(result)
    }

    /// Test the outer constructor before inspecting any nested payload.
    fn variant_cond(
        &mut self,
        sv: Value,
        sty: Ty,
        path: &[String],
        args: Option<&[PatArg]>,
    ) -> Result<(Value, bool), LowerError> {
        if path.len() == 1 {
            let sid = *self
                .ctx
                .ids
                .get(&path[0])
                .ok_or_else(|| err("unknown struct pattern"))?;
            if sty != Ty::Struct(sid) {
                return Ok((self.bconst(false), false));
            }
            let fields = self.pattern_fields(sid, args.unwrap_or(&[]))?;
            let nonnull = if self.ctx.is_value_struct(sid) {
                self.bconst(true)
            } else {
                let null = self.new_value(Ty::I1);
                self.emit(Inst::IsNull {
                    dest: null,
                    val: sv,
                });
                let value = self.new_value(Ty::I1);
                self.emit(Inst::Un {
                    dest: value,
                    op: UnOp::Not,
                    val: null,
                });
                value
            };
            let condition = self.guarded_pattern(nonnull, |this| {
                this.pattern_field_conditions(sv, sid, &fields)
            })?;
            return Ok((condition, false));
        }
        if path.len() != 2 {
            return Err(err("invalid constructor pattern"));
        }
        let info = self
            .ctx
            .enums
            .get(&path[0])
            .cloned()
            .ok_or_else(|| err("unknown enum pattern"))?;
        if !info.is_sum {
            let val = info
                .member_value(&path[1])
                .ok_or_else(|| err("unknown enum member"))?;
            let k = self.iconst(Ty::I64, val);
            return Ok((self.binop(AstBin::Eq, sv, sty, k, Ty::I64)?.0, false));
        }
        let v = info
            .variant(&path[1])
            .ok_or_else(|| err("unknown enum variant"))?;
        let sid = info.sid.expect("sum enum layout");
        if let Some(n) = &info.niche {
            let null = self.new_value(Ty::I1);
            self.emit(Inst::IsNull {
                dest: null,
                val: sv,
            });
            if path[1] == n.none_variant {
                return Ok((null, false));
            }
            let nonnull = self.new_value(Ty::I1);
            self.emit(Inst::Un {
                dest: nonnull,
                op: UnOp::Not,
                val: null,
            });
            let condition = self.guarded_pattern(nonnull, |this| {
                if let Some(a) = args.and_then(|a| a.first()) {
                    Ok(this.pattern_cond(sv, n.payload, &a.pattern)?.0)
                } else {
                    Ok(this.bconst(true))
                }
            })?;
            return Ok((condition, false));
        }
        let mut fields = Vec::new();
        let mut pos = 0;
        for a in args.unwrap_or(&[]) {
            let (_, idx, ty) = if let Some(name) = &a.name {
                v.fields.iter().find(|(n, _, _)| n == name)
            } else {
                let field = v.fields.get(pos);
                pos += 1;
                field
            }
            .ok_or_else(|| err("unknown variant pattern field"))?;
            fields.push((*idx, *ty, a.pattern.clone()));
        }
        let tag = self.field_get(sv, sid, 0, Ty::I32);
        let expected = self.iconst(Ty::I32, v.tag);
        let condition = self.binop(AstBin::Eq, tag, Ty::I32, expected, Ty::I32)?.0;
        let condition = self.guarded_pattern(condition, |this| {
            this.pattern_field_conditions(sv, sid, &fields)
        })?;
        Ok((condition, false))
    }

    /// Declare the bindings a matched pattern introduces into the current scope.
    fn bind_pattern(&mut self, sv: Value, sty: Ty, pat: &Pattern) -> Result<(), LowerError> {
        match pat {
            Pattern::Capture { name, .. } => {
                let slot = self.alloca(sty);
                self.store(slot, sv);
                self.declare(name, slot, sty);
                Ok(())
            }
            Pattern::As { pattern, name, .. } => {
                // Bind the inner pattern's own captures first, then bind `name`
                // to the whole matched value — same slot/type as a bare capture
                // of the scrutinee (D11, #603).
                self.bind_pattern(sv, sty, pattern)?;
                let slot = self.alloca(sty);
                self.store(slot, sv);
                self.declare(name, slot, sty);
                Ok(())
            }
            Pattern::ComptimeVariants { .. } => Err(err(
                "internal error: a comptime-variants match arm reached lowering; \
                 monomorphization should have expanded it (#1374)",
            )),
            Pattern::Tuple(patterns, _) => {
                let Ty::Struct(sid) = sty else {
                    return Ok(());
                };
                let Some(fields) = self.ctx.tuples.borrow().fields(sid) else {
                    return Ok(());
                };
                let rest = patterns
                    .iter()
                    .position(|p| matches!(p, Pattern::Rest { .. }));
                if if rest.is_some() {
                    fields.len() + 1 < patterns.len()
                } else {
                    fields.len() != patterns.len()
                } {
                    return Ok(());
                }
                let mut index = 0usize;
                for (i, pattern) in patterns.iter().enumerate() {
                    if let Pattern::Rest {
                        name: Some(name), ..
                    } = pattern
                    {
                        let after = patterns.len() - i - 1;
                        let end = fields.len().saturating_sub(after);
                        let types = fields[index..end].to_vec();
                        let sid2 = self.ctx.tuples.borrow_mut().intern(types.clone());
                        let value = self.new_value(Ty::Struct(sid2));
                        self.emit(Inst::StructNew {
                            dest: value,
                            sid: sid2,
                        });
                        for (j, ty) in types.iter().enumerate() {
                            let item = self.field_get(sv, sid, (index + j) as u32, *ty);
                            self.emit(Inst::FieldSet {
                                base: value,
                                sid: sid2,
                                idx: j as u32,
                                val: item,
                            });
                        }
                        let slot = self.alloca(Ty::Struct(sid2));
                        self.store(slot, value);
                        self.declare(name, slot, Ty::Struct(sid2));
                        index = end;
                    } else if !matches!(pattern, Pattern::Rest { .. }) {
                        if rest.is_some_and(|r| i > r) {
                            index = fields.len().saturating_sub(patterns.len() - i);
                        }
                        let ty = fields[index];
                        let item = self.field_get(sv, sid, index as u32, ty);
                        self.bind_pattern(item, ty, pattern)?;
                        index += 1;
                    }
                }
                Ok(())
            }
            Pattern::Sequence(patterns, _) => {
                let Ty::List(elem) = sty else {
                    return Ok(());
                };
                let len = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(len),
                    func: "zap_list_len".into(),
                    args: vec![sv],
                });
                let rest = patterns
                    .iter()
                    .position(|p| matches!(p, Pattern::Rest { .. }));
                for (i, pattern) in patterns.iter().enumerate() {
                    if let Pattern::Rest {
                        name: Some(name), ..
                    } = pattern
                    {
                        let lo = self.iconst(Ty::I64, i as i64);
                        let hi = self.iconst(Ty::I64, -((patterns.len() - i - 1) as i64));
                        let step = self.iconst(Ty::I64, 1);
                        let flag = self.iconst(Ty::I64, 1);
                        let no = self.iconst(Ty::I64, 0);
                        let value = self.new_value(Ty::List(elem));
                        self.emit(Inst::Call {
                            dest: Some(value),
                            func: "zap_list_slice".into(),
                            args: vec![
                                sv,
                                lo,
                                hi,
                                step,
                                flag,
                                if i + 1 == patterns.len() { no } else { flag },
                                no,
                            ],
                        });
                        let slot = self.alloca(Ty::List(elem));
                        self.store(slot, value);
                        self.declare(name, slot, Ty::List(elem));
                    } else if !matches!(pattern, Pattern::Rest { .. }) {
                        let idx = if rest.is_some_and(|r| i > r) {
                            let off = self.iconst(Ty::I64, (patterns.len() - i) as i64);
                            let v = self.new_value(Ty::I64);
                            self.emit(Inst::Bin {
                                dest: v,
                                op: BinOp::Sub,
                                lhs: len,
                                rhs: off,
                            });
                            v
                        } else {
                            self.iconst(Ty::I64, i as i64)
                        };
                        let ety = self.elem_ty(elem);
                        let item = self.new_value(ety);
                        self.emit(Inst::Call {
                            dest: Some(item),
                            func: format!("zap_list_get_{}", elem.val_suffix()),
                            args: vec![sv, idx],
                        });
                        self.bind_pattern(item, ety, pattern)?;
                    }
                }
                Ok(())
            }
            Pattern::Rest {
                name: Some(name), ..
            } => {
                let slot = self.alloca(sty);
                self.store(slot, sv);
                self.declare(name, slot, sty);
                Ok(())
            }
            Pattern::Or(patterns, _) => {
                let entry = self.cur;
                let end = self.new_block();
                let mut shared: HashMap<String, (Value, Ty)> = HashMap::new();
                for pattern in patterns {
                    let condition = self.pattern_cond(sv, sty, pattern)?.0;
                    let matched = self.new_block();
                    let next = self.new_block();
                    self.set_term(Term::BrIf {
                        cond: condition,
                        then_bb: matched,
                        else_bb: next,
                    });
                    self.position(matched);
                    self.push_scope();
                    self.bind_pattern(sv, sty, pattern)?;
                    let mut bindings: Vec<_> = self.scopes.pop().unwrap().into_iter().collect();
                    bindings.sort_by(|a, b| a.0.cmp(&b.0));
                    for (name, (local, ty)) in bindings {
                        let slot = if let Some((slot, expected)) = shared.get(&name) {
                            if *expected != ty {
                                return Err(err("or-pattern bindings must have matching types"));
                            }
                            *slot
                        } else {
                            let slot = self.new_value(Ty::Ptr);
                            self.func.blocks[entry as usize].insts.push(Inst::Alloca {
                                dest: slot,
                                pointee: ty,
                            });
                            shared.insert(name, (slot, ty));
                            slot
                        };
                        let value = self.load(local, ty);
                        self.store(slot, value);
                    }
                    self.ensure_br(end);
                    self.position(next);
                }
                self.set_term(Term::Unreachable);
                self.position(end);
                for (name, (slot, ty)) in shared {
                    self.declare(&name, slot, ty);
                }
                Ok(())
            }
            Pattern::Struct {
                path,
                args: Some(args),
                ..
            } if path.len() == 1 => {
                let sid = *self
                    .ctx
                    .ids
                    .get(&path[0])
                    .ok_or_else(|| err("unknown struct pattern"))?;
                for (idx, ty, pattern) in self.pattern_fields(sid, args)? {
                    self.bind_field(sv, sid, idx, ty, &pattern)?;
                }
                Ok(())
            }
            Pattern::Struct {
                path,
                args: Some(args),
                ..
            } if path.len() == 2 => {
                let info = self
                    .ctx
                    .enums
                    .get(&path[0])
                    .cloned()
                    .ok_or_else(|| err(format!("'{}' is not an enum", path[0])))?;
                // Niche optimization (I#154, D-0064 #4): `Some(x)` binds `x` to the
                // pointer itself (reinterpreted as the payload type); `None` binds
                // nothing. There is no tagged struct to read a field from.
                if let Some(n) = &info.niche {
                    if path[1] == n.some_variant {
                        if let Some(a) = args.first() {
                            self.bind_pattern(sv, n.payload, &a.pattern)?;
                        }
                    }
                    return Ok(());
                }
                let Some(v) = info.variant(&path[1]).cloned() else {
                    return Ok(());
                };
                let sid = info.sid.expect("tier-3 enum has a struct id");
                let mut pos = 0usize;
                for a in args {
                    let field = match &a.name {
                        Some(fname) => v.fields.iter().find(|(n, _, _)| n == fname).cloned(),
                        None => {
                            let f = v.fields.get(pos).cloned();
                            pos += 1;
                            f
                        }
                    };
                    let Some((_, fidx, fty)) = field else {
                        continue;
                    };
                    self.bind_field(sv, sid, fidx, fty, &a.pattern)?;
                }
                Ok(())
            }
            // Payload-free qualified constant (`Color.RED`, `Shape.Unit`) and
            // other refutable leaves bind nothing.
            _ => Ok(()),
        }
    }

    /// Bind one payload field's value to the capture in its sub-pattern.
    fn bind_field(
        &mut self,
        base: Value,
        sid: u32,
        fidx: u32,
        fty: Ty,
        sub: &Pattern,
    ) -> Result<(), LowerError> {
        match sub {
            Pattern::Wildcard(_) => Ok(()),
            _ => {
                let fv = self.new_value(fty);
                self.emit(Inst::FieldGet {
                    dest: fv,
                    base,
                    sid,
                    idx: fidx,
                });
                self.bind_pattern(fv, fty, sub)
            }
        }
    }
}
