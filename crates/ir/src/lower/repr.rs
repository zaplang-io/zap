//! Value → repr / display rendering for aggregates and interfaces.
//!
//! Struct-to-string rendering (#274 repr protocol, D-0124), interface-value
//! display (#707), and the synthesized `__repr__` functions for structs,
//! tier-1 boxed enums, and containers — resolving an explicit `__str__`/
//! `__repr__`, else `@derive(Repr)`, else the default field-wise form.
//! Extracted verbatim from `lower.rs` (#1134); `use super::*` re-exposes the
//! parent scope.

use super::*;

impl<'a> FnLower<'a> {
    /// Render a struct value to a string, for `print` / `str` / f-string / REPL
    /// contexts (D-0047; the general rendering protocol #274, D-0124). Resolves
    /// the type's renderer with [`Self::struct_repr_fn`] — an explicit `__str__`
    /// (preferred) or `__repr__`, else `@derive(Repr)`, else the compiler's
    /// default field-wise form `TypeName(field=value, …)` — and invokes it. The
    /// call is `throwing` because a user-defined `__str__`/`__repr__` (or a
    /// `bytes` field in the default render) may raise.
    ///
    /// `prefer_repr` selects the dunder preference for a type defining BOTH
    /// `__str__` and `__repr__` (#1384): `false` (str contexts: top-level
    /// `str()`/`print()`/f-string) prefers `__str__`; `true` (repr contexts:
    /// `repr()` and every container element, Python-exact) prefers `__repr__`.
    /// For a type defining only one dunder, or none, the choice is immaterial.
    pub(super) fn struct_to_str(
        &mut self,
        sid: u32,
        v: Value,
        prefer_repr: bool,
    ) -> Result<Value, LowerError> {
        let f = self.struct_repr_fn(sid, prefer_repr)?;
        let d = self.new_value(Ty::Ptr);
        self.throwing_call(Some(d), &f, vec![v]);
        Ok(d)
    }

    /// Render an **interface value** `v` of interface `iid` (#707, D-0124). The
    /// concrete type behind the fat pointer is erased at this site, so rendering
    /// cannot resolve a static renderer symbol the way [`Self::struct_to_str`]
    /// does; it emits an [`Inst::IfaceRender`] that dynamically dispatches through
    /// the itable's renderer slot to the concrete type's own renderer (recorded in
    /// [`StructCtx::iface_renderers`] at each concrete->interface coercion). The
    /// renderer may raise (a user `__str__`, or a `bytes` field decode), so it
    /// routes through the active landing pad exactly like [`Self::throwing_call`]:
    /// the [`Inst::IfaceRenderInvoke`] form inside a `try`, the plain
    /// [`Inst::IfaceRender`] otherwise. Always yields a `str`.
    pub(super) fn render_iface_value(
        &mut self,
        iface: Value,
        iid: u32,
    ) -> Result<Value, LowerError> {
        let dest = self.new_value(Ty::Ptr);
        if let Some(&lp) = self.landing_pads.last() {
            self.used_lps.insert(lp);
            self.emit(Inst::IfaceRenderInvoke {
                dest,
                iface,
                iid,
                unwind: lp,
            });
        } else {
            self.emit(Inst::IfaceRender { dest, iface, iid });
        }
        Ok(dest)
    }

    /// Resolve the symbol name of the function that renders a value of struct id
    /// `sid` to a string, synthesizing the compiler default on demand (#274,
    /// D-0124). Resolution order (first match wins), so an explicit override
    /// always beats the default:
    ///
    /// 1. a tuple layout → the Python `(v0, v1, …)` renderer (synthesized once);
    /// 2. a hand-written `__str__`;
    /// 3. a hand-written or `@derive(Repr)` `__repr__`;
    /// 4. a tier-3 data-carrying enum (D-0046) → the tag-dispatch structural
    ///    renderer `Enum.Variant(field=value, …)` (#706, synthesized once);
    /// 5. otherwise the compiler-synthesized default `TypeName(field=value, …)`
    ///    renderer (synthesized once, memoized in [`StructCtx::synth_reprfns`]).
    ///
    /// Only an interface value's fat struct still has no default and is rejected
    /// with a clean coded error pointing at the #707 follow-up — never an ICE or a
    /// nonsense render over synthetic `$data`/`$itable` slots.
    pub(super) fn struct_repr_fn(
        &mut self,
        sid: u32,
        prefer_repr: bool,
    ) -> Result<String, LowerError> {
        // Tuples have no `ctx.names` entry (their ids sit past `names.len()`), so
        // they MUST be handled before any `names[sid]` lookup — the same ordering
        // the tuple key-fn path relies on (#680).
        if self.ctx.tuples.borrow().is_tuple(sid) {
            if let Some(name) = self.ctx.synth_reprfns.borrow().get(&sid) {
                return Ok(name.clone());
            }
            let name = format!("$tuple${sid}$repr");
            // Memoize BEFORE emitting the body so a (hypothetically) self-referring
            // layout terminates; tuples cannot actually nest themselves, but this
            // keeps the invariant uniform with the struct path below.
            self.ctx
                .synth_reprfns
                .borrow_mut()
                .insert(sid, name.clone());
            let mut b = FnLower::new(
                name.clone(),
                vec![Ty::Struct(sid)],
                Ty::Ptr,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            if let Err(e) = gen_tuple_repr_body(&mut b, sid) {
                // Body generation failed (e.g. an unrenderable element type):
                // drop the memo so the half-built name never resolves to a missing
                // function if errors are collected rather than fatal.
                self.ctx.synth_reprfns.borrow_mut().remove(&sid);
                return Err(e);
            }
            self.ctx.generated.borrow_mut().push(b.func);
            return Ok(name);
        }
        // An interface value's synthetic `{data, itable}` fat struct has no static
        // renderer: its concrete type is erased, so it renders by **dynamic
        // dispatch** through the itable's renderer slot (#707, D-0124), which
        // [`FnLower::render_value`] emits as an [`Inst::IfaceRender`] *before*
        // reaching `struct_to_str`/here. Reaching this branch therefore means a fat
        // struct was handed to the static-renderer path directly — not a render
        // site — so it stays a clean coded error (never an ICE or a nonsense render
        // over the synthetic `$data`/`$itable` slots) rather than a reachable
        // user-facing limitation.
        if self.ctx.fat_to_iid.contains_key(&sid) {
            return Err(err(format!(
                "internal: interface value ('{}') reached the static renderer path; \
                 an interface value renders via dynamic dispatch (Inst::IfaceRender), \
                 not a static renderer symbol (#707, D-0124)",
                self.ctx.names[sid as usize]
            )));
        }
        // An explicit `__str__`/`__repr__` / `@derive(Repr)` wins over the
        // default — for a tier-3 enum too (its methods mangle to the same
        // `{Name}__{dunder}`), so this is tested BEFORE the enum's default
        // structural renderer below. `prefer_repr` flips the preference for a
        // type defining BOTH (#1384): repr contexts try `__repr__` first, str
        // contexts `__str__` first; each still falls back to the other.
        let order = if prefer_repr {
            ["__repr__", "__str__"]
        } else {
            ["__str__", "__repr__"]
        };
        for dunder in order {
            let mangled = format!("{}__{}", self.ctx.names[sid as usize], dunder);
            if self.sigs.contains_key(&mangled) {
                return Ok(mangled);
            }
        }
        // A tier-3 data-carrying enum (D-0046) lowers as a tagged struct
        // `{ $tag:i32, payload… }` (or a niche `Optional`-shaped pointer).
        // Render it structurally — `Enum.Variant(field=value, …)` for a
        // data-carrying variant, `Enum.Variant` for a payload-free one — via a
        // per-enum tag-dispatch renderer synthesized on demand (#706, D-0124
        // extended to D-0046), mirroring the default-struct synthesis below but
        // keyed on the tag like the tier-1 `.name` chain.
        if self
            .ctx
            .enums
            .values()
            .any(|e| e.is_sum && e.sid == Some(sid))
        {
            if let Some(name) = self.ctx.synth_reprfns.borrow().get(&sid) {
                return Ok(name.clone());
            }
            let name = format!("{}$repr$enum", self.ctx.names[sid as usize]);
            // Memoize BEFORE emitting the body so a recursive variant field whose
            // type is this same enum (`enum Expr: Add(Expr l, Expr r)`) resolves to
            // this name as a plain call — compile-time synthesis terminates (one
            // function per enum).
            self.ctx
                .synth_reprfns
                .borrow_mut()
                .insert(sid, name.clone());
            let mut b = FnLower::new(
                name.clone(),
                vec![Ty::Struct(sid)],
                Ty::Ptr,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            if let Err(e) = gen_enum_repr_body(&mut b, sid) {
                // Drop the memo on failure (e.g. an unrenderable payload type), so
                // a collected-error build never calls a half-built, never-emitted
                // function.
                self.ctx.synth_reprfns.borrow_mut().remove(&sid);
                return Err(e);
            }
            self.ctx.generated.borrow_mut().push(b.func);
            return Ok(name);
        }
        // The compiler-synthesized default field-wise renderer.
        if let Some(name) = self.ctx.synth_reprfns.borrow().get(&sid) {
            return Ok(name.clone());
        }
        let name = format!("{}$repr$default", self.ctx.names[sid as usize]);
        // Memoize BEFORE emitting the body: a field whose type is this same struct
        // (directly or through a mutual cycle) resolves to this name as a plain
        // call, so compile-time synthesis terminates (one function per struct).
        self.ctx
            .synth_reprfns
            .borrow_mut()
            .insert(sid, name.clone());
        let mut b = FnLower::new(
            name.clone(),
            vec![Ty::Struct(sid)],
            Ty::Ptr,
            false,
            true,
            self.sigs,
            self.ctx,
        );
        if let Err(e) = gen_struct_repr_body(&mut b, sid) {
            // Drop the memo on failure (e.g. an unrenderable field type), so a
            // collected-error build never calls a half-built, never-emitted fn.
            self.ctx.synth_reprfns.borrow_mut().remove(&sid);
            return Err(e);
        }
        self.ctx.generated.borrow_mut().push(b.func);
        Ok(name)
    }

    /// Synthesize the Display renderer for a tier-1 enum's interface box
    /// `{Enum}$box` (#731, D-0124/D-0016). The box erases the enum to a bare
    /// `{ $tag:i64 }` heap cell, so the generic struct renderer would print
    /// `Color$box($tag=1)` — wrong. This renderer instead reloads the tag and
    /// renders it as the enum member (`Color.Green`), reusing the exact tier-1
    /// enum render path (`render_value` on `Ty::Enum`) that a bare enum value
    /// uses. Placed in the itable's renderer slot so `print`/`str`/f-string on an
    /// interface value backed by a tier-1 enum renders correctly (ties off #707).
    /// Memoized per box id in `synth_reprfns`, like the other synthesized reprs.
    pub(super) fn tier1_box_repr_fn(
        &mut self,
        box_sid: u32,
        enum_id: u32,
    ) -> Result<String, LowerError> {
        if let Some(name) = self.ctx.synth_reprfns.borrow().get(&box_sid) {
            return Ok(name.clone());
        }
        let name = format!("{}$repr", self.ctx.names[box_sid as usize]);
        self.ctx
            .synth_reprfns
            .borrow_mut()
            .insert(box_sid, name.clone());
        let mut b = FnLower::new(
            name.clone(),
            vec![Ty::Struct(box_sid)],
            Ty::Ptr,
            false,
            true,
            self.sigs,
            self.ctx,
        );
        let tag = b.new_value(Ty::Enum(enum_id));
        b.emit(Inst::FieldGet {
            dest: tag,
            base: 0,
            sid: box_sid,
            idx: 0,
        });
        match b.render_value(tag, Ty::Enum(enum_id), RenderMode::Repr) {
            Ok(s) => b.set_term(Term::Ret(Some(s))),
            Err(e) => {
                self.ctx.synth_reprfns.borrow_mut().remove(&box_sid);
                return Err(e);
            }
        }
        self.ctx.generated.borrow_mut().push(b.func);
        Ok(name)
    }

    /// Resolve the symbol name of the function that renders a `list`/`dict`/`set`
    /// value of ZIR type `vt` to its Python string form (#274, D-0124),
    /// synthesizing it on demand (memoized per container type):
    /// `[v0, v1]` / `{k0: v0, k1: v1}` / `{v0, v1}`, with the Python-exact empty
    /// forms `[]`, `{}`, and `set()`. Every element/key/value renders in `repr`
    /// mode (so string elements are quoted), through the shared value renderer —
    /// so containers nest (`[[1], [2]]`, `{'k': [1, 2]}`) and carry structs,
    /// tuples, enums, and `Optional` uniformly. The renderer reuses the tested
    /// list-iteration (`iterate_list`) and dict key/value entry points, so it is
    /// not a bespoke runtime path.
    pub(super) fn container_repr_fn(&mut self, vt: Ty) -> Result<String, LowerError> {
        let key = format!("{vt:?}");
        if let Some(name) = self.ctx.synth_container_reprfns.borrow().get(&key) {
            return Ok(name.clone());
        }
        let name = format!("$crepr${}", self.ctx.synth_container_reprfns.borrow().len());
        let mut b = FnLower::new(
            name.clone(),
            vec![vt],
            Ty::Ptr,
            false,
            true,
            self.sigs,
            self.ctx,
        );
        // The container is the single `self` parameter: value 0.
        let c: Value = 0;
        match &vt {
            Ty::List(elem) => {
                let elem = *elem;
                let acc = b.alloca(Ty::Ptr);
                let first = b.alloca(Ty::I64);
                let open = b.strconst("[");
                b.store(acc, open);
                let one = b.iconst(Ty::I64, 1);
                b.store(first, one);
                b.iterate_list(c, elem, |s, e, _ety| {
                    gen_seq_sep(s, acc, first);
                    let es = render_elem_repr(s, e, elem)?;
                    let a = s.load(acc, Ty::Ptr);
                    let a2 = gen_concat(s, a, es);
                    s.store(acc, a2);
                    Ok(())
                })?;
                let a = b.load(acc, Ty::Ptr);
                let close = b.strconst("]");
                let a2 = gen_concat(&mut b, a, close);
                b.set_term(Term::Ret(Some(a2)));
            }
            Ty::Set(elem) => {
                let elem = *elem;
                // `zap_set_items` snapshots the set's elements as a `list[elem]`
                // in stable iteration order (#608/#609), reused by the list loop.
                let items = b.new_value(Ty::List(elem));
                b.emit(Inst::Call {
                    dest: Some(items),
                    func: "zap_set_items".to_string(),
                    args: vec![c],
                });
                let len = b.new_value(Ty::I64);
                b.emit(Inst::Call {
                    dest: Some(len),
                    func: "zap_list_len".to_string(),
                    args: vec![items],
                });
                let zero = b.iconst(Ty::I64, 0);
                let is_empty = b.new_value(Ty::I1);
                b.emit(Inst::ICmp {
                    dest: is_empty,
                    pred: CmpPred::Eq,
                    lhs: len,
                    rhs: zero,
                });
                let result = b.alloca(Ty::Ptr);
                let empty_bb = b.new_block();
                let nonempty_bb = b.new_block();
                let done_bb = b.new_block();
                b.set_term(Term::BrIf {
                    cond: is_empty,
                    then_bb: empty_bb,
                    else_bb: nonempty_bb,
                });
                // Python renders the empty set as `set()`, NOT `{}` (which is an
                // empty dict).
                b.position(empty_bb);
                let empty_str = b.strconst("set()");
                b.store(result, empty_str);
                b.set_term(Term::Br(done_bb));
                b.position(nonempty_bb);
                let acc = b.alloca(Ty::Ptr);
                let first = b.alloca(Ty::I64);
                let open = b.strconst("{");
                b.store(acc, open);
                let one = b.iconst(Ty::I64, 1);
                b.store(first, one);
                b.iterate_list(items, elem, |s, e, _ety| {
                    gen_seq_sep(s, acc, first);
                    let es = render_elem_repr(s, e, elem)?;
                    let a = s.load(acc, Ty::Ptr);
                    let a2 = gen_concat(s, a, es);
                    s.store(acc, a2);
                    Ok(())
                })?;
                let a = b.load(acc, Ty::Ptr);
                let close = b.strconst("}");
                let a2 = gen_concat(&mut b, a, close);
                b.store(result, a2);
                b.set_term(Term::Br(done_bb));
                b.position(done_bb);
                let out = b.load(result, Ty::Ptr);
                b.set_term(Term::Ret(Some(out)));
            }
            Ty::Dict(kk, vk) => {
                let (kk, vk) = (*kk, *vk);
                let acc = b.alloca(Ty::Ptr);
                let first = b.alloca(Ty::I64);
                let open = b.strconst("{");
                b.store(acc, open);
                let one = b.iconst(Ty::I64, 1);
                b.store(first, one);
                // Snapshot keys (stable order), then fetch each value with the
                // typed dict getter — the same entry points the dict literal /
                // merge paths use; a struct key supplies its `__hash__`/`__eq__`
                // via `key_fn_args`.
                let keys = b.new_value(Ty::List(kk));
                b.emit(Inst::Call {
                    dest: Some(keys),
                    func: format!("zap_dict_keys_{}", kk.val_suffix()),
                    args: vec![c],
                });
                let kfns = b.key_fn_args(kk)?;
                let get_fn = format!("zap_dict_get_{}_{}", kk.key_suffix(), vk.val_suffix());
                let vty = b.elem_ty(vk);
                b.iterate_list(keys, kk, |s, key, _kty| {
                    gen_seq_sep(s, acc, first);
                    // key repr
                    let ks = render_elem_repr(s, key, kk)?;
                    let a = s.load(acc, Ty::Ptr);
                    let a2 = gen_concat(s, a, ks);
                    let colon = s.strconst(": ");
                    let a3 = gen_concat(s, a2, colon);
                    s.store(acc, a3);
                    // value: look it up by key, then repr it
                    let val = s.new_value(vty);
                    let mut gargs = vec![c, key];
                    gargs.extend_from_slice(&kfns);
                    s.emit(Inst::Call {
                        dest: Some(val),
                        func: get_fn.clone(),
                        args: gargs,
                    });
                    let vs = render_elem_repr(s, val, vk)?;
                    let a4 = s.load(acc, Ty::Ptr);
                    let a5 = gen_concat(s, a4, vs);
                    s.store(acc, a5);
                    Ok(())
                })?;
                let a = b.load(acc, Ty::Ptr);
                let close = b.strconst("}");
                let a2 = gen_concat(&mut b, a, close);
                b.set_term(Term::Ret(Some(a2)));
            }
            _ => return Err(err("container_repr_fn called on a non-container type")),
        }
        self.ctx.generated.borrow_mut().push(b.func);
        self.ctx
            .synth_container_reprfns
            .borrow_mut()
            .insert(key, name.clone());
        Ok(name)
    }
}
