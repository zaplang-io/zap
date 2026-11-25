//! Binary-operator lowering.
//!
//! The core `binop` method (the int/float/str/bytes/overload paths, the single
//! largest method in lowering), user-struct operator-dunder dispatch
//! (`struct_binop`, forward on the left / reflected on the right, #1180),
//! `is None` / `is not None` (`lower_is_none`, #611), and the small literal
//! predicates `binop` uses for division/shift safety. Extracted verbatim from
//! `lower.rs` (#1135); `use super::*` re-exposes the parent scope (including the
//! `struct_has_dunder` helper, which stays in `lower.rs`).

use super::*;

/// The element-kind tag passed to `zap_vartuple_eq` (#1474): 0 = plain 64-bit
/// word (int/bool/char/enum), 1 = `f64`, 2 = `str`/`bytes` (`ZapStr` pointer).
/// `None` for an element kind `==` does not support yet (nested collection,
/// struct, complex, function) — reported as a clean error.
fn vartuple_elem_tag(elem: Elem) -> Option<i64> {
    match elem {
        Elem::I64 | Elem::Bool | Elem::Char | Elem::Enum(_) => Some(0),
        Elem::F64 => Some(1),
        Elem::Ptr | Elem::Bytes => Some(2),
        _ => None,
    }
}

impl<'a> FnLower<'a> {
    /// Desugar a binary operator whose left operand is a user struct to the
    /// matching dunder (D-0047). Equality/relational operators derive from
    /// `__eq__` / `__cmp__`; `and`/`or` use truthiness; the rest call their
    /// arithmetic/bitwise dunder directly and take its return type.
    fn struct_binop(
        &mut self,
        op: AstBin,
        sid: u32,
        l: Value,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let lt = Ty::Struct(sid);
        // `Optional[T]` defines `==`/`!=` (structural) and ordering (a total
        // order with `None` before every `Some`) directly, not via a
        // `__eq__`/`__cmp__` dunder (#1368). Intercept before the dunder dispatch
        // below, which would otherwise bail with "does not define '__eq__'".
        if matches!(op, Eq | Ne | Lt | Le | Gt | Ge) {
            if let Some(opt) = optional_layout(self.ctx, lt) {
                return self.optional_binop(op, &opt, l, r, rt);
            }
        }
        match op {
            Eq | Ne => {
                // Left `__eq__` by name wins; else reflected `__req__` on the
                // right (`a == b` -> `b.__req__(a)`, symmetric), #1198.
                let rsid = if let Ty::Struct(s) = &rt {
                    Some(*s)
                } else {
                    None
                };
                let use_reflected = !self.struct_has_dunder(sid, "__eq__")
                    && rsid.is_some_and(|s| self.struct_has_dunder(s, "__req__"));
                let (bit, _) = if use_reflected {
                    self.call_dunder(rsid.unwrap(), "__req__", vec![(r, rt), (l, lt)])?
                } else {
                    self.call_dunder(sid, "__eq__", vec![(l, lt), (r, rt)])?
                };
                if matches!(op, Ne) {
                    let d = self.new_value(Ty::I1);
                    self.emit(Inst::Un {
                        dest: d,
                        op: UnOp::Not,
                        val: bit,
                    });
                    Ok((d, Ty::I1))
                } else {
                    Ok((bit, Ty::I1))
                }
            }
            Lt | Le | Gt | Ge => {
                // `a < b` -> `a.__cmp__(b) < 0`. Left `__cmp__` by name wins; else
                // reflected `__rcmp__` on the right (`b.__rcmp__(a)`, which returns
                // the sign of `a <=> b`, so the derivation is identical), #1198.
                let rsid = if let Ty::Struct(s) = &rt {
                    Some(*s)
                } else {
                    None
                };
                let use_reflected = !self.struct_has_dunder(sid, "__cmp__")
                    && rsid.is_some_and(|s| self.struct_has_dunder(s, "__rcmp__"));
                let (cmp, _) = if use_reflected {
                    self.call_dunder(rsid.unwrap(), "__rcmp__", vec![(r, rt), (l, lt)])?
                } else {
                    self.call_dunder(sid, "__cmp__", vec![(l, lt), (r, rt)])?
                };
                let pred = match op {
                    Lt => CmpPred::Lt,
                    Le => CmpPred::Le,
                    Gt => CmpPred::Gt,
                    Ge => CmpPred::Ge,
                    _ => unreachable!(),
                };
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred,
                    lhs: cmp,
                    rhs: zero,
                });
                Ok((d, Ty::I1))
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
            Send => Err(err("'<-' (channel send) is not an overloadable operator")),
            In => unreachable!("'in' dispatches on the right operand"),
            _ => {
                let dunder = match op {
                    Add => "__add__",
                    Sub => "__sub__",
                    Mul => "__mul__",
                    Div => "__truediv__",
                    FloorDiv => "__floordiv__",
                    Mod => "__mod__",
                    Pow => "__pow__",
                    MatMul => "__matmul__",
                    BitAnd => "__and__",
                    BitOr => "__or__",
                    BitXor => "__xor__",
                    Shl => "__lshift__",
                    Shr => "__rshift__",
                    _ => unreachable!(),
                };
                // Forward on the left, parameter-aware (#1200). The left owns the
                // operator only when it defines the forward dunder AND that
                // dunder's monomorphized parameter accepts the right operand's
                // type. This is where the erased-generic left is resolved
                // concretely: sema could not test a type-variable parameter
                // (`Unknown`, permissive), so it kept the forward commit and left
                // the real accept/reject decision to here, where `self.sigs`
                // holds the monomorphized signature.
                let forward_ok = self.struct_has_dunder(sid, dunder)
                    && self.forward_param_accepts(sid, dunder, rt);
                if forward_ok {
                    return self.call_dunder(sid, dunder, vec![(l, lt), (r, rt)]);
                }
                // Reflected fallback (#1180/#1200): the right operand's reflected
                // dunder (`a + b` -> `b.__radd__(a)`), reached when the left
                // struct defines no forward dunder OR its forward dunder provably
                // rejects this RHS. `self` is the right operand.
                if let (Some(rdunder), Ty::Struct(rsid)) = (reflected_op_dunder(op), rt) {
                    if self.struct_has_dunder(rsid, rdunder) {
                        return self.call_dunder(rsid, rdunder, vec![(r, rt), (l, lt)]);
                    }
                }
                // The left's forward dunder rejects the RHS and no reflected
                // handler applies (#1200). In the normal pipeline sema has
                // already reported this (ZAP4071); the ir crate is also exercised
                // without sema, so emit a clean mismatch error here rather than
                // forming a forward call whose argument `coerce` would reinterpret
                // (the arithmetic dunders are outside `call_dunder`'s index-only
                // representation gate).
                if self.struct_has_dunder(sid, dunder) {
                    return Err(err(format!(
                        "type '{}' does not support operator with operand type '{}': \
                         its '{dunder}' does not accept this operand and no reflected \
                         handler is defined (D-0047/#1200)",
                        self.ctx.names[sid as usize],
                        self.render_ty(&rt),
                    )));
                }
                // The left defines no forward dunder and nothing reflected
                // applies: preserve the original clean "missing forward dunder"
                // error from `call_dunder`.
                self.call_dunder(sid, dunder, vec![(l, lt), (r, rt)])
            }
        }
    }

    /// `==`/`!=`/ordering on `Optional[T]` (#1368). `==`/`!=` are structural
    /// (`None == None`; `Some(x) == Some(y)` iff `x == y`; `Some` never equals
    /// `None`); ordering is a total order with `None` before every `Some`
    /// (`Option<T>`-style). A bare payload on the right (`opt == 7`) is compared
    /// as `opt == Some(7)`. The `Some`/`Some` payload comparison recurses into
    /// the ordinary `binop`, so the payload type's own operator — and its clean
    /// "not equatable/ordered" diagnostic when it lacks one — is reused.
    fn optional_binop(
        &mut self,
        op: AstBin,
        opt: &OptionalLayout,
        l: Value,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let optty = Ty::Struct(opt.sid);
        // Normalize the right operand to the Optional type: a bare payload
        // (`opt == 7`) becomes `Some(7)`.
        let r = if rt == optty {
            r
        } else {
            let inner = self.coerce(r, rt, opt.payload_ty);
            gen_opt_some(self, opt, inner)
        };
        match op {
            Eq | Ne => {
                let bit = self.optional_eq(opt, l, r)?;
                if matches!(op, Ne) {
                    let d = self.new_value(Ty::I1);
                    self.emit(Inst::Un {
                        dest: d,
                        op: UnOp::Not,
                        val: bit,
                    });
                    Ok((d, Ty::I1))
                } else {
                    Ok((bit, Ty::I1))
                }
            }
            // `a > b` == `b < a`, `a >= b` == `b <= a` — the operands (both now
            // `Optional[T]`) are swapped and routed through the `<` / `<=` forms.
            Lt => Ok((self.optional_lt(opt, l, r)?, Ty::I1)),
            Le => Ok((self.optional_le(opt, l, r)?, Ty::I1)),
            Gt => Ok((self.optional_lt(opt, r, l)?, Ty::I1)),
            Ge => Ok((self.optional_le(opt, r, l)?, Ty::I1)),
            _ => unreachable!("optional_binop is only called for a comparison operator"),
        }
    }

    /// Structural `Optional[T]` equality (#1368): true iff both are `None`, or
    /// both are `Some` with equal payloads. Payloads are only compared (via the
    /// ordinary `==` lowering) in the both-`Some` block, so a niche `None`'s
    /// absent payload is never read.
    fn optional_eq(
        &mut self,
        opt: &OptionalLayout,
        l: Value,
        r: Value,
    ) -> Result<Value, LowerError> {
        let l_none = gen_opt_is_none(self, opt, l);
        let r_none = gen_opt_is_none(self, opt, r);
        let either = self.new_value(Ty::I1);
        self.emit(Inst::Bin {
            dest: either,
            op: BinOp::Or,
            lhs: l_none,
            rhs: r_none,
        });
        let both = self.new_value(Ty::I1);
        self.emit(Inst::Bin {
            dest: both,
            op: BinOp::And,
            lhs: l_none,
            rhs: r_none,
        });
        let slot = self.alloca(Ty::I1);
        let none_bb = self.new_block();
        let some_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: either,
            then_bb: none_bb,
            else_bb: some_bb,
        });
        // At least one operand is `None`: equal iff both are `None`.
        self.position(none_bb);
        self.store(slot, both);
        self.set_term(Term::Br(end_bb));
        // Both are `Some`: compare the payloads with the ordinary `==`.
        self.position(some_bb);
        let lp = gen_opt_payload(self, opt, l);
        let rp = gen_opt_payload(self, opt, r);
        let (pe, _) = self.binop(AstBin::Eq, lp, opt.payload_ty, rp, opt.payload_ty)?;
        self.store(slot, pe);
        self.set_term(Term::Br(end_bb));
        self.position(end_bb);
        Ok(self.load(slot, Ty::I1))
    }

    /// `l < r` for `Optional[T]`, `None` before every `Some` (#1368):
    /// `r` is `None` -> false (nothing is `< None`); else `l` is `None` -> true
    /// (`None < Some`); else `lp < rp`.
    fn optional_lt(
        &mut self,
        opt: &OptionalLayout,
        l: Value,
        r: Value,
    ) -> Result<Value, LowerError> {
        let slot = self.alloca(Ty::I1);
        let r_none = gen_opt_is_none(self, opt, r);
        let rnone_bb = self.new_block();
        let rsome_bb = self.new_block();
        let ltrue_bb = self.new_block();
        let payload_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: r_none,
            then_bb: rnone_bb,
            else_bb: rsome_bb,
        });
        self.position(rnone_bb);
        let f = self.iconst(Ty::I1, 0);
        self.store(slot, f);
        self.set_term(Term::Br(end_bb));
        self.position(rsome_bb);
        let l_none = gen_opt_is_none(self, opt, l);
        self.set_term(Term::BrIf {
            cond: l_none,
            then_bb: ltrue_bb,
            else_bb: payload_bb,
        });
        self.position(ltrue_bb);
        let t = self.iconst(Ty::I1, 1);
        self.store(slot, t);
        self.set_term(Term::Br(end_bb));
        self.position(payload_bb);
        let lp = gen_opt_payload(self, opt, l);
        let rp = gen_opt_payload(self, opt, r);
        let (pc, _) = self.binop(AstBin::Lt, lp, opt.payload_ty, rp, opt.payload_ty)?;
        self.store(slot, pc);
        self.set_term(Term::Br(end_bb));
        self.position(end_bb);
        Ok(self.load(slot, Ty::I1))
    }

    /// `l <= r` for `Optional[T]`, `None` before every `Some` (#1368):
    /// `l` is `None` -> true (`None <= anything`); else `r` is `None` -> false;
    /// else `lp <= rp`.
    fn optional_le(
        &mut self,
        opt: &OptionalLayout,
        l: Value,
        r: Value,
    ) -> Result<Value, LowerError> {
        let slot = self.alloca(Ty::I1);
        let l_none = gen_opt_is_none(self, opt, l);
        let ltrue_bb = self.new_block();
        let lsome_bb = self.new_block();
        let rnone_bb = self.new_block();
        let payload_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: l_none,
            then_bb: ltrue_bb,
            else_bb: lsome_bb,
        });
        self.position(ltrue_bb);
        let t = self.iconst(Ty::I1, 1);
        self.store(slot, t);
        self.set_term(Term::Br(end_bb));
        self.position(lsome_bb);
        let r_none = gen_opt_is_none(self, opt, r);
        self.set_term(Term::BrIf {
            cond: r_none,
            then_bb: rnone_bb,
            else_bb: payload_bb,
        });
        self.position(rnone_bb);
        let f = self.iconst(Ty::I1, 0);
        self.store(slot, f);
        self.set_term(Term::Br(end_bb));
        self.position(payload_bb);
        let lp = gen_opt_payload(self, opt, l);
        let rp = gen_opt_payload(self, opt, r);
        let (pc, _) = self.binop(AstBin::Le, lp, opt.payload_ty, rp, opt.payload_ty)?;
        self.store(slot, pc);
        self.set_term(Term::Br(end_bb));
        self.position(end_bb);
        Ok(self.load(slot, Ty::I1))
    }

    /// Lower `x is None` / `x is not None` (#611, D18). Exactly one operand is a
    /// literal `None` (the caller guarantees it); the other is lowered and
    /// tested for emptiness. An `Optional[T]` reuses the niche/tagged
    /// empty-variant check (`gen_opt_is_none`); a plain reference `struct` is
    /// None exactly when its pointer is null. `None is None` folds to a
    /// constant. `is not` negates the result.
    pub(super) fn lower_is_none(
        &mut self,
        is_not: bool,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<(Value, Ty), LowerError> {
        let l_none = matches!(lhs, Expr::None(_));
        let r_none = matches!(rhs, Expr::None(_));
        if l_none && r_none {
            // `None is None` is True; `None is not None` is False.
            return Ok((self.bconst(!is_not), Ty::I1));
        }
        let other = if l_none { rhs } else { lhs };
        let (v, t) = self.expr(other)?;
        let is_none = if let Some(opt) = optional_layout(self.ctx, t) {
            gen_opt_is_none(self, &opt, v)
        } else {
            // Reference struct (or other reference pointer): null is None. Sema
            // (`is_nullable_ty`) has already rejected non-nullable operands.
            let d = self.new_value(Ty::I1);
            self.emit(Inst::IsNull { dest: d, val: v });
            d
        };
        if is_not {
            let d = self.new_value(Ty::I1);
            self.emit(Inst::Un {
                dest: d,
                op: UnOp::Not,
                val: is_none,
            });
            Ok((d, Ty::I1))
        } else {
            Ok((is_none, Ty::I1))
        }
    }

    /// True when `e` is a numeric literal that is definitely nonzero, so a
    /// `/` by it needs no divide-by-zero guard (#374). Conservative: grouped
    /// (`1_000`), hex, or other forms that don't parse here fall through to the
    /// guarded path rather than being wrongly treated as safe.
    pub(super) fn is_nonzero_numeric_literal(e: &Expr) -> bool {
        match e {
            Expr::Int(s, _) => s.parse::<i128>().map(|n| n != 0).unwrap_or(false),
            Expr::Float(s, _) => s.parse::<f64>().map(|f| f != 0.0).unwrap_or(false),
            _ => false,
        }
    }

    /// True when `e` is an integer literal shift count in `0..=63`, so the shift
    /// needs no range/sign guard (#374). Anything else — negative, `>= 64`,
    /// non-literal, or a form that doesn't parse here — takes the guarded path.
    pub(super) fn is_safe_shift_literal(e: &Expr) -> bool {
        match e {
            Expr::Int(s, _) => s
                .parse::<i64>()
                .map(|n| (0..64).contains(&n))
                .unwrap_or(false),
            _ => false,
        }
    }

    pub(super) fn binop(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;

        // `not in` (#593): negated membership. Reuse the `in` lowering verbatim
        // (the membership probe that dispatches on the right operand) and negate
        // its boolean result, so `x not in xs` lowers identically to
        // `not (x in xs)` — no separate membership path, no codegen surface.
        if matches!(op, NotIn) {
            let (b, bt) = self.binop(In, l, lt, r, rt)?;
            let bi = self.truthy(b, bt);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::Un {
                dest: d,
                op: UnOp::Not,
                val: bi,
            });
            return Ok((d, Ty::I1));
        }

        // `is` / `is not` (#611, D18): reference identity (same allocation).
        // Both operands are reference values — sema (`check_identity`) rejects
        // value types, and a literal `None` operand is handled earlier in the
        // `Expr::Binary` lowerer (it has the static types, so it emits the
        // null/empty-variant check). Reaching here both sides are concrete
        // reference pointers, so compare them directly; this is also the path a
        // chained `a is b` link takes. Codegen's `ICmp` accepts pointer
        // operands (it `ptrtoint`s them), so no element-kind dispatch is needed.
        if matches!(op, Is | IsNot) {
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: if matches!(op, Is) {
                    CmpPred::Eq
                } else {
                    CmpPred::Ne
                },
                lhs: l,
                rhs: r,
            });
            return Ok((d, Ty::I1));
        }

        // `==` / `!=` on two variadic `tuple[T]` values (#1474): element-wise
        // equality over the frozen list buffers, by element-kind tag. Ordering /
        // concat (#1475) and dict/set-key use are follow-ups.
        if matches!(op, Eq | Ne) {
            if let (Ty::VarTuple(le), Ty::VarTuple(_)) = (&lt, &rt) {
                let tag = vartuple_elem_tag(*le).ok_or_else(|| {
                    err("tuple[...] of this element type does not support == yet (#1474)")
                })?;
                let kind = self.iconst(Ty::I32, tag);
                let raw = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: "zap_vartuple_eq".to_string(),
                    args: vec![l, r, kind],
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: if matches!(op, Eq) {
                        CmpPred::Ne
                    } else {
                        CmpPred::Eq
                    },
                    lhs: raw,
                    rhs: zero,
                });
                return Ok((d, Ty::I1));
            }
        }

        // Channel send `ch <- v` (ticket #25, D-0031): handled before the
        // generic reference-type guard below, since the channel operand is a
        // reference type. Blocks until the value can be delivered; the element
        // kind picks the typed runtime entry point. Used as a statement, so the
        // result is a unit-like `i32 0`.
        if matches!(op, Send) {
            let Ty::Chan(elem) = lt else {
                return Err(err("the left operand of '<-' (send) must be a channel"));
            };
            let v = self.coerce(r, rt, elem.ty());
            self.throwing_call(
                None,
                &format!("zap_chan_send_{}", elem.val_suffix()),
                vec![l, v],
            );
            return Ok((self.iconst(Ty::I32, 0), Ty::I32));
        }

        // `@` matrix multiply / dot product (#509, PEP 465): not element-wise,
        // so it routes to its own runtime kernel rather than `array_binop`.
        if matches!(op, AstBin::MatMul)
            && (matches!(lt, Ty::Array(..)) || matches!(rt, Ty::Array(..)))
        {
            return self.lower_array_matmul(l, lt, r, rt);
        }

        // Element-wise comparison `a > 3` / `a == b` -> array[bool] (#510,
        // D-0175), the mask source for boolean-mask indexing. Must precede the
        // arithmetic `array_binop` (which rejects comparison ops).
        if matches!(op, Eq | Ne | Lt | Le | Gt | Ge)
            && (matches!(lt, Ty::Array(..)) || matches!(rt, Ty::Array(..)))
        {
            return self.lower_array_cmp(op, l, lt, r, rt);
        }
        // `&`/`|`/`^` combine bool masks (and `~` inverts, in the unary lowerer)
        // -> array[bool] (#510). Only bool-dtype array operands are allowed;
        // integer-array bitwise is a separate element-wise op (not indexing).
        if matches!(op, BitAnd | BitOr | BitXor)
            && (matches!(lt, Ty::Array(..)) || matches!(rt, Ty::Array(..)))
        {
            return self.lower_array_logical(op, l, lt, r, rt);
        }

        // Element-wise numeric-array ops (ticket #24): same-shape `a + b` and
        // scalar broadcast `a + 2.0`, lowered to a SIMD kernel in codegen.
        if matches!(lt, Ty::Array(..)) || matches!(rt, Ty::Array(..)) {
            return self.array_binop(op, l, lt, r, rt);
        }

        // Borrowed `strview` equality (#1082): `view == view`, `view == str`, or
        // `str == view`, comparing the borrowed bytes with no owned allocation.
        // Only equality is defined on a view in v1 (ordering → materialize).
        if matches!(op, Eq | Ne) {
            // Borrowed-view equality (#1082): `strview`/`bytesview` vs the same, or
            // vs an owned `str`/`bytes`. Every case is a raw byte compare over the
            // shared `ZapStr` storage, so one pair of entry points serves both view
            // kinds. `l_view`/`r_view` carry the operand's reserved layout id.
            let l_view = match lt {
                Ty::Struct(s) if self.is_any_view_sid(s) => Some(s),
                _ => None,
            };
            let r_view = match rt {
                Ty::Struct(s) if self.is_any_view_sid(s) => Some(s),
                _ => None,
            };
            if l_view.is_some() || r_view.is_some() {
                let raw = self.new_value(Ty::I64);
                match (l_view, r_view) {
                    (Some(ls), Some(rs)) => {
                        let (lb, lo, ll) = self.view_fields(l, ls);
                        let (rb, ro, rl) = self.view_fields(r, rs);
                        self.emit(Inst::Call {
                            dest: Some(raw),
                            func: "zap_strview_eq".to_string(),
                            args: vec![lb, lo, ll, rb, ro, rl],
                        });
                    }
                    (Some(ls), None) if matches!(rt, Ty::Ptr | Ty::Bytes) => {
                        let (lb, lo, ll) = self.view_fields(l, ls);
                        self.emit(Inst::Call {
                            dest: Some(raw),
                            func: "zap_strview_eq_str".to_string(),
                            args: vec![lb, lo, ll, r],
                        });
                    }
                    (None, Some(rs)) if matches!(lt, Ty::Ptr | Ty::Bytes) => {
                        let (rb, ro, rl) = self.view_fields(r, rs);
                        self.emit(Inst::Call {
                            dest: Some(raw),
                            func: "zap_strview_eq_str".to_string(),
                            args: vec![rb, ro, rl, l],
                        });
                    }
                    _ => {
                        return Err(err(
                            "a borrowed view compares for equality only with a str/bytes \
                             or another view",
                        ))
                    }
                }
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: if matches!(op, Eq) {
                        CmpPred::Ne
                    } else {
                        CmpPred::Eq
                    },
                    lhs: raw,
                    rhs: zero,
                });
                return Ok((d, Ty::I1));
            }
        }

        if lt == Ty::Complex || rt == Ty::Complex {
            let l = self.coerce(l, lt, Ty::Complex);
            let r = self.coerce(r, rt, Ty::Complex);
            if matches!(op, Eq | Ne) {
                let raw = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: "zap_complex_eq".to_string(),
                    args: vec![l, r],
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: if matches!(op, Eq) {
                        CmpPred::Ne
                    } else {
                        CmpPred::Eq
                    },
                    lhs: raw,
                    rhs: zero,
                });
                return Ok((d, Ty::I1));
            }
            let func = match op {
                Add => "zap_complex_add",
                Sub => "zap_complex_sub",
                Mul => "zap_complex_mul",
                Div => "zap_complex_div",
                Pow => "zap_complex_pow",
                _ => return Err(err("unsupported operation on complex values")),
            };
            let d = self.new_value(Ty::Complex);
            if matches!(op, Div) {
                self.throwing_call(Some(d), func, vec![l, r]);
            } else {
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: func.to_string(),
                    args: vec![l, r],
                });
            }
            return Ok((d, Ty::Complex));
        }

        if (lt == Ty::Bigint || rt == Ty::Bigint) && !lt.is_float() && !rt.is_float() {
            if matches!(op, Div) {
                let lf = self.as_f64(l, lt);
                let rf = self.as_f64(r, rt);
                let d = self.new_value(Ty::F64);
                self.emit(Inst::Bin {
                    dest: d,
                    op: BinOp::Div,
                    lhs: lf,
                    rhs: rf,
                });
                return Ok((d, Ty::F64));
            }
            let l = self.coerce(l, lt, Ty::Bigint);
            let r = self.coerce(r, rt, Ty::Bigint);
            if matches!(op, Eq | Ne | Lt | Le | Gt | Ge) {
                let cmp = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(cmp),
                    func: "zap_bigint_cmp".to_string(),
                    args: vec![l, r],
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                let pred = match op {
                    Eq => CmpPred::Eq,
                    Ne => CmpPred::Ne,
                    Lt => CmpPred::Lt,
                    Le => CmpPred::Le,
                    Gt => CmpPred::Gt,
                    Ge => CmpPred::Ge,
                    _ => unreachable!(),
                };
                self.emit(Inst::ICmp {
                    dest: d,
                    pred,
                    lhs: cmp,
                    rhs: zero,
                });
                return Ok((d, Ty::I1));
            }
            let (func, can_throw) = match op {
                Add => ("zap_bigint_add", false),
                Sub => ("zap_bigint_sub", false),
                Mul => ("zap_bigint_mul", false),
                FloorDiv => ("zap_bigint_floordiv", true),
                Mod => ("zap_bigint_mod", true),
                Pow => ("zap_bigint_pow", true),
                _ => return Err(err("unsupported operation on bigint values")),
            };
            let d = self.new_value(Ty::Bigint);
            if can_throw {
                self.throwing_call(Some(d), func, vec![l, r]);
            } else {
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: func.to_string(),
                    args: vec![l, r],
                });
            }
            return Ok((d, Ty::Bigint));
        }

        // Element-wise tuple comparison (D-0049): `t1 == t2` / `t1 != t2` compares
        // positionally, recursing into each element (so nested tuples compare
        // element-wise too). A tuple is an anonymous struct but not a *user*
        // struct, so this must precede operator-overloading dispatch below (a
        // tuple has no dunder methods); other operators on tuples are unsupported
        // and fall through to the struct path, which reports a clear error.
        if matches!(op, Eq | Ne) {
            if let Ty::Struct(ls) = lt {
                if self.ctx.tuples.borrow().is_tuple(ls) {
                    return self.tuple_cmp(op, l, ls, r, rt);
                }
            }
        }

        // Structural list equality (#1239): `l1 == l2` / `l1 != l2` compares the
        // two lists element-wise (order- and length-sensitive), the Python value
        // semantics for sequences, rather than by pointer identity. Must precede
        // the generic numeric/reference fall-through below, which would otherwise
        // `ptrtoint`-compare the list headers and report two separately allocated
        // but equal lists as unequal. `is` / `is not` keep identity (handled at
        // the top of `binop`). Equality is symmetric, so a list on either side is
        // routed with the list operand on the left; `list_cmp` raises a clear
        // error if the other operand is not itself a list.
        if matches!(op, Eq | Ne) && (matches!(lt, Ty::List(_)) || matches!(rt, Ty::List(_))) {
            if let Ty::List(lelem) = lt {
                return self.list_cmp(op, l, lelem, r, rt);
            }
            let Ty::List(relem) = rt else { unreachable!() };
            return self.list_cmp(op, r, relem, l, lt);
        }

        // Lexicographic list ordering (#1334 sibling): `l1 < l2` / `<=` / `>` /
        // `>=` compare two lists element-wise — the first differing element
        // decides (by element `<`), and a shorter list that is a prefix of the
        // other sorts first — matching Python's sequence ordering. Without this
        // the operands fall through to the generic numeric path below, which
        // `ptrtoint`-compares the two list headers (a meaningless allocation
        // address compare). Ordering is NOT symmetric, so operands are never
        // swapped: the list must be the left operand (a non-list left with a list
        // right is a type error either way).
        if matches!(op, Lt | Le | Gt | Ge)
            && (matches!(lt, Ty::List(_)) || matches!(rt, Ty::List(_)))
        {
            let Ty::List(lelem) = lt else {
                return Err(err(
                    "a list's `< <= > >=` (lexicographic) compares only with another list",
                ));
            };
            let Ty::List(relem) = rt else {
                return Err(err(
                    "a list's `< <= > >=` (lexicographic) compares only with another list",
                ));
            };
            return self.list_order_cmp(op, l, lelem, r, relem);
        }

        // Structural dict equality (#1328): `==` / `!=` on two dicts compares
        // entries, order-independent (Python mapping semantics), rather than by
        // pointer identity (which the generic fall-through would do). Equality is
        // symmetric, so a dict on either side routes with the dict on the left;
        // `dict_cmp` raises a clear error if the other operand is not a dict.
        if matches!(op, Eq | Ne) && (matches!(lt, Ty::Dict(..)) || matches!(rt, Ty::Dict(..))) {
            if let Ty::Dict(lkk, lvk) = lt {
                return self.dict_cmp(op, l, lkk, lvk, r, rt);
            }
            let Ty::Dict(rkk, rvk) = rt else {
                unreachable!()
            };
            return self.dict_cmp(op, r, rkk, rvk, l, lt);
        }

        // Structural set equality (#1328): membership-based, order-independent
        // (Python set semantics), rather than pointer identity.
        if matches!(op, Eq | Ne) && (matches!(lt, Ty::Set(_)) || matches!(rt, Ty::Set(_))) {
            if let Ty::Set(lelem) = lt {
                return self.set_cmp(op, l, lelem, r, rt);
            }
            let Ty::Set(relem) = rt else { unreachable!() };
            return self.set_cmp(op, r, relem, l, lt);
        }

        // Structural `oset` equality (#1328): order-SENSITIVE sequence equality
        // (its insertion order is observable and is why `oset` is distinct from
        // `set`), rather than pointer identity.
        if matches!(op, Eq | Ne) && (matches!(lt, Ty::OSet(_)) || matches!(rt, Ty::OSet(_))) {
            let (dl, le, dr, re) = if let Ty::OSet(le) = lt {
                let Ty::OSet(re) = rt else {
                    return Err(err("an oset compares for equality only with another oset"));
                };
                (l, le, r, re)
            } else {
                let Ty::OSet(re) = rt else { unreachable!() };
                let Ty::OSet(le) = lt else {
                    return Err(err("an oset compares for equality only with another oset"));
                };
                (r, re, l, le)
            };
            if le != re {
                return Err(err(
                    "cannot compare osets with different element types for equality",
                ));
            }
            return self.ordered_seq_cmp(op, dl, le, dr, re, "zap_oset_len", "zap_oset_get");
        }

        // Structural `deque` equality (#1328): order-sensitive, length-sensitive,
        // element-wise (Python `collections.deque` semantics), rather than pointer
        // identity.
        if matches!(op, Eq | Ne) && (matches!(lt, Ty::Deque(_)) || matches!(rt, Ty::Deque(_))) {
            let (dl, le, dr, re) = if let Ty::Deque(le) = lt {
                let Ty::Deque(re) = rt else {
                    return Err(err("a deque compares for equality only with another deque"));
                };
                (l, le, r, re)
            } else {
                let Ty::Deque(re) = rt else { unreachable!() };
                let Ty::Deque(le) = lt else {
                    return Err(err("a deque compares for equality only with another deque"));
                };
                (r, re, l, le)
            };
            return self.ordered_seq_cmp(op, dl, le, dr, re, "zap_deque_len", "zap_deque_get");
        }

        // `heap` has no value equality (#1328): a heap's internal array order is
        // implementation-defined (push order changes the layout), so neither a
        // pointer-identity test (the old behaviour) nor an element-wise array
        // compare is meaningful. Reject `==` / `!=` with a clear message; sema
        // (`arith_result`) reports the same in the normal pipeline, but the ir
        // crate is also exercised without sema, so guard here too. `is` / `is not`
        // (identity) remain available and are handled at the top of `binop`.
        if matches!(op, Eq | Ne) && (matches!(lt, Ty::Heap(_)) || matches!(rt, Ty::Heap(_))) {
            return Err(err(
                "a heap has no value equality: its internal order is implementation-defined, \
                 so `==` / `!=` is undefined; compare a snapshot (e.g. `sorted(h.items())`) \
                 or use `is` / `is not` for identity",
            ));
        }

        // Ordering (`< <= > >=`) on the core collections (#1328 sibling, #1334).
        // These are reference values but not `Ty::Ptr`, so without an explicit arm
        // they would fall through to the generic numeric path below, which
        // `ptrtoint`-compares the two container headers — a meaningless
        // allocation-address comparison. `set` and `deque` have well-defined
        // Python orderings (subset/superset and lexicographic); `dict`, `heap`,
        // and `oset` have none and are rejected. Sema (`arith_result`) reports the
        // rejections in the normal pipeline; the lowering guards here also serve
        // the ir crate, which is exercised without sema. Ordering is NOT
        // symmetric, so operands are never swapped: the collection must be the
        // left operand for a valid comparison.
        if matches!(op, Lt | Le | Gt | Ge) {
            if matches!(lt, Ty::Dict(..)) || matches!(rt, Ty::Dict(..)) {
                return Err(err(
                    "a dict is not orderable: `< <= > >=` are undefined for dicts \
                     (as in Python); compare values explicitly",
                ));
            }
            if matches!(lt, Ty::Heap(_)) || matches!(rt, Ty::Heap(_)) {
                return Err(err("a heap is not orderable: its internal order is \
                     implementation-defined, so `< <= > >=` are undefined; compare \
                     a snapshot (e.g. `sorted(h.items())`)"));
            }
            if matches!(lt, Ty::OSet(_)) || matches!(rt, Ty::OSet(_)) {
                return Err(err(
                    "an oset is not orderable: `< <= > >=` are undefined for osets; \
                     use a set for subset/superset or a list/deque for sequence ordering",
                ));
            }
            if matches!(lt, Ty::Set(_)) || matches!(rt, Ty::Set(_)) {
                let Ty::Set(lelem) = lt else {
                    return Err(err(
                        "a set's `< <= > >=` (subset/superset) compares only with another set",
                    ));
                };
                return self.set_order_cmp(op, l, lelem, r, rt);
            }
            if matches!(lt, Ty::Deque(_)) || matches!(rt, Ty::Deque(_)) {
                let Ty::Deque(lelem) = lt else {
                    return Err(err(
                        "a deque's `< <= > >=` (lexicographic) compares only with another deque",
                    ));
                };
                let Ty::Deque(relem) = rt else {
                    return Err(err(
                        "a deque's `< <= > >=` (lexicographic) compares only with another deque",
                    ));
                };
                return self.deque_order_cmp(op, l, lelem, r, relem);
            }
        }

        // Operator overloading on user structs (D-0047): `a op b` desugars to the
        // matching dunder method, resolved like any struct method call. `in`
        // dispatches on the right operand (`x in a` -> `a.__contains__(x)`); every
        // other operator dispatches on the left.
        if let (Ty::Struct(sid), false) = (lt, matches!(op, In)) {
            return self.struct_binop(op, sid, l, r, rt);
        }
        // Reflected fallback when the LEFT is a built-in (non-struct) and the
        // RIGHT is a user struct defining the reflected dunder (#1180): e.g.
        // `3.0 * v` -> `v.__rmul__(3.0)`. Mirrors sema's built-in-left reflected
        // path; the left-struct case is handled by `struct_binop` above.
        if let (Some(rdunder), Ty::Struct(rsid)) = (reflected_op_dunder(op), rt) {
            if self.struct_has_dunder(rsid, rdunder) {
                return self.call_dunder(rsid, rdunder, vec![(r, rt), (l, lt)]);
            }
        }
        // Reflected COMPARISON fallback when the LEFT is a built-in (non-struct)
        // and the RIGHT is a user struct defining the reflected comparison dunder
        // (#1198): `1 == v` -> `v.__req__(1)`; `1 < v` -> `v.__rcmp__(1) < 0`.
        if let Ty::Struct(rsid) = &rt {
            let rsid = *rsid;
            match op {
                Eq | Ne if self.struct_has_dunder(rsid, "__req__") => {
                    let (bit, _) = self.call_dunder(rsid, "__req__", vec![(r, rt), (l, lt)])?;
                    if matches!(op, Ne) {
                        let d = self.new_value(Ty::I1);
                        self.emit(Inst::Un {
                            dest: d,
                            op: UnOp::Not,
                            val: bit,
                        });
                        return Ok((d, Ty::I1));
                    }
                    return Ok((bit, Ty::I1));
                }
                Lt | Le | Gt | Ge if self.struct_has_dunder(rsid, "__rcmp__") => {
                    let (cmp, _) = self.call_dunder(rsid, "__rcmp__", vec![(r, rt), (l, lt)])?;
                    let pred = match op {
                        Lt => CmpPred::Lt,
                        Le => CmpPred::Le,
                        Gt => CmpPred::Gt,
                        Ge => CmpPred::Ge,
                        _ => unreachable!(),
                    };
                    let zero = self.iconst(Ty::I64, 0);
                    let d = self.new_value(Ty::I1);
                    self.emit(Inst::ICmp {
                        dest: d,
                        pred,
                        lhs: cmp,
                        rhs: zero,
                    });
                    return Ok((d, Ty::I1));
                }
                _ => {}
            }
        }
        if matches!(op, In) {
            if let Ty::Struct(sid) = rt {
                let contains = format!("{}__{}", self.ctx.names[sid as usize], "__contains__");
                if self.sigs.contains_key(&contains) {
                    let (ret, rty) =
                        self.call_dunder(sid, "__contains__", vec![(r, rt), (l, lt)])?;
                    return Ok((ret, rty));
                }
                // No `__contains__`: membership falls back to iterating the
                // container and comparing each element for equality (D-0048).
                let result = self.alloca(Ty::I1);
                let f = self.new_value(Ty::I1);
                self.emit(Inst::BConst {
                    dest: f,
                    val: false,
                });
                self.store(result, f);
                self.drive_iteration(r, sid, |s, ev, ety| {
                    let (eq, _) = s.binop(AstBin::Eq, ev, ety, l, lt)?;
                    let then_bb = s.new_block();
                    let cont_bb = s.new_block();
                    s.set_term(Term::BrIf {
                        cond: eq,
                        then_bb,
                        else_bb: cont_bb,
                    });
                    s.position(then_bb);
                    let t = s.new_value(Ty::I1);
                    s.emit(Inst::BConst { dest: t, val: true });
                    s.store(result, t);
                    let brk = s.loops.last().expect("in-iteration loop").brk;
                    s.set_term(Term::Br(brk));
                    s.position(cont_bb);
                    Ok(())
                })?;
                let d = self.load(result, Ty::I1);
                return Ok((d, Ty::I1));
            }
        }

        // Substring membership: `sub in s` -> `zap_str_contains` (ticket #284).
        // Both operands lower to `Ptr` (str); sema has checked the types.
        if matches!(op, In) && lt == Ty::Ptr && rt == Ty::Ptr {
            let raw = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(raw),
                func: "zap_str_contains".to_string(),
                args: vec![r, l],
            });
            let zero = self.iconst(Ty::I64, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: raw,
                rhs: zero,
            });
            return Ok((d, Ty::I1));
        }

        // Key membership: `k in <dict>` -> `zap_dict_contains_*`, normalized to a
        // bool (ticket #299). The key kind selects the entry point; a struct key
        // passes its `__hash__`/`__eq__` as the trailing function pointers.
        if matches!(op, In) {
            if let Ty::Dict(kk, _vk) = rt {
                // Borrowed-bytes probe (#1082): `v in d` with a `strview`/`bytesview`
                // key and a str/bytes-keyed dict looks up the borrowed bytes
                // directly — no owned key alloc, no root.
                if matches!(kk, Elem::Ptr | Elem::Bytes) {
                    if let Ty::Struct(sid) = lt {
                        if self.is_any_view_sid(sid) {
                            let (base, off, len) = self.view_fields(l, sid);
                            let word = self.new_value(Ty::I64);
                            self.emit(Inst::Call {
                                dest: Some(word),
                                func: "zap_dict_contains_strview".to_string(),
                                args: vec![r, base, off, len],
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
                    }
                }
                let l = self.coerce(l, lt, kk.ty());
                let kfns = self.key_fn_args(kk)?;
                let word = self.new_value(Ty::I64);
                let mut cargs = vec![r, l];
                cargs.extend_from_slice(&kfns);
                // A float key canonicalizes + NaN-rejects in the runtime (#803), so
                // `k in d` may raise a catchable ValueError on a NaN key.
                self.key_rt_call(
                    Some(word),
                    format!("zap_dict_contains_{}", kk.key_suffix()),
                    cargs,
                    kk,
                );
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
        }

        // List membership: `x in <list>` — a list has no hash index, so this is a
        // linear scan comparing each element to `x` for equality (ticket #399). The
        // comparison routes through `binop(Eq, ..)`, so elements use the same
        // equality semantics as `==` (numeric joins, `str` by contents, a struct's
        // `__eq__`). Lowered in-IR like a `for` loop, breaking on the first match.
        if matches!(op, In) {
            if let Ty::List(elem) = rt {
                let result = self.alloca(Ty::I1);
                let f = self.new_value(Ty::I1);
                self.emit(Inst::BConst {
                    dest: f,
                    val: false,
                });
                self.store(result, f);
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
                    args: vec![r],
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
                    args: vec![r, i],
                });
                let (eq, _) = self.binop(AstBin::Eq, e, elem.ty(), l, lt)?;
                let found_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: eq,
                    then_bb: found_bb,
                    else_bb: incr_bb,
                });

                self.position(found_bb);
                let t = self.new_value(Ty::I1);
                self.emit(Inst::BConst { dest: t, val: true });
                self.store(result, t);
                self.set_term(Term::Br(end_bb));

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
                let d = self.load(result, Ty::I1);
                return Ok((d, Ty::I1));
            }
        }

        // Membership: `x in <oset>` (#983) -> `zap_oset_contains_*`, bool.
        if matches!(op, In) {
            if let Ty::OSet(elem) = rt {
                if value_elem_kind(lt, &self.ctx.nested)? != elem {
                    return Err(err("'in' element has the wrong type for this oset"));
                }
                let l = self.coerce(l, lt, elem.ty());
                let word = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(word),
                    func: format!("zap_oset_contains_{}", elem.val_suffix()),
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
        }

        // Membership: `x in <bytes>` (#1355). An `int` left operand tests
        // byte-value membership — the byte values `for b in data` yields,
        // 0..=255 — via a linear scan reading each byte with `zap_bytes_index`
        // (so `85 in b"UTC"` is true, `0 in b"UTC"` false). A `bytes` left
        // operand tests subsequence membership via `zap_str_contains`, a
        // byte-level search (bytes share `str`'s buffer). `not in` reuses this
        // and negates.
        if matches!(op, In) && rt == Ty::Bytes {
            if lt == Ty::Bytes {
                let raw = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: "zap_str_contains".to_string(),
                    args: vec![r, l],
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: CmpPred::Ne,
                    lhs: raw,
                    rhs: zero,
                });
                return Ok((d, Ty::I1));
            }
            if !matches!(
                lt,
                Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64
            ) {
                return Err(err(
                    "'in' on bytes expects an int (a byte value) or bytes (a subsequence) \
                     on the left",
                ));
            }
            let needle = self.coerce(l, lt, Ty::I64);
            let result = self.alloca(Ty::I1);
            let f = self.new_value(Ty::I1);
            self.emit(Inst::BConst {
                dest: f,
                val: false,
            });
            self.store(result, f);
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
                func: "zap_str_len".to_string(),
                args: vec![r],
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
            let byte = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(byte),
                func: "zap_bytes_index".to_string(),
                args: vec![r, i],
            });
            let eq = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: eq,
                pred: CmpPred::Eq,
                lhs: byte,
                rhs: needle,
            });
            let found_bb = self.new_block();
            self.set_term(Term::BrIf {
                cond: eq,
                then_bb: found_bb,
                else_bb: incr_bb,
            });

            self.position(found_bb);
            let t = self.new_value(Ty::I1);
            self.emit(Inst::BConst { dest: t, val: true });
            self.store(result, t);
            self.set_term(Term::Br(end_bb));

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
            let d = self.load(result, Ty::I1);
            return Ok((d, Ty::I1));
        }

        // Membership: `x in <set>` -> `zap_set_contains_*`, normalized to a bool.
        if matches!(op, In) {
            let Ty::Set(elem) = rt else {
                return Err(err("'in' is only supported with a set on the right"));
            };
            if elem_kind(lt, &self.ctx.nested)? != elem {
                return Err(err("'in' element has the wrong type for this set"));
            }
            let l = self.coerce(l, lt, elem.ty());
            let kfns = self.key_fn_args(elem)?;
            let word = self.new_value(Ty::I64);
            let mut cargs = vec![r, l];
            cargs.extend_from_slice(&kfns);
            // A float member canonicalizes + NaN-rejects in the runtime (#803), so
            // `x in s` may raise a catchable ValueError on a NaN member.
            self.key_rt_call(
                Some(word),
                format!("zap_set_contains_{}", elem.key_suffix()),
                cargs,
                elem,
            );
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

        // `bytes` operands (I#337): `+` (concatenation -> bytes), `==`/`!=`
        // (content equality), and ordering `< <= > >=` (byte-wise), reusing the
        // `ZapStr` byte-wise runtime. Both operands share the physical repr, so
        // the comparison helpers are identical to `str`'s; `+` yields `bytes`.
        // Sema has checked that both sides are `bytes`.
        if lt == Ty::Bytes && rt == Ty::Bytes {
            if matches!(op, Add) {
                let d = self.new_value(Ty::Bytes);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_concat".to_string(),
                    args: vec![l, r],
                });
                return Ok((d, Ty::Bytes));
            }
            if matches!(op, Eq | Ne) {
                let raw = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: "zap_str_eq".to_string(),
                    args: vec![l, r],
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: if matches!(op, Eq) {
                        CmpPred::Ne
                    } else {
                        CmpPred::Eq
                    },
                    lhs: raw,
                    rhs: zero,
                });
                return Ok((d, Ty::I1));
            }
            if matches!(op, Lt | Le | Gt | Ge) {
                let raw = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: "zap_str_cmp".to_string(),
                    args: vec![l, r],
                });
                let zero = self.iconst(Ty::I64, 0);
                let pred = match op {
                    Lt => CmpPred::Lt,
                    Le => CmpPred::Le,
                    Gt => CmpPred::Gt,
                    Ge => CmpPred::Ge,
                    _ => unreachable!(),
                };
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred,
                    lhs: raw,
                    rhs: zero,
                });
                return Ok((d, Ty::I1));
            }
            return Err(err(
                "unsupported operation on bytes (bytes supports +, ==, !=, <, <=, >, >=)",
            ));
        }

        // Reference-typed operands: `str` (lowered to `Ptr`) supports `+`
        // (concatenation), `==`/`!=` (content equality), and ordering
        // `< <= > >=` (lexicographic by codepoint == byte-wise for UTF-8,
        // D-0025). Everything else on a reference type is unsupported.
        if matches!(lt, Ty::Ptr) || matches!(rt, Ty::Ptr) {
            if lt == Ty::Ptr && rt == Ty::Ptr {
                if matches!(op, Add) {
                    let d = self.new_value(Ty::Ptr);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_str_concat".to_string(),
                        args: vec![l, r],
                    });
                    return Ok((d, Ty::Ptr));
                }
                if matches!(op, Eq | Ne) {
                    // `zap_str_eq` returns 1/0; `!=` negates via comparing to 0.
                    let raw = self.new_value(Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(raw),
                        func: "zap_str_eq".to_string(),
                        args: vec![l, r],
                    });
                    let zero = self.iconst(Ty::I64, 0);
                    let d = self.new_value(Ty::I1);
                    self.emit(Inst::ICmp {
                        dest: d,
                        pred: if matches!(op, Eq) {
                            CmpPred::Ne
                        } else {
                            CmpPred::Eq
                        },
                        lhs: raw,
                        rhs: zero,
                    });
                    return Ok((d, Ty::I1));
                }
                if matches!(op, Lt | Le | Gt | Ge) {
                    // `zap_str_cmp` returns -1/0/1; compare it against 0.
                    let raw = self.new_value(Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(raw),
                        func: "zap_str_cmp".to_string(),
                        args: vec![l, r],
                    });
                    let zero = self.iconst(Ty::I64, 0);
                    let pred = match op {
                        Lt => CmpPred::Lt,
                        Le => CmpPred::Le,
                        Gt => CmpPred::Gt,
                        Ge => CmpPred::Ge,
                        _ => unreachable!(),
                    };
                    let d = self.new_value(Ty::I1);
                    self.emit(Inst::ICmp {
                        dest: d,
                        pred,
                        lhs: raw,
                        rhs: zero,
                    });
                    return Ok((d, Ty::I1));
                }
            }
            return Err(err(
                "unsupported operation on a reference type (str supports +, ==, !=, <, <=, >, >=)",
            ));
        }

        let is_float = lt.is_float() || rt.is_float();
        // Unsigned integer arithmetic/comparison promotes to `uint64` and wraps
        // modulo 2^64 (#552), mirroring the signed tower's promotion to `int64`;
        // a narrower unsigned result comes from an explicit `uintN(...)` cast.
        // `int_ty` is the integer width both operands are widened to, and its
        // signedness is what codegen reads to pick `udiv`/`urem`/`lshr`/unsigned
        // `icmp`. A mixed signed/unsigned pairing was rejected by sema (ZAP4015),
        // so at most one sign is in play here.
        let unsigned = lt.is_unsigned() || rt.is_unsigned();
        let int_ty = if unsigned { Ty::U64 } else { Ty::I64 };

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
                // Integer comparisons are done at a common 64-bit width so
                // mixed-width operands (e.g. `int32 < int64`, `uint8 < uint32`,
                // or a literal against an int32) share a type. Codegen reads the
                // operand type's sign to choose signed vs unsigned `icmp` (#552).
                let li = self.coerce(l, lt, int_ty);
                let ri = self.coerce(r, rt, int_ty);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred,
                    lhs: li,
                    rhs: ri,
                });
            }
            return Ok((d, Ty::I1));
        }

        match op {
            Add | Sub | Mul => {
                let irop = match op {
                    Add => BinOp::Add,
                    Sub => BinOp::Sub,
                    Mul => BinOp::Mul,
                    _ => unreachable!(),
                };
                if is_float {
                    // Float arithmetic computes at f64 (operands widened), per
                    // the numeric join in sema (any float operand -> float64).
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
                    // Integer arithmetic computes at a 64-bit width (operands
                    // widened), so `int32`/`uint8`/untyped-literal operands share
                    // a width. Add/Sub/Mul are bit-identical for signed and
                    // unsigned; `int_ty` carries the sign onward so a later
                    // narrowing cast or render picks the right behaviour (#552).
                    let li = self.coerce(l, lt, int_ty);
                    let ri = self.coerce(r, rt, int_ty);
                    let d = self.new_value(int_ty);
                    self.emit(Inst::Bin {
                        dest: d,
                        op: irop,
                        lhs: li,
                        rhs: ri,
                    });
                    Ok((d, int_ty))
                }
            }
            Mod => {
                // Modulo (D-0024) takes the sign of the divisor (Python
                // floored modulo), consistent with floored `//` so that
                // `a == (a // b) * b + (a % b)` holds. Both floats and integers
                // go through a runtime helper that corrects C's truncated
                // remainder (`fmod`/`%`), mirroring `zap_ffloordiv`/`zap_ifloordiv`
                // (#374/#751). Modulo by zero is a catchable ZeroDivisionError
                // (EXC-3, D-0007), so the call is a `throwing_call` (an `invoke`
                // inside a `try`) rather than a plain `call` that could not reach
                // the landing pad.
                if is_float {
                    let lf = self.as_f64(l, lt);
                    let rf = self.as_f64(r, rt);
                    let d = self.new_value(Ty::F64);
                    self.throwing_call(Some(d), "zap_ffmod", vec![lf, rf]);
                    Ok((d, Ty::F64))
                } else {
                    let li = self.coerce(l, lt, int_ty);
                    let ri = self.coerce(r, rt, int_ty);
                    let d = self.new_value(int_ty);
                    // Modulo by zero is a catchable built-in fault (EXC-3,
                    // D-0007): the helper raises through `zap_panic`, so this is
                    // a `throwing_call` (an `invoke` inside a `try`) rather than
                    // a plain `call`, which could not reach the landing pad. The
                    // unsigned path uses `urem` (no floored-sign correction —
                    // both operands are non-negative), the signed path the
                    // floored `zap_imod` (#552).
                    let helper = if unsigned { "zap_umod" } else { "zap_imod" };
                    self.throwing_call(Some(d), helper, vec![li, ri]);
                    Ok((d, int_ty))
                }
            }
            FloorDiv => {
                // Floor division (D-0024) rounds toward negative infinity,
                // unlike LLVM `sdiv` (toward zero) / `fdiv` (no rounding). The
                // toward-negative-infinity adjustment lives in a runtime helper
                // so codegen stays mechanical and the semantics are in one place.
                if is_float {
                    let lf = self.as_f64(l, lt);
                    let rf = self.as_f64(r, rt);
                    let d = self.new_value(Ty::F64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_ffloordiv".to_string(),
                        args: vec![lf, rf],
                    });
                    Ok((d, Ty::F64))
                } else {
                    let li = self.coerce(l, lt, int_ty);
                    let ri = self.coerce(r, rt, int_ty);
                    let d = self.new_value(int_ty);
                    // Divide by zero is a catchable built-in fault (EXC-3,
                    // D-0007): the helper raises through `zap_panic`, so this is
                    // a `throwing_call` (an `invoke` inside a `try`) rather than
                    // a plain `call` that could not reach the pad. Unsigned `//`
                    // is a plain `udiv` (floor == truncation for non-negative
                    // operands); signed `//` floors toward -inf (#552).
                    let helper = if unsigned {
                        "zap_udiv"
                    } else {
                        "zap_ifloordiv"
                    };
                    self.throwing_call(Some(d), helper, vec![li, ri]);
                    Ok((d, int_ty))
                }
            }
            Pow => {
                // `**` over the numeric tower (D-0019). A floating-point base or
                // exponent goes through libm `pow` (f64); the all-integer case
                // uses a binary-exponentiation runtime helper and keeps an
                // integer result per the static type. A negative exponent has no
                // integer result, so `zap_ipow` raises a catchable `ValueError`
                // (#374/B2) rather than silently flooring to 0; use a float base
                // (`2.0 ** -1`) for a real negative power.
                if is_float {
                    let lf = self.as_f64(l, lt);
                    let rf = self.as_f64(r, rt);
                    let d = self.new_value(Ty::F64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_powf".to_string(),
                        args: vec![lf, rf],
                    });
                    Ok((d, Ty::F64))
                } else {
                    let li = self.coerce(l, lt, int_ty);
                    let ri = self.coerce(r, rt, int_ty);
                    let d = self.new_value(int_ty);
                    // A negative exponent raises a catchable `ValueError`
                    // (zap_ipow -> zap_panic, #374/B2), so this must be an
                    // unwinding `invoke` that reaches the `try` landing pad,
                    // like int()/float() parse and the div/mod faults. The
                    // unsigned exponent is never negative, so `zap_upow` cannot
                    // raise; it wraps modulo 2^64 like the signed path (#552).
                    let helper = if unsigned { "zap_upow" } else { "zap_ipow" };
                    self.throwing_call(Some(d), helper, vec![li, ri]);
                    Ok((d, int_ty))
                }
            }
            Div => {
                let lf = self.as_f64(l, lt);
                let rf = self.as_f64(r, rt);
                let d = self.new_value(Ty::F64);
                // True division by zero raises a catchable ZeroDivisionError
                // (#374). The `Expr::Binary` lowerer emits a plain inline `fdiv`
                // when the divisor is a known-nonzero literal, so reaching here
                // means the divisor is not a safe constant — guard it through
                // the throwing helper (an `invoke` under a handler).
                self.throwing_call(Some(d), "zap_fdiv", vec![lf, rf]);
                Ok((d, Ty::F64))
            }
            BitAnd | BitOr | BitXor => {
                let irop = match op {
                    BitAnd => BinOp::And,
                    BitOr => BinOp::Or,
                    BitXor => BinOp::Xor,
                    _ => unreachable!(),
                };
                let li = self.coerce(l, lt, int_ty);
                let ri = self.coerce(r, rt, int_ty);
                let d = self.new_value(int_ty);
                self.emit(Inst::Bin {
                    dest: d,
                    op: irop,
                    lhs: li,
                    rhs: ri,
                });
                Ok((d, int_ty))
            }
            Shl | Shr => {
                // A shift count that is negative (-> ValueError, CPython) or
                // >= 64 (-> defined "shifted out", not LLVM poison) is validated
                // by a runtime helper (#374/#740). A negative count can raise, so
                // this is a throwing_call. The `Expr::Binary` lowerer emits a
                // plain inline shift when the count is a literal in 0..63.
                let li = self.coerce(l, lt, int_ty);
                let ri = self.coerce(r, rt, int_ty);
                let d = self.new_value(int_ty);
                // Left shift is bit-identical for both signs (`zap_ishl`); right
                // shift differs — unsigned uses a *logical* shift (`zap_ushr`,
                // zero-fill), signed an arithmetic one (`zap_ishr`, sign-fill)
                // (#552). Both validate the count (negative -> ValueError).
                let helper = if matches!(op, Shl) {
                    "zap_ishl"
                } else if unsigned {
                    "zap_ushr"
                } else {
                    "zap_ishr"
                };
                self.throwing_call(Some(d), helper, vec![li, ri]);
                Ok((d, int_ty))
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
            // `@` on arrays is intercepted at the top of `binop`; on any other
            // operand type sema has already raised a "no such operator" error.
            MatMul => Err(err(
                "operator '@' (matrix multiply) is only defined for arrays",
            )),
            In => Err(err("'in' is not supported by codegen yet")),
            NotIn => unreachable!("'not in' is rewritten to a negated 'in' in binop()"),
            // `Send` is intercepted at the top of `binop` (before the generic
            // reference-type guard), so it never reaches here.
            Send => unreachable!("channel send handled above"),
            // `is` / `is not` (#611) are intercepted at the top of `binop`.
            Is | IsNot => unreachable!("'is'/'is not' handled above"),
            Eq | Ne | Lt | Le | Gt | Ge => unreachable!(),
        }
    }
}
