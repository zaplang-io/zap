//! Collection-key hash/eq machinery (dict keys, set members).
//!
//! The runtime key-argument plan and per-element `__hash__`/`__eq__` emission
//! for hashable collection elements — scalars, strings, floats (#800/#803),
//! tuples (#680), optionals, enums, and struct keys (D-0047, which carry their
//! derived/hand-written dunder function pointers). Extracted verbatim from
//! `lower.rs` (#1134); `use super::*` re-exposes the parent scope.

use super::*;

impl<'a> FnLower<'a> {
    /// The extra runtime arguments for a collection *key* of the given element
    /// kind: for a struct key (D-0047), the `__hash__` and `__eq__` function
    /// pointers the `_fn` entry points take; for any other key, none. A struct
    /// key without both dunders is a clean error (needs `@derive(Hash, Eq)` or
    /// hand-written methods).
    pub(super) fn key_fn_args(&mut self, elem: Elem) -> Result<Vec<Value>, LowerError> {
        // A nested collection (#625) is unhashable: a `list`/`dict`/`set` cannot be
        // a dict key or a set element. Without this guard it would silently take
        // the pointer-key (`str`) path and hash the collection's pointer as if it
        // were a `ZapStr` — a silent miscompile. Fail loudly instead.
        if let Elem::Nested(id) = elem {
            let ty = self.ctx.nested.borrow().get(id);
            return Err(err(format!(
                "a {ty} is unhashable and cannot be used as a dict key or a set \
                 element (collections have no stable hash)"
            )));
        }
        // A variadic `tuple[T]` key (#1474): hashable iff its element kind is
        // (word / f64 / str). Synthesize a per-element-kind `__hash__`/`__eq__`
        // wrapper pair over `zap_vartuple_hash`/`zap_vartuple_eq` and route it
        // through the same fn-pointer key path as a tuple/struct key.
        if let Elem::VarTuple(id) = elem {
            let ty = self.ctx.nested.borrow().get(id);
            let inner_elem = match ty {
                Ty::VarTuple(e) => e,
                _ => {
                    return Err(err(
                        "internal: a VarTuple element did not resolve to tuple[T]",
                    ))
                }
            };
            let tag = vartuple_elem_tag(inner_elem).ok_or_else(|| {
                err(
                    "this tuple[T] is not hashable, so it cannot be a dict key or set \
                     element: only tuples of int/bool/char/enum, float, or str/bytes are \
                     hashable yet (#1474)",
                )
            })?;
            let (hashn, eqn) = self.vartuple_key_fns(tag)?;
            let hv = self.new_value(Ty::Ptr);
            self.emit(Inst::FuncAddr {
                dest: hv,
                func: hashn,
            });
            let ev = self.new_value(Ty::Ptr);
            self.emit(Inst::FuncAddr {
                dest: ev,
                func: eqn,
            });
            return Ok(vec![hv, ev]);
        }
        let Elem::Struct(sid) = elem else {
            return Ok(vec![]);
        };
        // A tuple key (#680, D-0049): tuples intern as struct ids at or past
        // `TupleInterner.base` (= `names.len()`), so they have no `ctx.names`
        // entry — indexing `names[sid]` here was the ICE. A tuple is hashable
        // exactly when all its elements are (CPython parity), so synthesize its
        // structural `__hash__`/`__eq__` on demand and route it through the same
        // function-pointer key path as a named `@derive(Hash, Eq)` struct.
        let (hashn, eqn) = if self.ctx.tuples.borrow().is_tuple(sid) {
            self.tuple_key_fns(sid)?
        } else if optional_layout(self.ctx, Ty::Struct(sid)).is_some() {
            // An `Optional[T]` key (#704): `None` and a present value are both
            // hashable (CPython parity). Optional lowers as a tagged/niche enum
            // value, so — like a tuple — synthesize its structural
            // `__hash__`/`__eq__` and route it through the fn-key path. A
            // sentinel hash for `None`, delegation to `T` for `Some`.
            self.optional_key_fns(sid)?
        } else if self
            .ctx
            .enums
            .values()
            .any(|e| e.is_sum && e.sid == Some(sid))
        {
            // A tier-3 (data-carrying) enum key (D-0139/#809): a tagged value
            // aggregate, hashable iff every payload field is — synthesize its
            // structural `__hash__`/`__eq__` on demand (tag switch + per-variant
            // payload fold) and route it through the fn-key path, like a tuple.
            self.enum_key_fns(sid)?
        } else {
            let name = self.ctx.names[sid as usize].clone();
            let hashn = format!("{name}__{}", "__hash__");
            let eqn = format!("{name}__{}", "__eq__");
            if !self.sigs.contains_key(&hashn) {
                // A `value struct` without `__hash__` did not auto-derive
                // (D-0139/#808) because some field is not hashable; name that
                // field so the diagnostic points at the real cause, not a generic
                // "add @derive". (A reference struct keeps the plain message —
                // identity is its default and value-equality is an opt-in.)
                if self.ctx.is_value_struct(sid) {
                    let flds = self.ctx.fields[sid as usize].clone();
                    if let Some((fname, fty)) = flds
                        .iter()
                        .find(|(_, fty)| self.elem_key_plan(*fty).is_err())
                    {
                        return Err(err(format!(
                            "value struct '{name}' is not hashable, so it cannot be a dict key \
                             or set element: field '{fname}' of type '{fty}' is not hashable \
                             (D-0139/#808). A value struct auto-derives Eq/Hash only when every \
                             field is hashable — a mutable collection (list/dict/set) or a \
                             reference struct without '__hash__' is not."
                        )));
                    }
                }
                return Err(err(format!(
                    "type '{name}' needs '__hash__' to be a dict/set key \
                     (use @derive(Hash) or define it; D-0047)"
                )));
            }
            if !self.sigs.contains_key(&eqn) {
                return Err(err(format!(
                    "type '{name}' needs '__eq__' to be a dict/set key \
                     (use @derive(Eq) or define it; D-0047)"
                )));
            }
            (hashn, eqn)
        };
        let hv = self.new_value(Ty::Ptr);
        self.emit(Inst::FuncAddr {
            dest: hv,
            func: hashn,
        });
        let ev = self.new_value(Ty::Ptr);
        self.emit(Inst::FuncAddr {
            dest: ev,
            func: eqn,
        });
        Ok(vec![hv, ev])
    }

    /// Synthesize (once per tuple id, #680 / D-0049) the structural `__hash__`
    /// and `__eq__` for a tuple used as a dict key or set element, returning
    /// their symbol names. A tuple is an anonymous value-struct aggregate, so this
    /// mirrors `@derive(Hash, Eq)` over the interned element layout. The
    /// `__hash__(t) -> i64` seeds `0` and folds each element's hash in with
    /// `zap_hash_combine` — `zap_i64_hash` for integer/bool/char/enum words,
    /// `zap_str_hash` for `str`/`bytes`, and the element's own `__hash__` for a
    /// nested tuple or hashable struct — while `__eq__(a, b) -> i1` ANDs the
    /// per-element comparisons element-for-element.
    ///
    /// A tuple is hashable exactly when every element is (CPython parity); an
    /// unhashable element (float, a nested collection, or a struct lacking
    /// `__hash__`/`__eq__`) is rejected with a clean coded diagnostic — never an
    /// ICE. The generated functions are appended to [`StructCtx::generated`] and
    /// referenced by `Inst::FuncAddr`, like the named-struct key path; they need
    /// no signature registration (they are only ever called indirectly by the
    /// collection runtime). Reused across every key use of the same tuple layout.
    /// Emit the hash of one field/element given its [`ElemKey`] plan, returning an
    /// `i64`. Shared by the tuple key path, the `Optional` key path, and the
    /// `@derive`/auto-derived struct `__hash__` (D-0139/#808), so every structural
    /// hash folds its fields the same way. `fty` is the field's own type (needed to
    /// widen an integer / `f32` before hashing).
    pub(super) fn emit_elem_hash(&mut self, fv: Value, fty: Ty, key: &ElemKey) -> Value {
        match key {
            ElemKey::Str => {
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_hash".to_string(),
                    args: vec![fv],
                });
                d
            }
            ElemKey::Int => {
                let i = self.coerce(fv, fty, Ty::I64);
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_i64_hash".to_string(),
                    args: vec![i],
                });
                d
            }
            ElemKey::Float(w) => {
                // Widen `f32` to the `double` the key-word helper takes, then
                // canonicalize `-0.0` / reject `NaN` (#803) and hash the word.
                let f = if *w == Ty::F32 {
                    self.coerce(fv, Ty::F32, Ty::F64)
                } else {
                    fv
                };
                let word = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(word),
                    func: "zap_f64_value_key_word".to_string(),
                    args: vec![f],
                });
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_i64_hash".to_string(),
                    args: vec![word],
                });
                d
            }
            ElemKey::Struct(ch, _) => {
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: ch.clone(),
                    args: vec![fv],
                });
                d
            }
        }
    }

    /// Emit the equality of one field/element pair given its [`ElemKey`] plan,
    /// returning an `i1`. The float arm uses `FCmp Eq` (so `-0.0 == 0.0` and
    /// `NaN != NaN`), consistent with the canonical hash. Shared by the same three
    /// structural paths as [`Self::emit_elem_hash`].
    pub(super) fn emit_elem_eq(&mut self, a: Value, c: Value, key: &ElemKey) -> Value {
        match key {
            ElemKey::Str => {
                let word = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(word),
                    func: "zap_str_eq".to_string(),
                    args: vec![a, c],
                });
                let zero = self.iconst(Ty::I64, 0);
                let bit = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: bit,
                    pred: CmpPred::Ne,
                    lhs: word,
                    rhs: zero,
                });
                bit
            }
            ElemKey::Int => {
                let bit = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: bit,
                    pred: CmpPred::Eq,
                    lhs: a,
                    rhs: c,
                });
                bit
            }
            ElemKey::Float(_) => {
                let bit = self.new_value(Ty::I1);
                self.emit(Inst::FCmp {
                    dest: bit,
                    pred: CmpPred::Eq,
                    lhs: a,
                    rhs: c,
                });
                bit
            }
            ElemKey::Struct(_, ce) => {
                let bit = self.new_value(Ty::I1);
                self.emit(Inst::Call {
                    dest: Some(bit),
                    func: ce.clone(),
                    args: vec![a, c],
                });
                bit
            }
        }
    }

    /// Lower an explicit capability-dunder call (`recv.__cmp__(x)` /
    /// `recv.__eq__(x)` / `recv.__hash__()`, D-0139) on a value `recv` of an
    /// intrinsic type `bty` (a scalar word — ints, `bool`, `char`, a tier-1 enum —
    /// `bytes`, or a `float`). It emits the SAME primitive the operator surface and
    /// the dict/set key path already use (via [`Self::elem_key_plan`] /
    /// [`Self::emit_elem_hash`] / [`Self::emit_elem_eq`] / [`gen_field_cmp`]), so
    /// the method and operator surfaces agree for every type a capability admits
    /// (I#820). Reached from the method-call path when a bounded generic parameter
    /// `[K: Comparable]`/`[K: Hashable]` is monomorphized to such a type and the
    /// body names the dunder; a scalar has no method symbol to dispatch to, which
    /// is why the plain struct-method path cannot handle it. `__cmp__` returns the
    /// `i64` three-way (−1/0/+1), `__eq__` a `bool`, `__hash__` an `i64` — matching
    /// the dunders' D-0047 signatures.
    pub(super) fn lower_intrinsic_capability_dunder(
        &mut self,
        recv: Value,
        bty: Ty,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        // The receiver's key classification: `Int` (ints/bool/char/tier-1 enum),
        // `Str` (`bytes`), or `Float`. A `Struct` plan cannot arise here (the
        // caller gates on intrinsic `bty`), so anything else is a bug.
        let key = self.elem_key_plan(bty)?;
        match name {
            "__hash__" => {
                if !args.is_empty() {
                    return Err(err("__hash__() takes no arguments (D-0047)"));
                }
                let h = self.emit_elem_hash(recv, bty, &key);
                Ok((h, Ty::I64))
            }
            "__eq__" | "__cmp__" => {
                if args.len() != 1 {
                    return Err(err(format!(
                        "{name}(self, other) takes exactly one argument (D-0047)"
                    )));
                }
                // The argument is the `other` operand; it has the same intrinsic
                // type as the receiver, so coerce it to `bty` and compare.
                let (av, at) = self.expr(&args[0].value)?;
                let other = self.coerce(av, at, bty);
                if name == "__eq__" {
                    let bit = self.emit_elem_eq(recv, other, &key);
                    Ok((bit, Ty::I1))
                } else {
                    let c = match &key {
                        // `bytes`/`str`: lexicographic byte compare, the same
                        // `gen_field_cmp` routes a `Ptr` field through.
                        ElemKey::Str => {
                            let d = self.new_value(Ty::I64);
                            self.emit(Inst::Call {
                                dest: Some(d),
                                func: "zap_str_cmp".to_string(),
                                args: vec![recv, other],
                            });
                            d
                        }
                        // Ints/bool/char/tier-1 enum and floats: the shared
                        // three-way compare (coerces integer widths to i64; floats
                        // via `FCmp`).
                        _ => gen_field_cmp(self, recv, other, bty)?,
                    };
                    Ok((c, Ty::I64))
                }
            }
            other => Err(err(format!(
                "'{other}' is not a capability method on an intrinsic value"
            ))),
        }
    }

    /// Synthesize the `tuple[T]` key `(__hash__, __eq__)` wrapper pair for an
    /// element-kind `tag` (#1474), memoized module-wide. Each is a one-call
    /// trampoline to the runtime (`zap_vartuple_hash`/`zap_vartuple_eq`) with the
    /// kind baked in, so it matches the fn-pointer key calling convention
    /// (`hash(ptr) -> i64`, `eq(ptr, ptr) -> i1`) the dict/set key path expects.
    fn vartuple_key_fns(&mut self, tag: i64) -> Result<(String, String), LowerError> {
        if let Some(pair) = self.ctx.synth_vartuple_keyfns.borrow().get(&tag) {
            return Ok(pair.clone());
        }
        let hashn = format!("$vt${tag}$hash");
        let eqn = format!("$vt${tag}$eq");
        {
            let mut b = FnLower::new(
                hashn.clone(),
                vec![Ty::Ptr],
                Ty::I64,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            let kind = b.iconst(Ty::I32, tag);
            let d = b.new_value(Ty::I64);
            b.emit(Inst::Call {
                dest: Some(d),
                func: "zap_vartuple_hash".to_string(),
                args: vec![0, kind],
            });
            b.set_term(Term::Ret(Some(d)));
            self.ctx.generated.borrow_mut().push(b.func);
        }
        {
            let mut b = FnLower::new(
                eqn.clone(),
                vec![Ty::Ptr, Ty::Ptr],
                Ty::I1,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            let kind = b.iconst(Ty::I32, tag);
            let raw = b.new_value(Ty::I64);
            b.emit(Inst::Call {
                dest: Some(raw),
                func: "zap_vartuple_eq".to_string(),
                args: vec![0, 1, kind],
            });
            let zero = b.iconst(Ty::I64, 0);
            let d = b.new_value(Ty::I1);
            b.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: raw,
                rhs: zero,
            });
            b.set_term(Term::Ret(Some(d)));
            self.ctx.generated.borrow_mut().push(b.func);
        }
        let pair = (hashn, eqn);
        self.ctx
            .synth_vartuple_keyfns
            .borrow_mut()
            .insert(tag, pair.clone());
        Ok(pair)
    }

    fn tuple_key_fns(&mut self, sid: u32) -> Result<(String, String), LowerError> {
        if let Some(pair) = self.ctx.synth_keyfns.borrow().get(&sid) {
            return Ok(pair.clone());
        }
        let fields = self
            .ctx
            .tuples
            .borrow()
            .fields(sid)
            .expect("tuple_key_fns called on a non-tuple struct id");
        // Resolve each element's per-element key handling first, so an unhashable
        // element fails cleanly before any function is emitted. For a nested
        // aggregate element this recurses to generate that element's key
        // functions; for a named struct it requires derived/defined dunders.
        let plan: Vec<ElemKey> = fields
            .iter()
            .map(|fty| self.elem_key_plan(*fty))
            .collect::<Result<_, _>>()?;

        let hashn = format!("$tuple${sid}$hash");
        let eqn = format!("$tuple${sid}$eq");

        // __hash__(tuple) -> i64: fold per-element hashes, like gen_hash_method.
        {
            let mut b = FnLower::new(
                hashn.clone(),
                vec![Ty::Struct(sid)],
                Ty::I64,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            let mut acc = b.iconst(Ty::I64, 0);
            for (idx, (fty, key)) in fields.iter().zip(&plan).enumerate() {
                let fv = b.new_value(*fty);
                b.emit(Inst::FieldGet {
                    dest: fv,
                    base: 0,
                    sid,
                    idx: idx as u32,
                });
                let h = b.emit_elem_hash(fv, *fty, key);
                let next = b.new_value(Ty::I64);
                b.emit(Inst::Call {
                    dest: Some(next),
                    func: "zap_hash_combine".to_string(),
                    args: vec![acc, h],
                });
                acc = next;
            }
            b.set_term(Term::Ret(Some(acc)));
            self.ctx.generated.borrow_mut().push(b.func);
        }

        // __eq__(a, b) -> i1: AND the per-element comparisons, like gen_eq_method.
        {
            let mut b = FnLower::new(
                eqn.clone(),
                vec![Ty::Struct(sid), Ty::Struct(sid)],
                Ty::I1,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            let mut acc = b.iconst(Ty::I1, 1);
            for (idx, (fty, key)) in fields.iter().zip(&plan).enumerate() {
                let idx = idx as u32;
                let a = b.new_value(*fty);
                b.emit(Inst::FieldGet {
                    dest: a,
                    base: 0,
                    sid,
                    idx,
                });
                let c = b.new_value(*fty);
                b.emit(Inst::FieldGet {
                    dest: c,
                    base: 1,
                    sid,
                    idx,
                });
                let bit = b.emit_elem_eq(a, c, key);
                let next = b.new_value(Ty::I1);
                b.emit(Inst::Bin {
                    dest: next,
                    op: BinOp::And,
                    lhs: acc,
                    rhs: bit,
                });
                acc = next;
            }
            b.set_term(Term::Ret(Some(acc)));
            self.ctx.generated.borrow_mut().push(b.func);
        }

        let pair = (hashn, eqn);
        self.ctx.synth_keyfns.borrow_mut().insert(sid, pair.clone());
        Ok(pair)
    }

    /// Classify one element/payload type for the structural key path — shared by
    /// a tuple's per-field handling (#680, D-0049) and an `Optional[T]`'s payload
    /// (#704, D-0122). An unhashable element yields a clean coded diagnostic,
    /// never an ICE; an aggregate element (a nested tuple, an `Optional[T]`, or a
    /// `@derive(Hash, Eq)` struct) resolves to its own `(__hash__, __eq__)`,
    /// recursing to synthesize them on demand.
    pub(super) fn elem_key_plan(&mut self, fty: Ty) -> Result<ElemKey, LowerError> {
        Ok(match fty {
            Ty::Ptr | Ty::Bytes => ElemKey::Str,
            t if t.int_width().is_some() => ElemKey::Int,
            Ty::F32 | Ty::F64 => ElemKey::Float(fty),
            Ty::Struct(csid) => {
                if self.ctx.tuples.borrow().is_tuple(csid) {
                    let (ch, ce) = self.tuple_key_fns(csid)?;
                    ElemKey::Struct(ch, ce)
                } else if optional_layout(self.ctx, Ty::Struct(csid)).is_some() {
                    let (ch, ce) = self.optional_key_fns(csid)?;
                    ElemKey::Struct(ch, ce)
                } else if self
                    .ctx
                    .enums
                    .values()
                    .any(|e| e.is_sum && e.sid == Some(csid))
                {
                    // A tier-3 (data-carrying) enum key (D-0139/#809): a tagged
                    // value aggregate, hashable iff every payload field is —
                    // synthesized on demand like a tuple/Optional, routed through
                    // the function-pointer key path.
                    let (ch, ce) = self.enum_key_fns(csid)?;
                    ElemKey::Struct(ch, ce)
                } else {
                    let name = self.ctx.names[csid as usize].clone();
                    let ch = format!("{name}____hash__");
                    let ce = format!("{name}____eq__");
                    if !self.sigs.contains_key(&ch) || !self.sigs.contains_key(&ce) {
                        return Err(err(format!(
                            "a '{name}' element needs '__hash__' and '__eq__' to be part of \
                             a dict key or set element (use @derive(Hash, Eq) or define \
                             them; D-0047, D-0049)"
                        )));
                    }
                    ElemKey::Struct(ch, ce)
                }
            }
            other => {
                return Err(err(format!(
                    "a '{other}' element is not hashable and cannot be a dict key or set \
                     element (only integers, bools, char, enums, str, bytes, and \
                     tuples/Optionals/structs of those; D-0049, D-0122)"
                )))
            }
        })
    }

    /// Synthesize (once per `Optional[T]` instance id, #704 / D-0122) the
    /// structural `__hash__` and `__eq__` for an `Optional[T]` used as a dict key
    /// or set element, returning their symbol names. CPython parity: `None` and a
    /// present value are both hashable, so — like a tuple — the Optional routes
    /// through the function-pointer key path (D-0047). `__hash__(o) -> i64`
    /// returns a fixed sentinel for `None` and delegates to `T`'s hashing for
    /// `Some(x)`; `__eq__(a, b) -> i1` is true iff both are `None`, or both are
    /// `Some` with equal payloads — so `None` never compares equal to any present
    /// value (a hash collision between the sentinel and a payload hash is
    /// therefore harmless). An unhashable `T` (e.g. `Optional[float]`) is rejected
    /// with the same clean coded diagnostic as elsewhere, never an ICE. The
    /// generated functions append to [`StructCtx::generated`] and are referenced
    /// by `Inst::FuncAddr`, needing no signature registration (they are only ever
    /// called indirectly by the collection runtime). Reused across every key use
    /// of the same Optional instance, and the tagged/niche layout is handled by
    /// the shared `gen_opt_*` constructors (#564, D-0064).
    fn optional_key_fns(&mut self, sid: u32) -> Result<(String, String), LowerError> {
        if let Some(pair) = self.ctx.synth_keyfns.borrow().get(&sid) {
            return Ok(pair.clone());
        }
        let opt = optional_layout(self.ctx, Ty::Struct(sid))
            .expect("optional_key_fns called on a non-Optional struct id");
        // Classify the payload first, so an unhashable `T` fails cleanly before
        // any function is emitted. A `Some` payload that is itself a tuple or
        // Optional recurses to synthesize that layout's own key functions.
        let plan = self.elem_key_plan(opt.payload_ty)?;

        let hashn = format!("$opt${sid}$hash");
        let eqn = format!("$opt${sid}$eq");
        // A fixed, arbitrary nonzero hash for `None` (ASCII "none"). It need not
        // avoid payload hashes: `__eq__` keeps `None` distinct from every present
        // value, so a bucket collision only chains.
        const NONE_HASH: i64 = 0x6E6F_6E65;

        // __hash__(opt) -> i64: sentinel for `None`, delegate to `T` for `Some`.
        {
            let mut b = FnLower::new(
                hashn.clone(),
                vec![Ty::Struct(sid)],
                Ty::I64,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            let fv: Value = 0; // the Optional parameter
            let slot = b.alloca(Ty::I64);
            let is_none = gen_opt_is_none(&mut b, &opt, fv);
            let none_bb = b.new_block();
            let some_bb = b.new_block();
            let end_bb = b.new_block();
            b.set_term(Term::BrIf {
                cond: is_none,
                then_bb: none_bb,
                else_bb: some_bb,
            });
            b.position(none_bb);
            let sentinel = b.iconst(Ty::I64, NONE_HASH);
            b.store(slot, sentinel);
            b.set_term(Term::Br(end_bb));
            b.position(some_bb);
            let payload = gen_opt_payload(&mut b, &opt, fv);
            let h = b.emit_elem_hash(payload, opt.payload_ty, &plan);
            b.store(slot, h);
            b.set_term(Term::Br(end_bb));
            b.position(end_bb);
            let acc = b.load(slot, Ty::I64);
            b.set_term(Term::Ret(Some(acc)));
            self.ctx.generated.borrow_mut().push(b.func);
        }

        // __eq__(a, b) -> i1: both None -> true; one None -> false; both Some ->
        // compare payloads. The IR has no phi, so the result merges through a slot.
        {
            let mut b = FnLower::new(
                eqn.clone(),
                vec![Ty::Struct(sid), Ty::Struct(sid)],
                Ty::I1,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            let a: Value = 0;
            let c: Value = 1;
            let slot = b.alloca(Ty::I1);
            let a_none = gen_opt_is_none(&mut b, &opt, a);
            let b_none = gen_opt_is_none(&mut b, &opt, c);
            let a_none_bb = b.new_block();
            let a_some_bb = b.new_block();
            let end_bb = b.new_block();
            b.set_term(Term::BrIf {
                cond: a_none,
                then_bb: a_none_bb,
                else_bb: a_some_bb,
            });
            // a is None: equal iff b is also None.
            b.position(a_none_bb);
            b.store(slot, b_none);
            b.set_term(Term::Br(end_bb));
            // a is Some: equal iff b is Some and the payloads compare equal.
            b.position(a_some_bb);
            let b_none_bb = b.new_block();
            let both_some_bb = b.new_block();
            b.set_term(Term::BrIf {
                cond: b_none,
                then_bb: b_none_bb,
                else_bb: both_some_bb,
            });
            b.position(b_none_bb);
            let f = b.bconst(false);
            b.store(slot, f);
            b.set_term(Term::Br(end_bb));
            b.position(both_some_bb);
            let pa = gen_opt_payload(&mut b, &opt, a);
            let pc = gen_opt_payload(&mut b, &opt, c);
            let bit = b.emit_elem_eq(pa, pc, &plan);
            b.store(slot, bit);
            b.set_term(Term::Br(end_bb));
            b.position(end_bb);
            let r = b.load(slot, Ty::I1);
            b.set_term(Term::Ret(Some(r)));
            self.ctx.generated.borrow_mut().push(b.func);
        }

        let pair = (hashn, eqn);
        self.ctx.synth_keyfns.borrow_mut().insert(sid, pair.clone());
        Ok(pair)
    }

    /// Synthesize (once per tier-3 enum tagged-struct id, D-0139/#809) the
    /// structural `__hash__`/`__eq__` for a data-carrying enum used as a dict key
    /// or set element, returning their symbol names. A tier-3 enum is a tagged
    /// value aggregate (D-0046), so — like a tuple/`Optional` — it is hashable iff
    /// every payload field is, and routes through the function-pointer key path.
    /// `__hash__` folds `hash(tag)` with the active variant's payload-field hashes
    /// (switch on the `i32` tag at field 0, mirroring the tier-3 repr chain);
    /// `__eq__` is false on differing tags and otherwise ANDs the active variant's
    /// per-field comparisons. An unhashable payload (e.g. a `list` field) is a
    /// clean coded error via [`Self::elem_key_plan`], never an ICE. A niche
    /// `Optional`-shaped enum has no tag slot and is handled exactly like
    /// [`Self::optional_key_fns`] (null == the payload-free variant).
    fn enum_key_fns(&mut self, sid: u32) -> Result<(String, String), LowerError> {
        if let Some(pair) = self.ctx.synth_keyfns.borrow().get(&sid) {
            return Ok(pair.clone());
        }
        let info = self
            .ctx
            .enums
            .values()
            .find(|e| e.is_sum && e.sid == Some(sid))
            .cloned()
            .ok_or_else(|| err("enum_key_fns called on a non-tier-3-enum struct id"))?;
        let ename = info.enum_name.clone();
        let hashn = format!("$enum${sid}$hash");
        let eqn = format!("$enum${sid}$eq");

        // Niche `Optional`-shaped layout: no tag slot; `self` IS the payload
        // pointer, null for the payload-free variant. Identical treatment to an
        // `Optional[T]` key.
        if let Some(n) = &info.niche {
            let opt = OptionalLayout {
                sid,
                payload_ty: n.payload,
                niche: true,
                some_tag: 0,
                none_tag: 0,
                payload_idx: 0,
            };
            let plan = self.elem_key_plan(opt.payload_ty)?;
            const NONE_HASH: i64 = 0x6E6F_6E65;
            {
                let mut b = FnLower::new(
                    hashn.clone(),
                    vec![Ty::Struct(sid)],
                    Ty::I64,
                    false,
                    true,
                    self.sigs,
                    self.ctx,
                );
                let slot = b.alloca(Ty::I64);
                let is_none = gen_opt_is_none(&mut b, &opt, 0);
                let none_bb = b.new_block();
                let some_bb = b.new_block();
                let end_bb = b.new_block();
                b.set_term(Term::BrIf {
                    cond: is_none,
                    then_bb: none_bb,
                    else_bb: some_bb,
                });
                b.position(none_bb);
                let sentinel = b.iconst(Ty::I64, NONE_HASH);
                b.store(slot, sentinel);
                b.set_term(Term::Br(end_bb));
                b.position(some_bb);
                let payload = gen_opt_payload(&mut b, &opt, 0);
                let h = b.emit_elem_hash(payload, opt.payload_ty, &plan);
                b.store(slot, h);
                b.set_term(Term::Br(end_bb));
                b.position(end_bb);
                let acc = b.load(slot, Ty::I64);
                b.set_term(Term::Ret(Some(acc)));
                self.ctx.generated.borrow_mut().push(b.func);
            }
            {
                let mut b = FnLower::new(
                    eqn.clone(),
                    vec![Ty::Struct(sid), Ty::Struct(sid)],
                    Ty::I1,
                    false,
                    true,
                    self.sigs,
                    self.ctx,
                );
                let slot = b.alloca(Ty::I1);
                let a_none = gen_opt_is_none(&mut b, &opt, 0);
                let b_none = gen_opt_is_none(&mut b, &opt, 1);
                let a_none_bb = b.new_block();
                let a_some_bb = b.new_block();
                let end_bb = b.new_block();
                b.set_term(Term::BrIf {
                    cond: a_none,
                    then_bb: a_none_bb,
                    else_bb: a_some_bb,
                });
                b.position(a_none_bb);
                b.store(slot, b_none);
                b.set_term(Term::Br(end_bb));
                b.position(a_some_bb);
                let b_none_bb = b.new_block();
                let both_some_bb = b.new_block();
                b.set_term(Term::BrIf {
                    cond: b_none,
                    then_bb: b_none_bb,
                    else_bb: both_some_bb,
                });
                b.position(b_none_bb);
                let f = b.bconst(false);
                b.store(slot, f);
                b.set_term(Term::Br(end_bb));
                b.position(both_some_bb);
                let pa = gen_opt_payload(&mut b, &opt, 0);
                let pc = gen_opt_payload(&mut b, &opt, 1);
                let bit = b.emit_elem_eq(pa, pc, &plan);
                b.store(slot, bit);
                b.set_term(Term::Br(end_bb));
                b.position(end_bb);
                let r = b.load(slot, Ty::I1);
                b.set_term(Term::Ret(Some(r)));
                self.ctx.generated.borrow_mut().push(b.func);
            }
            let pair = (hashn, eqn);
            self.ctx.synth_keyfns.borrow_mut().insert(sid, pair.clone());
            return Ok(pair);
        }

        // General tagged layout: field 0 is the `i32` tag. Pre-classify every
        // payload field first (fail-fast + synthesize nested key fns) so an
        // unhashable payload is reported before any function is emitted.
        let variants = info.variants.clone();
        let mut plans: Vec<Vec<ElemKey>> = Vec::with_capacity(variants.len());
        for v in &variants {
            let mut ps = Vec::with_capacity(v.fields.len());
            for (_n, _idx, fty) in &v.fields {
                ps.push(self.elem_key_plan(*fty)?);
            }
            plans.push(ps);
        }

        // __hash__(self) -> i64: fold hash(tag) with the active variant's payload.
        {
            let mut b = FnLower::new(
                hashn.clone(),
                vec![Ty::Struct(sid)],
                Ty::I64,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            let result = b.alloca(Ty::I64);
            let merge = b.new_block();
            let mut check = b.cur;
            for (vi, v) in variants.iter().enumerate() {
                b.position(check);
                let tag = b.field_get(0, sid, 0, Ty::I32);
                let expected = b.iconst(Ty::I32, v.tag);
                let matches = b.new_value(Ty::I1);
                b.emit(Inst::ICmp {
                    dest: matches,
                    pred: CmpPred::Eq,
                    lhs: tag,
                    rhs: expected,
                });
                let found = b.new_block();
                let next = b.new_block();
                b.set_term(Term::BrIf {
                    cond: matches,
                    then_bb: found,
                    else_bb: next,
                });
                b.position(found);
                // Seed with the hash of the tag so two variants with identical
                // payloads still hash differently.
                let tagv = b.field_get(0, sid, 0, Ty::I32);
                let tag64 = b.coerce(tagv, Ty::I32, Ty::I64);
                let mut acc = b.new_value(Ty::I64);
                b.emit(Inst::Call {
                    dest: Some(acc),
                    func: "zap_i64_hash".to_string(),
                    args: vec![tag64],
                });
                for ((_n, idx, fty), key) in v.fields.iter().zip(&plans[vi]) {
                    let fv = b.field_get(0, sid, *idx, *fty);
                    let h = b.emit_elem_hash(fv, *fty, key);
                    let nxt = b.new_value(Ty::I64);
                    b.emit(Inst::Call {
                        dest: Some(nxt),
                        func: "zap_hash_combine".to_string(),
                        args: vec![acc, h],
                    });
                    acc = nxt;
                }
                b.store(result, acc);
                b.ensure_br(merge);
                check = next;
            }
            b.position(check);
            let message = b.strconst(&format!("invalid tag for enum {ename}"));
            b.throwing_call(None, "zap_panic", vec![message]);
            if !b.terminated() {
                b.set_term(Term::Unreachable);
            }
            b.position(merge);
            let r = b.load(result, Ty::I64);
            b.set_term(Term::Ret(Some(r)));
            self.ctx.generated.borrow_mut().push(b.func);
        }

        // __eq__(a, b) -> i1: differing tags -> false; else AND the active
        // variant's per-field comparisons.
        {
            let mut b = FnLower::new(
                eqn.clone(),
                vec![Ty::Struct(sid), Ty::Struct(sid)],
                Ty::I1,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            let result = b.alloca(Ty::I1);
            let merge = b.new_block();
            let ta = b.field_get(0, sid, 0, Ty::I32);
            let tb = b.field_get(1, sid, 0, Ty::I32);
            let tags_eq = b.new_value(Ty::I1);
            b.emit(Inst::ICmp {
                dest: tags_eq,
                pred: CmpPred::Eq,
                lhs: ta,
                rhs: tb,
            });
            let same_bb = b.new_block();
            let diff_bb = b.new_block();
            b.set_term(Term::BrIf {
                cond: tags_eq,
                then_bb: same_bb,
                else_bb: diff_bb,
            });
            b.position(diff_bb);
            let f = b.bconst(false);
            b.store(result, f);
            b.ensure_br(merge);
            b.position(same_bb);
            let mut check = b.cur;
            for (vi, v) in variants.iter().enumerate() {
                b.position(check);
                let tag = b.field_get(0, sid, 0, Ty::I32);
                let expected = b.iconst(Ty::I32, v.tag);
                let matches = b.new_value(Ty::I1);
                b.emit(Inst::ICmp {
                    dest: matches,
                    pred: CmpPred::Eq,
                    lhs: tag,
                    rhs: expected,
                });
                let found = b.new_block();
                let next = b.new_block();
                b.set_term(Term::BrIf {
                    cond: matches,
                    then_bb: found,
                    else_bb: next,
                });
                b.position(found);
                let mut acc = b.bconst(true);
                for ((_n, idx, fty), key) in v.fields.iter().zip(&plans[vi]) {
                    let fa = b.field_get(0, sid, *idx, *fty);
                    let fb = b.field_get(1, sid, *idx, *fty);
                    let bit = b.emit_elem_eq(fa, fb, key);
                    let nxt = b.new_value(Ty::I1);
                    b.emit(Inst::Bin {
                        dest: nxt,
                        op: BinOp::And,
                        lhs: acc,
                        rhs: bit,
                    });
                    acc = nxt;
                }
                b.store(result, acc);
                b.ensure_br(merge);
                check = next;
            }
            // Tags equal but matched no variant: unreachable for a valid value.
            b.position(check);
            let f2 = b.bconst(false);
            b.store(result, f2);
            b.ensure_br(merge);
            b.position(merge);
            let r = b.load(result, Ty::I1);
            b.set_term(Term::Ret(Some(r)));
            self.ctx.generated.borrow_mut().push(b.func);
        }

        let pair = (hashn, eqn);
        self.ctx.synth_keyfns.borrow_mut().insert(sid, pair.clone());
        Ok(pair)
    }
}
