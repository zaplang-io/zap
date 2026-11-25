//! Value → string conversions and f-strings (D-0025/D-0026).
//!
//! `str(x)`/`repr(x)` rendering, truthiness-to-bool, f-string assembly, and
//! `format(value, spec)` formatting for scalar/char/bool/str values. Extracted
//! verbatim from `lower.rs` (#1127); `use super::*` re-exposes the parent scope.

use super::*;

impl<'a> FnLower<'a> {
    /// Render a scalar/char/bool/str value to a `str` (the `str(x)` path).
    /// f-string fields with no format spec share this exact lowering, so
    /// `str(42)` and `f"{42}"` produce byte-identical output.
    /// Top-level string rendering (`str(x)` / `print(x)` / an f-string field):
    /// the `str`-mode form of the general Display protocol (#274, D-0124), where
    /// a bare string/char/bytes value renders WITHOUT quotes. Equivalent to
    /// `render_value(v, vt, RenderMode::Str)`.
    pub(super) fn render_str(&mut self, v: Value, vt: Ty) -> Result<Value, LowerError> {
        self.render_value(v, vt, RenderMode::Str)
    }

    /// Nested string rendering (a struct field, container element, or tuple
    /// element): the `repr`-mode form of the Display protocol (#274, D-0124),
    /// where a string renders Python-quoted (`'a'`), `bytes` as `b'…'`, and a
    /// `char` as its quoted one-character form — exactly as Python shows a value
    /// *inside* a container/`repr`. Every other type renders identically to
    /// `str` mode.
    pub(super) fn render_repr(&mut self, v: Value, vt: Ty) -> Result<Value, LowerError> {
        self.render_value(v, vt, RenderMode::Repr)
    }

    /// The single value-rendering protocol (#274, D-0124). `mode` selects the
    /// Python `str`/`repr` distinction, which affects ONLY leaf string/char/bytes
    /// values: in `Str` mode a bare string renders unquoted (top-level
    /// `print`/`str`), in `Repr` mode it renders quoted (`'a'`), as Python shows
    /// a value nested inside a container or struct. Aggregates (structs, tuples,
    /// containers) always render their children in `Repr` mode regardless of how
    /// the aggregate itself was reached; `Optional` propagates the ambient mode to
    /// its payload.
    pub(super) fn render_value(
        &mut self,
        v: Value,
        vt: Ty,
        mode: RenderMode,
    ) -> Result<Value, LowerError> {
        // `bytes`: `str` mode decodes the UTF-8 bytes (I#337, may raise on invalid
        // UTF-8, so it routes through `throwing_call`); `repr` mode renders the
        // Python `b'…'` byte-string form (no decode, never raises).
        if matches!(vt, Ty::Bytes) {
            let d = self.new_value(Ty::Ptr);
            match mode {
                RenderMode::Str => self.throwing_call(Some(d), "zap_bytes_to_str", vec![v]),
                RenderMode::Repr => self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_bytes_repr".to_string(),
                    args: vec![v],
                }),
            }
            return Ok(d);
        }
        // `Optional[T]` renders its payload when `Some`, and the literal
        // `"None"` when `None` (#498, matching Python's `f"{d.get(k)}"`). The
        // payload is rendered recursively in the SAME mode, so a nested
        // `Optional[str]` element quotes its payload; a non-renderable payload
        // falls through to the error below.
        if let Some(opt) = optional_layout(self.ctx, vt) {
            let slot = self.alloca(Ty::Ptr);
            let is_none = gen_opt_is_none(self, &opt, v);
            let none_bb = self.new_block();
            let some_bb = self.new_block();
            let join_bb = self.new_block();
            self.set_term(Term::BrIf {
                cond: is_none,
                then_bb: none_bb,
                else_bb: some_bb,
            });
            self.position(none_bb);
            let none_str = self.strconst("None");
            self.store(slot, none_str);
            self.set_term(Term::Br(join_bb));
            self.position(some_bb);
            let payload = gen_opt_payload(self, &opt, v);
            let some_str = self.render_value(payload, opt.payload_ty, mode)?;
            self.store(slot, some_str);
            self.set_term(Term::Br(join_bb));
            self.position(join_bb);
            return Ok(self.load(slot, Ty::Ptr));
        }
        let (func, arg) = match vt {
            // A bare `str`: unquoted in `str` mode (passthrough), Python-quoted in
            // `repr` mode (`'a'`, with the real quote-selection + escaping done by
            // the runtime helper).
            Ty::Ptr => match mode {
                RenderMode::Str => return Ok(v),
                RenderMode::Repr => ("zap_str_repr", v),
            },
            Ty::Bigint => {
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_bigint_to_str".to_string(),
                    args: vec![v],
                });
                return Ok(d);
            }
            Ty::Complex => {
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_complex_to_str".to_string(),
                    args: vec![v],
                });
                return Ok(d);
            }
            // A user struct (#274, D-0124): render via its `__str__`/`__repr__` /
            // `@derive(Repr)` / the compiler default. This is what makes
            // `str(point)` / `f"{point}"` work, not just `print(point)` — closing
            // the pre-#274 asymmetry where only `print` reached the struct path.
            // `Optional[T]` (also a `Ty::Struct`) was already handled above; a
            // tuple or tier-3 enum routes through `struct_to_str`. An interface
            // value (its synthetic `{data, itable}` fat struct) renders by
            // **dynamic dispatch** to the concrete type's renderer through the
            // itable's renderer slot (#707, D-0124), since its concrete type is
            // erased — not through the static `struct_to_str`. The struct's
            // renderer always quotes its own fields, so the ambient `mode` does
            // not matter here.
            Ty::Struct(sid) => {
                if let Some(iid) = self.ctx.iid_of_fat_sid(sid) {
                    return self.render_iface_value(v, iid);
                }
                return self.struct_to_str(sid, v, matches!(mode, RenderMode::Repr));
            }
            // A tier-1 enum (D-0018): the qualified canonical variant name,
            // `EnumName.VARIANT`, reusing the `.name` discriminant→name chain. An
            // unknown discriminant raises, exactly as `.name` does.
            Ty::Enum(id) => {
                let ename = self
                    .ctx
                    .enums
                    .values()
                    .find(|e| !e.is_sum && e.enum_id == id)
                    .map(|e| e.enum_name.clone())
                    .ok_or_else(|| err("enum identity is not registered"))?;
                let prefix = self.strconst(&format!("{ename}."));
                let (vname, _) = self.lower_enum_name(v, id)?;
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_concat".to_string(),
                    args: vec![prefix, vname],
                });
                return Ok(d);
            }
            // Native container rendering (#274, D-0124): `[v0, v1]`, `{k: v}`,
            // `{v0, v1}` with Python-exact empty forms `[]`, `{}`, `set()`.
            // Elements/keys/values render in `repr` mode (so string elements are
            // quoted), via a synthesized per-container-type renderer.
            Ty::List(_) | Ty::Dict(_, _) | Ty::Set(_) | Ty::VarTuple(_) => {
                let f = self.container_repr_fn(vt)?;
                let d = self.new_value(Ty::Ptr);
                self.throwing_call(Some(d), &f, vec![v]);
                return Ok(d);
            }
            // A `char` (#709): `str` mode renders its one-codepoint string;
            // `repr` mode renders that string Python-quoted (`'c'`). An `int32`
            // is NOT a char — it renders as a decimal through the integer arm
            // below (fixing the pre-#709 conflation where an `int32` scalar /
            // field rendered as a character).
            Ty::Char => match mode {
                RenderMode::Str => ("zap_char_to_str", v),
                RenderMode::Repr => {
                    let s = self.new_value(Ty::Ptr);
                    self.emit(Inst::Call {
                        dest: Some(s),
                        func: "zap_char_to_str".to_string(),
                        args: vec![v],
                    });
                    ("zap_str_repr", s)
                }
            },
            Ty::I1 => ("zap_bool_to_str", self.coerce(v, vt, Ty::I64)),
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 => ("zap_i64_to_str", self.coerce(v, vt, Ty::I64)),
            // Unsigned integers render as *unsigned* decimal (no phantom sign;
            // a `uint64` above `INT64_MAX` prints its true magnitude). The value
            // is zero-extended to a 64-bit word first (#552). repr == str.
            Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 => ("zap_u64_to_str", self.coerce(v, vt, Ty::U64)),
            Ty::F32 | Ty::F64 => ("zap_f64_to_str", self.as_f64(v, vt)),
            _ => return Err(err("cannot render this value as a string")),
        };
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args: vec![arg],
        });
        Ok(d)
    }

    /// Python-style truthiness: a number/char is falsey at zero, a str when
    /// empty, a bool is itself.
    pub(super) fn value_to_bool(&mut self, v: Value, vt: Ty) -> Result<Value, LowerError> {
        match vt {
            Ty::I1 => Ok(v),
            // Unsigned integers are falsey at zero exactly like signed ones; the
            // `!= 0` test is sign-agnostic (#552).
            Ty::I8
            | Ty::I16
            | Ty::I32
            | Ty::Char
            | Ty::I64
            | Ty::U8
            | Ty::U16
            | Ty::U32
            | Ty::U64 => {
                let z = self.iconst(vt, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: CmpPred::Ne,
                    lhs: v,
                    rhs: z,
                });
                Ok(d)
            }
            Ty::F32 | Ty::F64 => {
                let fv = self.as_f64(v, vt);
                let z = self.fconst(Ty::F64, 0.0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::FCmp {
                    dest: d,
                    pred: CmpPred::Ne,
                    lhs: fv,
                    rhs: z,
                });
                Ok(d)
            }
            // A non-empty `str` or `bytes` is truthy: compare byte length to 0.
            Ty::Ptr | Ty::Bytes => {
                let len = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(len),
                    func: "zap_str_len".to_string(),
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
                Ok(d)
            }
            _ => Err(err("bool() expects a str, bytes, char, number or bool")),
        }
    }

    /// Lower an f-string (D-0026) into a left-folded `zap_str_concat` of its
    /// literal runs and formatted fields. The split and spec parsing are shared
    /// with the type checker via `zapc_syntax::fstring`.
    pub(super) fn lower_fstring(
        &mut self,
        value: &str,
        raw: bool,
        span_start: u32,
        mono_fields: &[ast::Expr],
    ) -> Result<(Value, Ty), LowerError> {
        let base = span_start.saturating_add(2);
        let (parts, errors) = fstring::split_fstring(value, base);
        if let Some(e) = errors.into_iter().next() {
            // sema already reported these with proper spans; this is defensive.
            return Err(err(e.message));
        }
        // Monomorphization records the specialized field expressions here, since
        // it cannot see the ones re-parsed above (#498). Use them positionally
        // when present (their count matches the re-split fields), so inline
        // generics / `dict.get` in a field lower against concrete types. When
        // absent (nothing needed specializing, or the recording was skipped) the
        // freshly-parsed field expressions are used.
        let use_mono = mono_fields.len()
            == parts
                .iter()
                .filter(|p| matches!(p, FStringPart::Field { .. }))
                .count();
        let mut field_idx = 0usize;
        let mut pieces: Vec<Value> = Vec::new();
        // A compile-time UPPER BOUND on the joined byte length (#1081). Starts at
        // `Some(0)` and accumulates each piece's maximum width; it becomes `None`
        // the moment a piece's length is not statically bounded (an interpolated
        // string, a float, an enum, a spec'd/padded field). Over-estimating a
        // width is always safe — it only forgoes the no-root optimization — while
        // under-estimating would let a run-time heap spill go unrooted (a UAF), so
        // every uncertain piece drops the bound to `None`.
        let mut max_len: Option<u64> = Some(0);
        for part in &parts {
            let piece = match part {
                FStringPart::Text(t) => {
                    let data = if raw {
                        t.clone().into_bytes()
                    } else {
                        unescape(t, false)?
                    };
                    max_len = max_len.map(|acc| acc.saturating_add(data.len() as u64));
                    let d = self.new_value(Ty::Ptr);
                    self.emit(Inst::StrConst {
                        dest: d,
                        bytes: data,
                    });
                    d
                }
                FStringPart::Field { expr, spec, .. } => {
                    let field_expr = if use_mono {
                        &mono_fields[field_idx]
                    } else {
                        expr
                    };
                    field_idx += 1;
                    let (v, vt) = self.expr(field_expr)?;
                    let w = fstring_render_max_len(vt, spec.is_some());
                    max_len = max_len.and_then(|acc| w.map(|w| acc.saturating_add(w)));
                    match spec {
                        None => self.render_str(v, vt)?,
                        Some(fs) => self.format_value(v, vt, fs)?,
                    }
                }
            };
            pieces.push(piece);
        }
        // Join the pieces in a single pass (#839; folds in the #837 lever-3(b)
        // single-alloc join). Zero pieces -> empty literal; a single piece is
        // already the whole string (no join / extra allocation). Two or more ->
        // one `StrBuild`, which the escape pass may later stack-allocate when the
        // result is proven non-escaping (the `f"k{i}"` transient-key win).
        let result = match pieces.len() {
            0 => {
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::StrConst {
                    dest: d,
                    bytes: Vec::new(),
                });
                d
            }
            1 => pieces[0],
            _ => {
                // Bounded iff the whole join provably fits the stack buffer, so a
                // stack promotion of it can never spill and needs no GC root
                // (#1081). The escape pass sets `stack`/`cap`; `bounded` only
                // gates whether codegen may drop the root.
                let bounded = max_len.is_some_and(|l| l <= STR_STACK_CAP as u64);
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::StrBuild {
                    dest: d,
                    parts: pieces,
                    stack: false,
                    bounded,
                    cap: 0,
                });
                d
            }
        };
        Ok((result, Ty::Ptr))
    }

    /// Render a value through a format spec (D-0026). Dispatches to the runtime
    /// `zap_fmt_int`/`zap_fmt_flt`/`zap_fmt_str` with the spec decomposed into
    /// integer constants.
    pub(super) fn format_value(
        &mut self,
        v: Value,
        vt: Ty,
        spec: &FormatSpec,
    ) -> Result<Value, LowerError> {
        let align_code = match spec.align {
            Some(Align::Left) => '<' as i64,
            Some(Align::Right) => '>' as i64,
            Some(Align::Center) => '^' as i64,
            Some(Align::SignAware) => '=' as i64,
            None => 0,
        };
        let sign_code = match spec.sign {
            '+' => 1,
            ' ' => 2,
            _ => 0,
        };
        let fill_code = spec.fill as i64;
        let width = spec.width.map(|w| w as i64).unwrap_or(-1);
        let prec = spec.precision.map(|p| p as i64).unwrap_or(-1);

        // Choose the rendering family: an explicit type wins; otherwise the
        // value's own kind decides (int/float numeric, char/bool/str textual).
        let want_int = match spec.ty {
            Some(t) if FormatSpec::is_int_type(t) => true,
            Some(t) if FormatSpec::is_float_type(t) || t == 's' => false,
            _ => matches!(vt, Ty::I8 | Ty::I16 | Ty::I64),
        };
        let want_flt = match spec.ty {
            Some(t) if FormatSpec::is_float_type(t) => true,
            Some(_) => false,
            None => matches!(vt, Ty::F32 | Ty::F64),
        };

        if want_int {
            let (base, upper) = match spec.ty {
                Some('b') => (2, 0),
                Some('o') => (8, 0),
                Some('x') => (16, 0),
                Some('X') => (16, 1),
                _ => (10, 0),
            };
            let iv = self.coerce(v, vt, Ty::I64);
            let b = self.iconst(Ty::I64, base);
            let up = self.iconst(Ty::I64, upper);
            let alt = self.iconst(Ty::I64, spec.alt as i64);
            let sign = self.iconst(Ty::I64, sign_code);
            let w = self.iconst(Ty::I64, width);
            let fill = self.iconst(Ty::I64, fill_code);
            let al = self.iconst(Ty::I64, align_code);
            let zero = self.iconst(Ty::I64, spec.zero as i64);
            let d = self.new_value(Ty::Ptr);
            self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_fmt_int".to_string(),
                args: vec![iv, b, up, alt, sign, w, fill, al, zero],
            });
            return Ok(d);
        }

        if want_flt {
            let ftype = match spec.ty {
                Some('e') => 'e' as i64,
                Some('E') => 'E' as i64,
                Some('f') | Some('F') => 'f' as i64,
                _ => 'g' as i64, // 'g'/'G'/none default to compact
            };
            let fv = self.as_f64(v, vt);
            let ft = self.iconst(Ty::I64, ftype);
            let sign = self.iconst(Ty::I64, sign_code);
            let w = self.iconst(Ty::I64, width);
            let p = self.iconst(Ty::I64, prec);
            let fill = self.iconst(Ty::I64, fill_code);
            let al = self.iconst(Ty::I64, align_code);
            let zero = self.iconst(Ty::I64, spec.zero as i64);
            let d = self.new_value(Ty::Ptr);
            self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_fmt_flt".to_string(),
                args: vec![fv, ft, sign, w, p, fill, al, zero],
            });
            return Ok(d);
        }

        // Textual: render char/bool/str to a string, then pad/truncate.
        let s = self.render_str(v, vt)?;
        let w = self.iconst(Ty::I64, width);
        let p = self.iconst(Ty::I64, prec);
        let fill = self.iconst(Ty::I64, fill_code);
        let al = self.iconst(Ty::I64, align_code);
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(d),
            func: "zap_fmt_str".to_string(),
            args: vec![s, w, p, fill, al],
        });
        Ok(d)
    }
}
