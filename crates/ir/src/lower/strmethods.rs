//! String-method and string/bytes-view lowering.
//!
//! `str` method lowering to the `zap_str_*` runtime entry points (ticket #284,
//! D-0025), the string/bytes **view** machinery (#1082) — the `&self` view
//! accessors, the `build_strview`/`build_bytesview` constructors, the field
//! readers, and `lower_strview_method`/`lower_bytesview_method` — plus
//! `lower_match_method` (regex match-object `.group(...)` dispatch). The
//! interner-side `strview_sid`/`bytesview_sid` (`&mut self`) stay in `lower.rs`
//! with the other interners. Extracted verbatim from `lower.rs` (#1151);
//! `use super::*` re-exposes the parent scope.

use super::*;

impl<'a> FnLower<'a> {
    /// `str` methods (ticket #284, D-0025). The receiver `recv` is the string
    /// (`Ptr`). Sema has already type-checked the method name, arity, and
    /// argument types, so this is a mechanical lowering to the `zap_str_*`
    /// runtime entry points. Case folding / whitespace are ASCII in v1.
    pub(super) fn lower_str_method(
        &mut self,
        recv: Value,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        // Lower a single `str`-typed argument (its ZIR type is `Ptr`).
        let one_str = |s: &mut Self| -> Result<Value, LowerError> {
            if args.len() != 1 {
                return Err(err(format!("str.{name}() takes exactly one argument")));
            }
            let (v, _t) = s.expr(&args[0].value)?;
            Ok(v)
        };
        // `s.view()` / `s.view(lo)` / `s.view(lo, hi)` → a borrowed `strview`
        // (#1082): a by-value tuple `(backing, byte_off, byte_len)`. The byte
        // bounds are computed once, in the runtime, from the codepoint indices
        // (D-0025, Python clamping incl. negatives); `has_lo`/`has_hi` make the
        // zero-arg full view fall out of the same two calls. No owned alloc, no
        // copy — the tuple aliases `s`'s bytes.
        if name == "view" {
            if args.len() > 2 {
                return Err(err("str.view() takes 0, 1, or 2 arguments"));
            }
            let (lo, has_lo) = match args.first() {
                Some(a) => (self.expr(&a.value)?.0, self.iconst(Ty::I64, 1)),
                None => (self.iconst(Ty::I64, 0), self.iconst(Ty::I64, 0)),
            };
            let (hi, has_hi) = match args.get(1) {
                Some(a) => (self.expr(&a.value)?.0, self.iconst(Ty::I64, 1)),
                None => (self.iconst(Ty::I64, 0), self.iconst(Ty::I64, 0)),
            };
            return Ok((
                self.build_strview(recv, lo, has_lo, hi, has_hi),
                self.strview_ty(),
            ));
        }
        match name {
            "upper" | "lower" => {
                if !args.is_empty() {
                    return Err(err(format!("str.{name}() takes no arguments")));
                }
                let func = if name == "upper" {
                    "zap_str_upper"
                } else {
                    "zap_str_lower"
                };
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: func.to_string(),
                    args: vec![recv],
                });
                Ok((d, Ty::Ptr))
            }
            "strip" | "lstrip" | "rstrip" => {
                if args.len() > 1 {
                    return Err(err(format!("str.{name}() takes 0 or 1 arguments")));
                }
                let mode = match name {
                    "strip" => 0,
                    "lstrip" => 1,
                    _ => 2,
                };
                let m = self.iconst(Ty::I64, mode);
                let d = self.new_value(Ty::Ptr);
                if let Some(a) = args.first() {
                    // strip(chars): trim any byte in the explicit set (#971).
                    let (cs, _) = self.expr(&a.value)?;
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_str_strip_chars".to_string(),
                        args: vec![recv, cs, m],
                    });
                } else {
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_str_strip".to_string(),
                        args: vec![recv, m],
                    });
                }
                Ok((d, Ty::Ptr))
            }
            "startswith" | "endswith" | "contains" => {
                let a = one_str(self)?;
                let func = match name {
                    "startswith" => "zap_str_startswith",
                    "endswith" => "zap_str_endswith",
                    _ => "zap_str_contains",
                };
                let raw = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: func.to_string(),
                    args: vec![recv, a],
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: CmpPred::Ne,
                    lhs: raw,
                    rhs: zero,
                });
                Ok((d, Ty::I1))
            }
            "find" | "index" => {
                let a = one_str(self)?;
                let d = self.new_value(Ty::I64);
                if name == "find" {
                    // `find` returns -1 when absent — infallible.
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_str_find".to_string(),
                        args: vec![recv, a],
                    });
                } else {
                    // `index` raises a catchable `ValueError` when absent (#1566),
                    // so it must carry an unwind edge to a `try` in this frame.
                    self.throwing_call(Some(d), "zap_str_index_of", vec![recv, a]);
                }
                Ok((d, Ty::I64))
            }
            "replace" => {
                if args.len() != 2 {
                    return Err(err("str.replace() takes exactly two arguments"));
                }
                let (o, _) = self.expr(&args[0].value)?;
                let (n, _) = self.expr(&args[1].value)?;
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_replace".to_string(),
                    args: vec![recv, o, n],
                });
                Ok((d, Ty::Ptr))
            }
            "split" | "rsplit" => {
                // 0 args: whitespace split. 1 arg: explicit separator. 2 args:
                // separator + int maxsplit (#971). maxsplit defaults to -1.
                if args.len() > 2 {
                    return Err(err(format!("str.{name}() takes 0, 1, or 2 arguments")));
                }
                let d = self.new_value(Ty::List(Elem::Ptr));
                let maxsplit = if let Some(a) = args.get(1) {
                    let (v, vt) = self.expr(&a.value)?;
                    self.coerce(v, vt, Ty::I64)
                } else {
                    self.iconst(Ty::I64, -1)
                };
                if let Some(a) = args.first() {
                    let (sep, _) = self.expr(&a.value)?;
                    let func = if name == "split" {
                        "zap_str_split_sep"
                    } else {
                        "zap_str_rsplit_sep"
                    };
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: func.to_string(),
                        args: vec![recv, sep, maxsplit],
                    });
                } else {
                    let func = if name == "split" {
                        "zap_str_split_ws"
                    } else {
                        "zap_str_rsplit_ws"
                    };
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: func.to_string(),
                        args: vec![recv, maxsplit],
                    });
                }
                Ok((d, Ty::List(Elem::Ptr)))
            }
            "splitlines" => {
                if !args.is_empty() {
                    return Err(err("str.splitlines() takes no arguments"));
                }
                let d = self.new_value(Ty::List(Elem::Ptr));
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_splitlines".to_string(),
                    args: vec![recv],
                });
                Ok((d, Ty::List(Elem::Ptr)))
            }
            "count" | "rfind" | "rindex" => {
                let a = one_str(self)?;
                let d = self.new_value(Ty::I64);
                if name == "rindex" {
                    // `rindex` raises a catchable `ValueError` when absent
                    // (#1566); `count`/`rfind` are infallible.
                    self.throwing_call(Some(d), "zap_str_rindex", vec![recv, a]);
                } else {
                    let func = if name == "count" {
                        "zap_str_count"
                    } else {
                        "zap_str_rfind"
                    };
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: func.to_string(),
                        args: vec![recv, a],
                    });
                }
                Ok((d, Ty::I64))
            }
            "removeprefix" | "removesuffix" => {
                let a = one_str(self)?;
                let func = if name == "removeprefix" {
                    "zap_str_removeprefix"
                } else {
                    "zap_str_removesuffix"
                };
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: func.to_string(),
                    args: vec![recv, a],
                });
                Ok((d, Ty::Ptr))
            }
            // partition / rpartition (#1029, D-0218): split on the first / last
            // occurrence of the separator into a (head, sep, tail) tuple. The
            // runtime returns a GC result object (throwing — an empty separator
            // raises ValueError); three getters feed an interned (str, str, str)
            // tuple, the same aggregate path as `process.run`.
            "partition" | "rpartition" => {
                let sep = one_str(self)?;
                let func = if name == "partition" {
                    "zap_str_partition"
                } else {
                    "zap_str_rpartition"
                };
                let h = self.new_value(Ty::Ptr);
                self.throwing_call(Some(h), func, vec![recv, sep]);
                let mut fields = Vec::with_capacity(3);
                for getter in ["zap_str_part_head", "zap_str_part_sep", "zap_str_part_tail"] {
                    let v = self.new_value(Ty::Ptr);
                    self.emit(Inst::Call {
                        dest: Some(v),
                        func: getter.to_string(),
                        args: vec![h],
                    });
                    fields.push(v);
                }
                let sid = self
                    .ctx
                    .tuples
                    .borrow_mut()
                    .intern(vec![Ty::Ptr, Ty::Ptr, Ty::Ptr]);
                let dest = self.new_value(Ty::Struct(sid));
                self.emit(Inst::StructNew { dest, sid });
                for (idx, val) in fields.into_iter().enumerate() {
                    self.emit(Inst::FieldSet {
                        base: dest,
                        sid,
                        idx: idx as u32,
                        val,
                    });
                }
                Ok((dest, Ty::Struct(sid)))
            }
            "ljust" | "rjust" | "center" => {
                // width (int) + optional fill (str); default fill is a space.
                if args.is_empty() || args.len() > 2 {
                    return Err(err(format!("str.{name}() takes 1 or 2 arguments")));
                }
                let (wv, wt) = self.expr(&args[0].value)?;
                let width = self.coerce(wv, wt, Ty::I64);
                let fill = if let Some(a) = args.get(1) {
                    let (v, _) = self.expr(&a.value)?;
                    v
                } else {
                    let e = self.new_value(Ty::Ptr);
                    self.emit(Inst::StrConst {
                        dest: e,
                        bytes: b" ".to_vec(),
                    });
                    e
                };
                let func = match name {
                    "ljust" => "zap_str_ljust",
                    "rjust" => "zap_str_rjust",
                    _ => "zap_str_center",
                };
                // A fill that is not exactly one codepoint raises a catchable
                // TypeError (#1648), so this must be a throwing call for
                // `try`/`except` to intercept it.
                let d = self.new_value(Ty::Ptr);
                self.throwing_call(Some(d), func, vec![recv, width, fill]);
                Ok((d, Ty::Ptr))
            }
            "zfill" => {
                if args.len() != 1 {
                    return Err(err("str.zfill() takes exactly one argument"));
                }
                let (wv, wt) = self.expr(&args[0].value)?;
                let width = self.coerce(wv, wt, Ty::I64);
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_zfill".to_string(),
                    args: vec![recv, width],
                });
                Ok((d, Ty::Ptr))
            }
            // expandtabs([tabsize]): optional int tab stop, default 8 (#1034).
            // Grows a temp buffer (zap_strbuf_ensure), which raises a catchable
            // `MemoryError` on allocation failure (#1581), so the call carries an
            // unwind edge inside a `try`.
            "expandtabs" => {
                if args.len() > 1 {
                    return Err(err("str.expandtabs() takes 0 or 1 arguments"));
                }
                let tabsize = if let Some(a) = args.first() {
                    let (v, vt) = self.expr(&a.value)?;
                    self.coerce(v, vt, Ty::I64)
                } else {
                    self.iconst(Ty::I64, 8)
                };
                let d = self.new_value(Ty::Ptr);
                self.throwing_call(Some(d), "zap_str_expandtabs", vec![recv, tabsize]);
                Ok((d, Ty::Ptr))
            }
            // translate(table): dict[str,str] char map. The character substitution
            // never raises, but the temp buffer it grows (zap_strbuf_ensure) can
            // raise a catchable `MemoryError` on allocation failure (#1581), so the
            // call carries an unwind edge inside a `try`.
            "translate" => {
                if args.len() != 1 {
                    return Err(err("str.translate() takes exactly one argument"));
                }
                let (table, _) = self.expr(&args[0].value)?;
                let d = self.new_value(Ty::Ptr);
                self.throwing_call(Some(d), "zap_str_translate", vec![recv, table]);
                Ok((d, Ty::Ptr))
            }
            // format_map(mapping): dict[str,str] field fill. Raises KeyError on a
            // missing field / ValueError on a malformed template, so it is a
            // throwing call that `try` can catch (#1034).
            "format_map" => {
                if args.len() != 1 {
                    return Err(err("str.format_map() takes exactly one argument"));
                }
                let (m, _) = self.expr(&args[0].value)?;
                let d = self.new_value(Ty::Ptr);
                self.throwing_call(Some(d), "zap_str_format_map", vec![recv, m]);
                Ok((d, Ty::Ptr))
            }
            "title" | "capitalize" | "casefold" | "swapcase" => {
                if !args.is_empty() {
                    return Err(err(format!("str.{name}() takes no arguments")));
                }
                let func = match name {
                    "title" => "zap_str_title",
                    "capitalize" => "zap_str_capitalize",
                    "casefold" => "zap_str_casefold",
                    _ => "zap_str_swapcase",
                };
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: func.to_string(),
                    args: vec![recv],
                });
                Ok((d, Ty::Ptr))
            }
            "isdigit" | "isalpha" | "isalnum" | "isspace" | "islower" | "isupper" => {
                if !args.is_empty() {
                    return Err(err(format!("str.{name}() takes no arguments")));
                }
                let func = match name {
                    "isdigit" => "zap_str_isdigit",
                    "isalpha" => "zap_str_isalpha",
                    "isalnum" => "zap_str_isalnum",
                    "isspace" => "zap_str_isspace",
                    "islower" => "zap_str_islower",
                    _ => "zap_str_isupper",
                };
                let raw = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: func.to_string(),
                    args: vec![recv],
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: CmpPred::Ne,
                    lhs: raw,
                    rhs: zero,
                });
                Ok((d, Ty::I1))
            }
            "join" => {
                // `sep.join(parts)`: the receiver is the separator; the single
                // argument is a `list[str]`.
                if args.len() != 1 {
                    return Err(err("str.join() takes exactly one argument"));
                }
                let (lv, _lt) = self.expr(&args[0].value)?;
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_join".to_string(),
                    args: vec![recv, lv],
                });
                Ok((d, Ty::Ptr))
            }
            "encode" => {
                // UTF-8 byte values as a `list[int]` — the reachable byte view
                // for SigV4 until the full `bytes` type lands.
                if !args.is_empty() {
                    return Err(err("str.encode() takes no arguments"));
                }
                let d = self.new_value(Ty::List(Elem::I64));
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_encode".to_string(),
                    args: vec![recv],
                });
                Ok((d, Ty::List(Elem::I64)))
            }
            _ => Err(err(format!("str has no method '{name}'"))),
        }
    }

    /// The interned tuple id for a borrowed `strview` — `(backing: str,
    /// byte_off: int64, byte_len: int64)` (#1082). Always interns the same three
    /// field types, so the id is stable across the module.
    fn strview_sid(&self) -> u32 {
        self.ctx.tuples.borrow_mut().strview_sid()
    }

    /// The ZIR type of a `strview` value: the reserved aggregate layout.
    fn strview_ty(&self) -> Ty {
        Ty::Struct(self.strview_sid())
    }

    /// Is `sid` the reserved `strview` layout? A pure read that does **not**
    /// allocate the layout, so probing an arbitrary struct id during dispatch
    /// never materializes a stray `strview`. The reserved id is a `push_layout`
    /// id, so it is distinct from any natural `(str, int64, int64)` tuple.
    pub(super) fn is_strview_sid(&self, sid: u32) -> bool {
        self.ctx.tuples.borrow().is_strview(sid)
    }

    fn bytesview_sid(&self) -> u32 {
        self.ctx.tuples.borrow_mut().bytesview_sid()
    }

    pub(super) fn bytesview_ty(&self) -> Ty {
        Ty::Struct(self.bytesview_sid())
    }

    /// Is `sid` the reserved `bytesview` layout (pure read)?
    pub(super) fn is_bytesview_sid(&self, sid: u32) -> bool {
        self.ctx.tuples.borrow().is_bytesview(sid)
    }

    /// Is `sid` either borrowed-view layout (`strview` or `bytesview`)? The
    /// read-only consumers (`==`, hash, dict probe, `print`) treat both uniformly
    /// because both are a traced backing pointer + byte offset/length over the
    /// same `ZapStr` storage; only creation (codepoint vs byte bounds) and `len`
    /// differ.
    pub(super) fn is_any_view_sid(&self, sid: u32) -> bool {
        self.is_strview_sid(sid) || self.is_bytesview_sid(sid)
    }

    /// Build a `bytesview` value (#1082 follow-up). Bounds are **byte** indices
    /// (I#337), so the runtime does no UTF-8 walk — just Python clamping over the
    /// backing's byte length.
    pub(super) fn build_bytesview(
        &mut self,
        backing: Value,
        lo: Value,
        has_lo: Value,
        hi: Value,
        has_hi: Value,
    ) -> Value {
        let off = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(off),
            func: "zap_bytesview_off".to_string(),
            args: vec![backing, lo, has_lo],
        });
        let len = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(len),
            func: "zap_bytesview_len".to_string(),
            args: vec![backing, off, hi, has_hi],
        });
        let sid = self.bytesview_sid();
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        for (idx, v) in [backing, off, len].into_iter().enumerate() {
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: idx as u32,
                val: v,
            });
        }
        dest
    }

    /// Build a `strview` value `(backing, byte_off, byte_len)` (#1082). The byte
    /// bounds are computed in the runtime from the codepoint indices `lo`/`hi`
    /// (present per `has_lo`/`has_hi`), so a full view (`has_* == 0`) yields
    /// `off = 0`, `len = backing->len`. No owned allocation and no byte copy.
    fn build_strview(
        &mut self,
        backing: Value,
        lo: Value,
        has_lo: Value,
        hi: Value,
        has_hi: Value,
    ) -> Value {
        let off = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(off),
            func: "zap_strview_off".to_string(),
            args: vec![backing, lo, has_lo],
        });
        let len = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(len),
            func: "zap_strview_len".to_string(),
            args: vec![backing, off, hi, has_hi],
        });
        let sid = self.strview_sid();
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        for (idx, v) in [backing, off, len].into_iter().enumerate() {
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: idx as u32,
                val: v,
            });
        }
        dest
    }

    /// Extract the `(backing, byte_off, byte_len)` fields of a `strview` value,
    /// for passing to a `(base, off, len)` view-ABI runtime entry point (#1082).
    pub(super) fn strview_fields(&mut self, recv: Value) -> (Value, Value, Value) {
        let sid = self.strview_sid();
        self.view_fields(recv, sid)
    }

    /// Extract the three fields of any borrowed view (`strview` / `bytesview`),
    /// given its reserved layout `sid`. Field 0 (the backing) keeps its layout
    /// type — `str` (`Ptr`) or `bytes` — so codegen's `FieldGet` type-checks;
    /// both are a single pointer word passed as `void*` to the view ABI.
    pub(super) fn view_fields(&mut self, recv: Value, sid: u32) -> (Value, Value, Value) {
        let base_ty = self
            .ctx
            .tuples
            .borrow()
            .fields(sid)
            .and_then(|f| f.first().copied())
            .unwrap_or(Ty::Ptr);
        let base = self.new_value(base_ty);
        self.emit(Inst::FieldGet {
            dest: base,
            base: recv,
            sid,
            idx: 0,
        });
        let off = self.new_value(Ty::I64);
        self.emit(Inst::FieldGet {
            dest: off,
            base: recv,
            sid,
            idx: 1,
        });
        let len = self.new_value(Ty::I64);
        self.emit(Inst::FieldGet {
            dest: len,
            base: recv,
            sid,
            idx: 2,
        });
        (base, off, len)
    }

    /// Lower a method call on a borrowed `strview` receiver (#1082). Sema has
    /// already restricted the name/arity to the read-only consumer subset. Each
    /// method extracts the `(base, off, len)` triple and calls a `_view` runtime
    /// entry point that operates on the borrowed bytes with **no owned allocation
    /// and no root** — the whole point of the borrow.
    pub(super) fn lower_strview_method(
        &mut self,
        recv: Value,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let (base, off, len) = self.strview_fields(recv);
        // A single `str`/`strview` argument, materialized to a `(base, off, len)`
        // triple so the view probes compare borrowed bytes against borrowed bytes.
        let one_arg_triple = |s: &mut Self| -> Result<(Value, Value, Value), LowerError> {
            if args.len() != 1 {
                return Err(err(format!("strview.{name}() takes exactly one argument")));
            }
            let (v, t) = s.expr(&args[0].value)?;
            Ok(match t {
                // A `strview` argument: already a triple.
                Ty::Struct(sid) if s.is_strview_sid(sid) => s.strview_fields(v),
                // A `str` argument: a full view over it (off 0, len = ->len).
                _ => {
                    let z = s.iconst(Ty::I64, 0);
                    let full = s.build_strview(v, z, z, z, z);
                    s.strview_fields(full)
                }
            })
        };
        match name {
            "startswith" | "endswith" | "contains" => {
                let (abase, aoff, alen) = one_arg_triple(self)?;
                let func = match name {
                    "startswith" => "zap_strview_startswith",
                    "endswith" => "zap_strview_endswith",
                    _ => "zap_strview_contains",
                };
                let raw = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: func.to_string(),
                    args: vec![base, off, len, abase, aoff, alen],
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: CmpPred::Ne,
                    lhs: raw,
                    rhs: zero,
                });
                Ok((d, Ty::I1))
            }
            // Codepoint length of the borrowed range (Python `len`, D-0025).
            "len" => {
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_strview_cp_count".to_string(),
                    args: vec![base, off, len],
                });
                Ok((d, Ty::I64))
            }
            // `v.__hash__()` — seeded wyhash of the borrowed bytes, identical to
            // the hash of the materialized `str` (and to the dict/set key hash),
            // with no owned allocation (#1082/#1064).
            "__hash__" => {
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_strview_hash".to_string(),
                    args: vec![base, off, len],
                });
                Ok((d, Ty::I64))
            }
            // Re-slice into a narrower view: `v.view(lo, hi)` (#1082). A view of a
            // view resolves to a view of the original backing with composed byte
            // bounds, handled by the runtime `zap_strview_sub`.
            "view" => {
                if args.len() > 2 {
                    return Err(err("strview.view() takes 0, 1, or 2 arguments"));
                }
                let (lo, has_lo) = match args.first() {
                    Some(a) => (self.expr(&a.value)?.0, self.iconst(Ty::I64, 1)),
                    None => (self.iconst(Ty::I64, 0), self.iconst(Ty::I64, 0)),
                };
                let (hi, has_hi) = match args.get(1) {
                    Some(a) => (self.expr(&a.value)?.0, self.iconst(Ty::I64, 1)),
                    None => (self.iconst(Ty::I64, 0), self.iconst(Ty::I64, 0)),
                };
                // Sub-view byte offset within the backing, then a length, both
                // relative to the original backing — reuse the creation helpers
                // with the view's own byte window as the base window.
                let noff = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(noff),
                    func: "zap_strview_sub_off".to_string(),
                    args: vec![base, off, len, lo, has_lo],
                });
                let nlen = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(nlen),
                    func: "zap_strview_sub_len".to_string(),
                    args: vec![base, off, len, noff, hi, has_hi],
                });
                let sid = self.strview_sid();
                let dest = self.new_value(Ty::Struct(sid));
                self.emit(Inst::StructNew { dest, sid });
                for (idx, v) in [base, noff, nlen].into_iter().enumerate() {
                    self.emit(Inst::FieldSet {
                        base: dest,
                        sid,
                        idx: idx as u32,
                        val: v,
                    });
                }
                Ok((dest, Ty::Struct(sid)))
            }
            // Materialize an owned `str` copy — the one allocating method, the
            // escape hatch out of the borrow.
            "to_str" => {
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_strview_to_str".to_string(),
                    args: vec![base, off, len],
                });
                Ok((d, Ty::Ptr))
            }
            _ => Err(err(format!("strview has no method '{name}'"))),
        }
    }

    /// Lower a method call on a borrowed `bytesview` receiver (#1082 follow-up).
    /// The probes reuse the raw-byte `zap_strview_*` entry points (a `bytesview`
    /// and a `strview` have identical `(base, off, len)` storage); only creation
    /// / sub-view use the byte-indexed `zap_bytesview_*` helpers, and `len` is the
    /// byte count (the length field itself). `to_bytes` materializes an owned
    /// `bytes`.
    pub(super) fn lower_bytesview_method(
        &mut self,
        recv: Value,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let bvsid = self.bytesview_sid();
        let (base, off, len) = self.view_fields(recv, bvsid);
        // A single `bytes`/`bytesview` argument as a `(base, off, len)` triple.
        let one_arg_triple = |s: &mut Self| -> Result<(Value, Value, Value), LowerError> {
            if args.len() != 1 {
                return Err(err(format!(
                    "bytesview.{name}() takes exactly one argument"
                )));
            }
            let (v, t) = s.expr(&args[0].value)?;
            Ok(match t {
                Ty::Struct(sid) if s.is_bytesview_sid(sid) => s.view_fields(v, sid),
                // A `bytes` argument: a full view over it (off 0, len = ->len).
                _ => {
                    let z = s.iconst(Ty::I64, 0);
                    let full = s.build_bytesview(v, z, z, z, z);
                    s.view_fields(full, bvsid)
                }
            })
        };
        match name {
            "startswith" | "endswith" | "contains" => {
                let (abase, aoff, alen) = one_arg_triple(self)?;
                let func = match name {
                    "startswith" => "zap_strview_startswith",
                    "endswith" => "zap_strview_endswith",
                    _ => "zap_strview_contains",
                };
                let raw = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: func.to_string(),
                    args: vec![base, off, len, abase, aoff, alen],
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: CmpPred::Ne,
                    lhs: raw,
                    rhs: zero,
                });
                Ok((d, Ty::I1))
            }
            // Byte length of the borrowed range (I#337): the length field itself.
            "len" => Ok((len, Ty::I64)),
            "__hash__" => {
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_strview_hash".to_string(),
                    args: vec![base, off, len],
                });
                Ok((d, Ty::I64))
            }
            // Sub-view (byte-indexed): `v.view(lo, hi)`.
            "view" => {
                if args.len() > 2 {
                    return Err(err("bytesview.view() takes 0, 1, or 2 arguments"));
                }
                let (lo, has_lo) = match args.first() {
                    Some(a) => (self.expr(&a.value)?.0, self.iconst(Ty::I64, 1)),
                    None => (self.iconst(Ty::I64, 0), self.iconst(Ty::I64, 0)),
                };
                let (hi, has_hi) = match args.get(1) {
                    Some(a) => (self.expr(&a.value)?.0, self.iconst(Ty::I64, 1)),
                    None => (self.iconst(Ty::I64, 0), self.iconst(Ty::I64, 0)),
                };
                let noff = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(noff),
                    func: "zap_bytesview_sub_off".to_string(),
                    args: vec![base, off, len, lo, has_lo],
                });
                let nlen = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(nlen),
                    func: "zap_bytesview_sub_len".to_string(),
                    args: vec![base, off, len, noff, hi, has_hi],
                });
                let dest = self.new_value(Ty::Struct(bvsid));
                self.emit(Inst::StructNew { dest, sid: bvsid });
                for (idx, v) in [base, noff, nlen].into_iter().enumerate() {
                    self.emit(Inst::FieldSet {
                        base: dest,
                        sid: bvsid,
                        idx: idx as u32,
                        val: v,
                    });
                }
                Ok((dest, Ty::Struct(bvsid)))
            }
            // Materialize an owned `bytes` copy (one contiguous ZapStr, typed
            // `bytes`) — the escape hatch out of the borrow.
            "to_bytes" => {
                let d = self.new_value(Ty::Bytes);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_strview_to_str".to_string(),
                    args: vec![base, off, len],
                });
                Ok((d, Ty::Bytes))
            }
            _ => Err(err(format!("bytesview has no method '{name}'"))),
        }
    }

    /// Methods on a match object: `m.group(n)` (positional, 0 = whole match) and
    /// `m.group("name")` (named capture). Both return a `str` (empty if the
    /// group did not participate).
    pub(super) fn lower_match_method(
        &mut self,
        recv: Value,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "group" => {
                if args.len() != 1 {
                    return Err(err("group() takes exactly one argument"));
                }
                let (arg, aty) = self.expr(&args[0].value)?;
                let d = self.new_value(Ty::Ptr);
                if aty == Ty::Ptr {
                    // Named group: `m.group("year")`.
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_regex_group_name".to_string(),
                        args: vec![recv, arg],
                    });
                } else {
                    // Positional group: `m.group(0)`.
                    let idx = self.coerce(arg, aty, Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_regex_group".to_string(),
                        args: vec![recv, idx],
                    });
                }
                Ok((d, Ty::Ptr))
            }
            other => Err(err(format!(
                "a match object has no method '{other}' (supported: group)"
            ))),
        }
    }
}
