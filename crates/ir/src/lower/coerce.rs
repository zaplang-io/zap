//! Coercions / numeric helpers.
//!
//! Value coercion between types (`coerce`), the `as_f64` float widening helper,
//! numeric `convert`, and `truthy` (value → `i1`). Extracted verbatim from
//! `lower.rs` (#1127); `use super::*` re-exposes the parent scope.

use super::*;

impl<'a> FnLower<'a> {
    pub(super) fn coerce(&mut self, v: Value, from: Ty, to: Ty) -> Value {
        if from == to {
            return v;
        }
        if to == Ty::Bigint {
            let d = self.new_value(Ty::Bigint);
            if from.int_width().is_some() {
                let arg = self.coerce(v, from, Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_bigint_from_i64".to_string(),
                    args: vec![arg],
                });
            } else if from.is_float() {
                let arg = self.as_f64(v, from);
                self.throwing_call(Some(d), "zap_bigint_from_f64", vec![arg]);
            } else {
                return v;
            }
            return d;
        }
        if from == Ty::Bigint && to == Ty::Complex {
            let real = self.coerce(v, from, Ty::F64);
            let d = self.new_value(Ty::Complex);
            self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_complex_from_f64".to_string(),
                args: vec![real],
            });
            return d;
        }
        if from == Ty::Bigint && to.is_float() {
            let f64v = self.new_value(Ty::F64);
            self.emit(Inst::Call {
                dest: Some(f64v),
                func: "zap_bigint_to_f64".to_string(),
                args: vec![v],
            });
            return if to == Ty::F64 {
                f64v
            } else {
                self.coerce(f64v, Ty::F64, to)
            };
        }
        if to == Ty::Complex && (from.int_width().is_some() || from.is_float()) {
            let real = self.as_f64(v, from);
            let d = self.new_value(Ty::Complex);
            self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_complex_from_f64".to_string(),
                args: vec![real],
            });
            return d;
        }
        // Concrete struct -> interface value: build the `{data, itable}` fat
        // pointer (D-0016, design/0006 §2.4). A reference struct is used directly
        // as `data` (shared heap identity). A `value struct` is **boxed** first
        // (I#75 Part 1, D-0051): copied to a fresh heap instance so `data` holds a
        // stable, GC-traced pointer to a copy that is independent of the source
        // (whose storage may be a stack slot under the register/sret ABI, or an
        // lvalue that must not alias). interface -> interface and `None` ->
        // interface are documented follow-ups.
        if let Ty::Struct(to_sid) = to {
            if let Some(iid) = self.ctx.iid_of_fat_sid(to_sid) {
                if let Ty::Struct(from_sid) = from {
                    if self.ctx.iid_of_fat_sid(from_sid).is_none() {
                        let data = if self.ctx.is_value_struct(from_sid) {
                            self.copy_value_struct(v, from_sid)
                        } else {
                            v
                        };
                        let dest = self.new_value(to);
                        self.emit(Inst::IfaceMake {
                            dest,
                            data,
                            iid,
                            concrete_sid: from_sid,
                        });
                        // Synthesize and record the concrete type's Display
                        // renderer (#707, D-0124) so codegen can place it in the
                        // renderer slot of this `(concrete, iid)` itable — letting
                        // `print`/`str`/f-string on the resulting interface value
                        // dynamically dispatch to the concrete type's renderer
                        // (`Circle(r=2.0)`, a tier-3 enum's structural form, …).
                        // Synthesis is memoized, so repeated coercions of the same
                        // concrete type are free. A renderer that cannot be built
                        // (an unrenderable field type — not reachable for the field
                        // types D-0124 supports) simply records nothing, leaving a
                        // null renderer slot rather than failing construction, so an
                        // interface used only for method dispatch is never regressed.
                        if !self.ctx.iface_renderers.borrow().contains_key(&from_sid) {
                            if let Ok(sym) = self.struct_repr_fn(from_sid) {
                                self.ctx.iface_renderers.borrow_mut().insert(from_sid, sym);
                            }
                        }
                        return dest;
                    }
                } else if let Ty::Enum(eid) = from {
                    // Tier-1 (unboxed-integer) enum -> interface value (#731,
                    // D-0016). A tier-1 enum has no pointer identity, so box its
                    // scalar tag into a fresh heap cell `{Enum}$box = { $tag:i64 }`
                    // and use that pointer as the fat pointer's `data`. Each
                    // widening boxes afresh — so the box's `is`-identity is unique
                    // per conversion, NOT shared across widenings of the same tag
                    // (documented under the D-0016 extension); use `==` for value
                    // equality. The box is an immutable, pointer-free leaf: GC
                    // traces it trivially and rooting is transitive through the fat
                    // pointer (as for value-struct boxing, I#75). Dispatch goes
                    // through the itable's per-method thunks `{Enum}$box__m`.
                    let ename = self
                        .ctx
                        .enums
                        .values()
                        .find(|e| !e.is_sum && e.enum_id == eid)
                        .map(|e| e.enum_name.clone());
                    if let Some(box_sid) =
                        ename.and_then(|n| self.ctx.ids.get(&format!("{n}$box")).copied())
                    {
                        let boxv = self.new_value(Ty::Struct(box_sid));
                        self.emit(Inst::StructNew {
                            dest: boxv,
                            sid: box_sid,
                        });
                        self.emit(Inst::FieldSet {
                            base: boxv,
                            sid: box_sid,
                            idx: 0,
                            val: v,
                        });
                        let dest = self.new_value(to);
                        self.emit(Inst::IfaceMake {
                            dest,
                            data: boxv,
                            iid,
                            concrete_sid: box_sid,
                        });
                        // #707 renderer slot: a per-box renderer that reloads the
                        // tag and renders the enum member (`Color.Green`), so
                        // `print`/`str`/f-string on the interface value works.
                        if !self.ctx.iface_renderers.borrow().contains_key(&box_sid) {
                            if let Ok(sym) = self.tier1_box_repr_fn(box_sid, eid) {
                                self.ctx.iface_renderers.borrow_mut().insert(box_sid, sym);
                            }
                        }
                        return dest;
                    }
                }
                // Not a concrete-struct source we can box — leave unchanged; sema
                // has already rejected a non-conforming or unsupported source.
                return v;
            }
        }
        if let (Some(fw), Some(tw)) = (from.int_width(), to.int_width()) {
            if fw == tw {
                return v;
            }
            let d = self.new_value(to);
            // Widen by zero-extension when the *source* is unsigned (#552), by
            // sign-extension otherwise; narrowing is a plain truncation for
            // both. (Implicit widening only ever pairs same-sign types — sema's
            // `assignable` forbids a sign crossing — so the source's sign is the
            // right one to key on.)
            let kind = if tw > fw {
                if from.is_unsigned() {
                    CastKind::ZExt
                } else {
                    CastKind::SExt
                }
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
            // `uitofp` for an unsigned source so a `uint64` above `INT64_MAX`
            // converts to the right magnitude (reached by `/` on unsigned
            // operands, which promotes to `f64`); `sitofp` otherwise (#552).
            let kind = if from.is_unsigned() {
                CastKind::UIToFP
            } else {
                CastKind::SIToFP
            };
            self.emit(Inst::Cast {
                dest: d,
                kind,
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

    /// Widen a numeric value to `f64` (int -> sitofp, f32 -> fpext, f64 -> self).
    pub(super) fn as_f64(&mut self, v: Value, t: Ty) -> Value {
        self.coerce(v, t, Ty::F64)
    }

    /// An explicit numeric conversion to `to` (the `int32(x)` / `float64(x)`
    /// form, D-0020). Unlike [`coerce`], which only widens, this also narrows
    /// and converts float -> int (a user wrote the cast, so data loss is their
    /// intent). Non-numeric operands pass through unchanged.
    pub(super) fn convert(&mut self, v: Value, from: Ty, to: Ty) -> Value {
        if from == to {
            return v;
        }
        if from == Ty::Bigint && to.int_width().is_some() {
            if to == Ty::I32 {
                let i32v = self.new_value(Ty::I32);
                self.throwing_call(Some(i32v), "zap_bigint_to_i32", vec![v]);
                return i32v;
            }
            let i64v = self.new_value(Ty::I64);
            self.throwing_call(Some(i64v), "zap_bigint_to_i64", vec![v]);
            return self.coerce(i64v, Ty::I64, to);
        }
        if from == Ty::Bigint && to.is_float() {
            return self.coerce(v, from, to);
        }
        let kind = match (from, to) {
            (f, t) if f.int_width().is_some() && t.int_width().is_some() => {
                if f.int_width() == t.int_width() {
                    // Same physical width, possibly a sign reinterpret
                    // (`int64(u)` / `uint64(i)`): the bit pattern is unchanged,
                    // so no cast instruction is needed — the dest type already
                    // carries the new signedness (#552). This is how an
                    // out-of-range round-trip wraps (`int64(uint64 > INT64_MAX)`
                    // is negative), matching a C reinterpret cast.
                    return v;
                }
                // Widen by zero-extension from an unsigned source (`uint64(small_u)`),
                // by sign-extension from a signed source; narrowing truncates
                // for both (C cast / modular-wrap semantics, #552).
                if t.int_width().unwrap() > f.int_width().unwrap() {
                    if f.is_unsigned() {
                        CastKind::ZExt
                    } else {
                        CastKind::SExt
                    }
                } else {
                    CastKind::Trunc
                }
            }
            (f, t) if f.int_width().is_some() && t.is_float() => {
                if f.is_unsigned() {
                    CastKind::UIToFP
                } else {
                    CastKind::SIToFP
                }
            }
            // Float -> int: `fptoui` into an unsigned target, `fptosi` into a
            // signed one (#552). Out-of-range float -> int is C-undefined; Zap
            // documents it as implementation-defined saturation/wrap.
            (f, t) if f.is_float() && t.is_unsigned() => CastKind::FPToUI,
            (f, t) if f.is_float() && t.int_width().is_some() => CastKind::FPToSI,
            (f, t) if f.is_float() && t.is_float() => {
                if t == Ty::F64 {
                    CastKind::FPExt
                } else {
                    CastKind::FPTrunc
                }
            }
            _ => return v,
        };
        let d = self.new_value(to);
        self.emit(Inst::Cast {
            dest: d,
            kind,
            val: v,
        });
        d
    }

    pub(super) fn truthy(&mut self, v: Value, t: Ty) -> Value {
        if t == Ty::I1 {
            return v;
        }
        // The truth value of a (possibly multi-element) array is ambiguous
        // (#510, D-0175) — exactly NumPy's rule. This covers `if a:`, `a and b`,
        // `not a`, and chained comparison `lo < a < hi` (which desugars through
        // `and`). Raise a catchable `ValueError`; the result value is unused.
        if matches!(t, Ty::Array(..)) {
            self.throwing_call(None, "zap_array_truth_ambiguous", vec![]);
            return self.iconst(Ty::I1, 0);
        }
        if t == Ty::Complex || t == Ty::Bigint {
            let raw = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(raw),
                func: if t == Ty::Complex {
                    "zap_complex_truthy".to_string()
                } else {
                    "zap_bigint_truthy".to_string()
                },
                args: vec![v],
            });
            let zero = self.iconst(Ty::I64, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: raw,
                rhs: zero,
            });
            return d;
        }
        // Truthiness overloading on user structs (D-0047): `if a:` / `not a` /
        // `a and b` call `a.__bool__()` when defined. A struct with no `__bool__`
        // is a non-null object, so it is truthy.
        if let Ty::Struct(sid) = t {
            let mangled = format!("{}__{}", self.ctx.names[sid as usize], "__bool__");
            if self.sigs.contains_key(&mangled) {
                let d = self.new_value(Ty::I1);
                self.throwing_call(Some(d), &mangled, vec![v]);
                return d;
            }
            return self.iconst(Ty::I1, 1);
        }
        if matches!(t, Ty::Match | Ty::Py) {
            // Reference types are truthy when non-null. Codegen's `ICmp` only
            // compares integers, so go through the runtime (`zap_ptr_nonnull`
            // returns 0/1) and compare that i64 against zero. For a match
            // object, null means "no match" (D-0010). A Python handle is a
            // non-null wrapper, so it is effectively always truthy here; real
            // Python truthiness (`PyObject_IsTrue`) is a documented follow-up.
            let nn = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(nn),
                func: "zap_ptr_nonnull".to_string(),
                args: vec![v],
            });
            let z = self.iconst(Ty::I64, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: nn,
                rhs: z,
            });
            return d;
        }
        // Strings/bytes and built-in containers (list/dict/set/heap/array) follow
        // Python / D-0022 truthiness: empty is falsy, non-empty truthy. They are
        // heap pointers, so the integer `ICmp` fallthrough below would feed a
        // `PointerValue` where codegen expects an `IntValue` and crash (#382), and
        // the old `zap_ptr_nonnull` path made every non-null `str` truthy so `""`
        // was wrongly truthy (#385). Route through the length runtime and compare
        // the element/byte count to zero instead. `str` and `bytes` share the
        // `ZapStr {len, data}` layout, so both use `zap_str_len`.
        let len_fn = match t {
            Ty::Ptr | Ty::Bytes => Some("zap_str_len"),
            Ty::List(_) => Some("zap_list_len"),
            Ty::Dict(..) => Some("zap_dict_len"),
            Ty::Set(_) => Some("zap_set_len"),
            Ty::Heap(_) => Some("zap_heap_len"),
            Ty::Deque(_) => Some("zap_deque_len"),
            Ty::OSet(_) => Some("zap_oset_len"),
            Ty::Array(..) => Some("zap_array_len"),
            _ => None,
        };
        if let Some(len_fn) = len_fn {
            let len = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(len),
                func: len_fn.to_string(),
                args: vec![v],
            });
            let z = self.iconst(Ty::I64, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: len,
                rhs: z,
            });
            return d;
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
