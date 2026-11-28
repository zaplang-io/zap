//! Numeric-array lowering (ticket #24, epic #499).
//!
//! The dense N-D numeric array feature: construction (list-literal fill,
//! `zeros`/`arange`/`linspace`, dtype/scalar coercion), element access and the
//! full-index flat-offset fast path (#1120), fancy/boolean/advanced indexing and
//! assignment, `.shape`/`.ndim`/`.T` attributes, the reduction/search/`unique`
//! methods (#507), element-wise and complex binops with SIMD-friendly ZIR nodes,
//! comparisons, logical ops, `reshape`/`join`/`matmul`. Moved verbatim from
//! `lower.rs` (#1124, restored by #1583); `use super::*` re-exposes the parent
//! scope (shared helpers such as `array_slice_bound` and the general
//! index/slice lowerings stay in `lower.rs`).

use super::*;

impl<'a> FnLower<'a> {
    /// `array[dtype]([...])` — allocate an array from a list literal and fill it
    /// element-by-element in row-major order. A flat literal builds a 1-D array;
    /// a **nested rectangular** literal builds an N-D array whose static rank is
    /// the nesting depth and whose shape is the per-level length (`[[1, 2],
    /// [3, 4]]` -> shape `(2, 2)`, #500). Ragged (non-rectangular) literals are
    /// rejected. Non-literal arguments (`array[f64](xs)`) and `zeros`/`arange`
    /// constructors remain deferred (#504).
    pub(super) fn lower_array_new(
        &mut self,
        dtype_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let Expr::Name(dname, _) = dtype_expr else {
            return Err(err("array[...] expects a dtype name, e.g. array[f64]"));
        };
        let dtype = dtype_from_name(dname)?;
        if args.len() != 1 || args[0].name.is_some() {
            return Err(err("array[T](...) takes one positional list literal"));
        }
        let Expr::List(elems, _) = &args[0].value else {
            return Err(err(
                "array[T](...) takes a list literal, e.g. array[f64]([1.0, 2.0, 3.0]) \
                 or a nested one array[f64]([[1.0, 2.0], [3.0, 4.0]]) \
                 (non-literal construction is not supported yet)",
            ));
        };

        // The shape is read by descending into the first child while it is a
        // list; `flatten_array_literal` then validates every other row against
        // that shape and collects the scalar leaves in row-major order.
        let mut shape: Vec<i64> = vec![elems.len() as i64];
        {
            let mut cur = elems;
            while let Some(Expr::List(inner, _)) = cur.first() {
                shape.push(inner.len() as i64);
                cur = inner;
            }
        }
        if shape.len() > u8::MAX as usize {
            return Err(err("array rank (nesting depth) is too large"));
        }
        let ndim = shape.len() as u8;
        let mut flat: Vec<&Expr> = Vec::new();
        for c in elems {
            flatten_array_literal(c, &shape[1..], &mut flat)?;
        }
        let ty = Ty::Array(dtype, ndim);

        let code = self.iconst(Ty::I64, dtype.code());
        let arr = self.new_value(ty);
        if ndim == 1 {
            // 1-D: the historical single-dim construction (unchanged IR shape).
            let n = self.iconst(Ty::I64, shape[0]);
            self.emit(Inst::Call {
                dest: Some(arr),
                func: "zap_array_new".to_string(),
                args: vec![code, n],
            });
        } else {
            // N-D: allocate the rank-`ndim` header + total-element buffer, then
            // write each axis length before filling the flat elements.
            let total: i64 = shape.iter().product();
            let ndim_v = self.iconst(Ty::I64, ndim as i64);
            let total_v = self.iconst(Ty::I64, total);
            self.emit(Inst::Call {
                dest: Some(arr),
                func: "zap_array_new_nd".to_string(),
                args: vec![code, ndim_v, total_v],
            });
            for (axis, dim) in shape.iter().enumerate() {
                let ax = self.iconst(Ty::I64, axis as i64);
                let dv = self.iconst(Ty::I64, *dim);
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_array_set_dim".to_string(),
                    args: vec![arr, ax, dv],
                });
            }
        }
        let setter = format!("zap_array_set_{}", dtype.suffix());
        for (i, e) in flat.iter().enumerate() {
            let (v, vt) = self.expr(e)?;
            // Numeric scalars fill any array; a `complex` element additionally
            // fills a `complex` array (#938). Real elements of a complex array
            // promote to `complex` via `coerce` (imag 0), matching NumPy's
            // `np.array([1, 2], dtype=complex)`.
            let elem_ok = vt.int_width().is_some()
                || vt.is_float()
                || (matches!(dtype, Dtype::Complex) && matches!(vt, Ty::Complex));
            if !elem_ok {
                return Err(err(
                    "array construction requires numeric scalars in a (possibly nested) list literal",
                ));
            }
            let v = self.coerce(v, vt, dtype.ty());
            let idx = self.iconst(Ty::I64, i as i64);
            self.emit(Inst::Call {
                dest: None,
                func: setter.clone(),
                args: vec![arr, idx, v],
            });
        }
        Ok((arr, ty))
    }

    /// The dtype named by a constructor subscript like `zeros[f64]`.
    fn ctor_dtype(&self, index: &Expr) -> Result<Dtype, LowerError> {
        let Expr::Name(dname, _) = index else {
            return Err(err(
                "an array constructor expects a dtype name, e.g. zeros[f64]((3, 4))",
            ));
        };
        dtype_from_name(dname)
    }

    /// Evaluate a constructor shape argument into one `i64` dim value per axis
    /// (#504). A tuple literal `(d0, d1, ...)` is N-D (rank = its arity); a single
    /// expression is 1-D. The rank is therefore static (as D-0164 requires); the
    /// dim *values* may be runtime. An empty shape `()` is rejected.
    fn array_shape_dims(&mut self, shape: &Expr) -> Result<Vec<Value>, LowerError> {
        let parts: Vec<&Expr> = match shape {
            Expr::Tuple(elems, _) => elems.iter().collect(),
            other => vec![other],
        };
        if parts.is_empty() {
            return Err(err("an array shape needs at least one dimension"));
        }
        if parts.len() > u8::MAX as usize {
            return Err(err("array rank (shape length) is too large"));
        }
        let mut dims = Vec::with_capacity(parts.len());
        for p in parts {
            let (v, t) = self.expr(p)?;
            if t.int_width().is_none() {
                return Err(err("array shape dimensions must be integers"));
            }
            dims.push(self.coerce(v, t, Ty::I64));
        }
        Ok(dims)
    }

    /// Allocate a rank-`dims.len()` array of `dtype` with the given (runtime) dim
    /// values: compute the element count as their product, allocate via
    /// `zap_array_new_nd`, then write each axis with `zap_array_set_dim`.
    pub(super) fn array_alloc_shaped(&mut self, dtype: Dtype, dims: &[Value]) -> Value {
        let ndim = dims.len() as u8;
        let code = self.iconst(Ty::I64, dtype.code());
        let mut total = self.iconst(Ty::I64, 1);
        for &d in dims {
            let next = self.new_value(Ty::I64);
            self.emit(Inst::Bin {
                dest: next,
                op: BinOp::Mul,
                lhs: total,
                rhs: d,
            });
            total = next;
        }
        let ndim_v = self.iconst(Ty::I64, ndim as i64);
        let arr = self.new_value(Ty::Array(dtype, ndim));
        self.emit(Inst::Call {
            dest: Some(arr),
            func: "zap_array_new_nd".to_string(),
            args: vec![code, ndim_v, total],
        });
        for (axis, &d) in dims.iter().enumerate() {
            let ax = self.iconst(Ty::I64, axis as i64);
            self.emit(Inst::Call {
                dest: None,
                func: "zap_array_set_dim".to_string(),
                args: vec![arr, ax, d],
            });
        }
        arr
    }

    /// `zeros[T](shape)` / `ones[T](shape)` / `full[T](shape, value)` (#504): a
    /// shaped array filled with 0 / 1 / a given scalar. The fill value crosses as
    /// an `i64` word (integer/bool dtypes) or an `f64` word (float dtypes).
    pub(super) fn lower_array_fill_ctor(
        &mut self,
        index: &Expr,
        args: &[ast::Arg],
        fill: ArrayFill,
    ) -> Result<(Value, Ty), LowerError> {
        let dtype = self.ctor_dtype(index)?;
        let want = if matches!(fill, ArrayFill::Full) {
            2
        } else {
            1
        };
        if args.len() != want || args.iter().any(|a| a.name.is_some()) {
            return Err(err(if matches!(fill, ArrayFill::Full) {
                "full[T](shape, value) takes a shape and a fill value"
            } else {
                "zeros[T](shape) / ones[T](shape) take one shape argument"
            }));
        }
        let dims = self.array_shape_dims(&args[0].value)?;
        let ndim = dims.len() as u8;
        let arr = self.array_alloc_shaped(dtype, &dims);
        if dtype.is_float() {
            let fv = match fill {
                ArrayFill::Zeros => self.fconst(Ty::F64, 0.0),
                ArrayFill::Ones => self.fconst(Ty::F64, 1.0),
                ArrayFill::Full => {
                    let (v, vt) = self.expr(&args[1].value)?;
                    if vt.int_width().is_none() && !vt.is_float() {
                        return Err(err("full value must be a numeric scalar"));
                    }
                    self.coerce(v, vt, Ty::F64)
                }
            };
            self.emit(Inst::Call {
                dest: None,
                func: "zap_array_fill_f".to_string(),
                args: vec![arr, fv],
            });
        } else {
            let iv = match fill {
                ArrayFill::Zeros => self.iconst(Ty::I64, 0),
                ArrayFill::Ones => self.iconst(Ty::I64, 1),
                ArrayFill::Full => {
                    let (v, vt) = self.expr(&args[1].value)?;
                    if vt.int_width().is_none() && !vt.is_float() {
                        return Err(err("full value must be a numeric scalar"));
                    }
                    self.coerce(v, vt, Ty::I64)
                }
            };
            self.emit(Inst::Call {
                dest: None,
                func: "zap_array_fill_i".to_string(),
                args: vec![arr, iv],
            });
        }
        Ok((arr, Ty::Array(dtype, ndim)))
    }

    /// Evaluate a constructor scalar argument, coercing to `to` (`Ty::I64`/`F64`).
    fn ctor_scalar(&mut self, arg: &ast::Arg, to: Ty) -> Result<Value, LowerError> {
        let (v, t) = self.expr(&arg.value)?;
        if t.int_width().is_none() && !t.is_float() {
            return Err(err("array constructor arguments must be numeric scalars"));
        }
        Ok(self.coerce(v, t, to))
    }

    /// `arange[T](start, stop[, step])` (#504): a rank-1 array `start, start+step,
    /// ...` up to (excluding) `stop`; the length is a runtime value. Float dtypes
    /// use the `_f` runtime entry point, integer dtypes `_i`.
    pub(super) fn lower_array_arange(
        &mut self,
        index: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let dtype = self.ctor_dtype(index)?;
        if !(2..=3).contains(&args.len()) || args.iter().any(|a| a.name.is_some()) {
            return Err(err(
                "arange[T](start, stop[, step]) takes two or three positional arguments",
            ));
        }
        let ty = Ty::Array(dtype, 1);
        let arr = self.new_value(ty);
        let code = self.iconst(Ty::I64, dtype.code());
        let word = if dtype.is_float() { Ty::F64 } else { Ty::I64 };
        let start = self.ctor_scalar(&args[0], word)?;
        let stop = self.ctor_scalar(&args[1], word)?;
        let step = if args.len() == 3 {
            self.ctor_scalar(&args[2], word)?
        } else if dtype.is_float() {
            self.fconst(Ty::F64, 1.0)
        } else {
            self.iconst(Ty::I64, 1)
        };
        let func = if dtype.is_float() {
            "zap_array_arange_f"
        } else {
            "zap_array_arange_i"
        };
        // arange raises a catchable ValueError on a zero step, so it must be a
        // throwing call (invoke) for an enclosing `try` to catch it.
        self.throwing_call(Some(arr), func, vec![code, start, stop, step]);
        Ok((arr, ty))
    }

    /// `linspace[T](start, stop, num)` (#504): a rank-1 array of `num` evenly
    /// spaced samples from `start` to `stop` inclusive of both ends.
    pub(super) fn lower_array_linspace(
        &mut self,
        index: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let dtype = self.ctor_dtype(index)?;
        if args.len() != 3 || args.iter().any(|a| a.name.is_some()) {
            return Err(err(
                "linspace[T](start, stop, num) takes three positional arguments",
            ));
        }
        let ty = Ty::Array(dtype, 1);
        let arr = self.new_value(ty);
        let code = self.iconst(Ty::I64, dtype.code());
        let start = self.ctor_scalar(&args[0], Ty::F64)?;
        let stop = self.ctor_scalar(&args[1], Ty::F64)?;
        let num = self.ctor_scalar(&args[2], Ty::I64)?;
        self.emit(Inst::Call {
            dest: Some(arr),
            func: "zap_array_linspace".to_string(),
            args: vec![code, start, stop, num],
        });
        Ok((arr, ty))
    }

    /// Evaluate the axis indices of an array subscript, for an assignment target.
    /// `m[i, j]` arrives as an `Expr::Tuple`; `a[i]` as a single expression. Each
    /// index must be an integer; a sliced assignment target is not supported (a
    /// sliced read goes through `lower_array_tuple`). Returns one `i64` index
    /// value per axis supplied.
    fn array_axis_indices(&mut self, index: &Expr) -> Result<Vec<Value>, LowerError> {
        let parts: Vec<&Expr> = match index {
            Expr::Tuple(elems, _) => elems.iter().collect(),
            other => vec![other],
        };
        let mut out = Vec::with_capacity(parts.len());
        for p in parts {
            if matches!(p, Expr::Slice { .. } | Expr::SliceSpec { .. }) {
                return Err(err(
                    "assigning to a sliced array index (a strided-view store) is not \
                     supported yet (#502); index all axes with integers",
                ));
            }
            let (iv, it) = self.expr(p)?;
            if it.int_width().is_none() {
                return Err(err("array indices must be integers"));
            }
            out.push(self.coerce_index(iv, it));
        }
        if out.is_empty() {
            return Err(err("an array index requires at least one integer index"));
        }
        Ok(out)
    }

    /// Fold a sequence of leading axis indices into successive **row views**
    /// (`zap_array_row`), each reducing the rank by one (#500). Returns the final
    /// (lower-rank) array value; an empty `idxs` returns `arr` unchanged.
    fn array_fold_rows(&mut self, arr: Value, d: Dtype, ndim: u8, idxs: &[Value]) -> Value {
        let mut cur = arr;
        let mut rank = ndim;
        for &i in idxs {
            rank -= 1;
            let v = self.new_value(Ty::Array(d, rank));
            // A partial index `m[i]` is a rank-reduced view via
            // `zap_array_index_axis`, which raises a catchable `IndexError` when
            // `i` is out of range (#1566) — carry the unwind edge.
            self.throwing_call(Some(v), "zap_array_row", vec![cur, i]);
            cur = v;
        }
        cur
    }

    /// Validate one axis index of a full integer index against that axis's
    /// length (`zap_array_ckidx`, #894) and return the validated value, so the
    /// flat-offset Horner can thread it in. Preserves the per-axis out-of-range
    /// abort the former `zap_array_row` fold performed, without allocating a
    /// view: e.g. `m[0, 5]` on a `(3, 3)` array still raises rather than
    /// silently reading flat slot 5.
    fn array_ckidx(&mut self, arr: Value, axis: usize, i: Value) -> Value {
        let ax = self.iconst(Ty::I64, axis as i64);
        let v = self.new_value(Ty::I64);
        // An out-of-range axis index raises a catchable `IndexError` (#1566), so
        // it carries an unwind edge to a `try` in the calling frame.
        self.throwing_call(Some(v), "zap_array_ckidx", vec![arr, ax, i]);
        v
    }

    /// Compute the whole-shape row-major flat element index of a FULL integer
    /// index `m[i0, i1, ..., i_{n-1}]` inline (#894), bounds-checking each axis
    /// (`array_ckidx`) and folding the dims with Horner's rule
    /// (`flat = (..(i0 * d1 + i1) * d2 + i2) ..`, the trailing-axis dims read via
    /// `zap_array_dim`). No `zap_array_row` view is allocated; the single
    /// `zap_array_get/set_<dtype>` that consumes the result addresses storage
    /// through `zap_array_phys_nd`, so this composes with strided views.
    /// `idxs.len()` must equal `ndim` (>= 2 — rank 1 is already a direct get).
    fn array_full_flat_index(&mut self, arr: Value, ndim: u8, idxs: &[Value]) -> Value {
        let n = ndim as usize;
        debug_assert_eq!(idxs.len(), n);
        let mut flat = self.array_ckidx(arr, 0, idxs[0]);
        for (k, &idx) in idxs.iter().enumerate().skip(1) {
            let axis = self.iconst(Ty::I64, k as i64);
            let dk = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(dk),
                func: "zap_array_dim".to_string(),
                args: vec![arr, axis],
            });
            let ik = self.array_ckidx(arr, k, idx);
            let scaled = self.new_value(Ty::I64);
            self.emit(Inst::Bin {
                dest: scaled,
                op: BinOp::Mul,
                lhs: flat,
                rhs: dk,
            });
            let next = self.new_value(Ty::I64);
            self.emit(Inst::Bin {
                dest: next,
                op: BinOp::Add,
                lhs: scaled,
                rhs: ik,
            });
            flat = next;
        }
        flat
    }

    /// Is `e` a pure, side-effect-free scalar index safe to lower
    /// speculatively while probing a chain (#894 part 2)? Conservative: a bare
    /// name or an integer literal — the hot `m[i][j]` loop-index / constant
    /// forms. Because such an expression has no side effect and cannot raise,
    /// abandoning a non-matching chain leaves only dead, re-executable IR, so
    /// the fall-through per-step lowering stays correct. Anything else
    /// (arithmetic, a call, a nested index that could raise) is not fused.
    fn is_pure_scalar_index(e: &Expr) -> bool {
        matches!(e, Expr::Name(..) | Expr::Int(..))
    }

    /// #894 part 2: fuse a chained FULL integer index `root[i0][i1]…[i_{n-1}]`
    /// into a single flat scalar get — identical to the comma form
    /// `root[i0, …, i_{n-1}]` (Increment 19). Each `root[i0]…[i_k]` prefix would
    /// otherwise materialize a rank-reduced **row view** (one `zap_array_row`
    /// allocation per link) only to index it again; when every link is a pure
    /// scalar index those intermediates are dead, single-use temporaries that
    /// provably cannot escape or alias, so the whole chain lowers to the inline
    /// flat offset with no allocation.
    ///
    /// Returns `None` (fall through to the per-step lowering, unchanged
    /// behavior) unless: every link is a pure scalar index, the root is a bare
    /// name that lowers to an `array[T]`, and the chain indexes ALL `ndim` axes
    /// (a partial chain still yields a bindable view per step; a slice / tuple /
    /// fancy-array index is not a plain integer chain). The pure-index guard also
    /// keeps the `None` paths sound: any IR emitted before bailing is
    /// side-effect-free and the caller re-lowers the chain per step.
    pub(super) fn try_fuse_full_index_chain(
        &mut self,
        base: &Expr,
        index: &Expr,
    ) -> Result<Option<(Value, Ty)>, LowerError> {
        if !Self::is_pure_scalar_index(index) {
            return Ok(None);
        }
        // Collect the chain, outer index first, walking inward through nested
        // `Expr::Index` bases until the first non-index root.
        let mut rev_idx: Vec<&Expr> = vec![index];
        let mut root = base;
        while let Expr::Index {
            base: inner,
            index: idx,
            ..
        } = root
        {
            if !Self::is_pure_scalar_index(idx) {
                return Ok(None);
            }
            rev_idx.push(idx);
            root = inner;
        }
        // A lone `m[i]` is not a chain; and the root must be a bare name so that
        // re-lowering it on the fall-through path is a harmless repeated load.
        if rev_idx.len() < 2 || !matches!(root, Expr::Name(..)) {
            return Ok(None);
        }
        let (rv, rty) = self.expr(root)?;
        let (d, ndim) = match rty {
            Ty::Array(d, n) => (d, n),
            _ => return Ok(None),
        };
        // Only a FULL chain (one index per axis) fuses to a scalar; a partial
        // chain still produces a bindable rank-reduced view per step.
        if rev_idx.len() != ndim as usize {
            return Ok(None);
        }
        // Source order is root-most first, so reverse the outer-first list and
        // evaluate each index exactly once in that order (matching the per-step
        // order: the base `m[i]` — hence `i` — before the outer index `j`).
        let mut idxs: Vec<Value> = Vec::with_capacity(rev_idx.len());
        for ie in rev_idx.iter().rev() {
            let (iv, it) = self.expr(ie)?;
            if it.int_width().is_none() {
                // An array-typed index is advanced indexing (#510), not a plain
                // integer chain — do not fuse.
                return Ok(None);
            }
            idxs.push(self.coerce_index(iv, it));
        }
        let flat = self.array_full_flat_index(rv, ndim, &idxs);
        let dest = self.new_value(d.ty());
        // Out-of-range flat index raises a catchable `IndexError` (#1566).
        self.throwing_call(
            Some(dest),
            &format!("zap_array_get_{}", d.suffix()),
            vec![rv, flat],
        );
        Ok(Some((dest, d.ty())))
    }

    /// Lower an N-D array subscript **get** `m[i, j, ...]`. A full index (one per
    /// axis) names a scalar element; a partial index (fewer than `ndim`) yields a
    /// rank-reduced **view** (#500). Too many indices is an error.
    pub(super) fn lower_array_index_get(
        &mut self,
        arr: Value,
        d: Dtype,
        ndim: u8,
        index: &Expr,
    ) -> Result<(Value, Ty), LowerError> {
        // A multi-axis index that contains any slice (`m[:, j]`, `m[lo:hi, k]`,
        // the 1-D `a[i]` never reaches here) folds per axis into a strided view
        // (#502); the all-integer paths below stay the plain contiguous
        // row-fold + scalar get.
        // Advanced indexing (#510, D-0175): a single array operand is a boolean
        // mask (`a[mask]`) or an integer index array (`a[idx]`, fancy gather).
        // Detect by lowering the lone subscript once; a scalar integer index or
        // a view is handled inline here (reusing the already-evaluated value so
        // the index runs exactly once).
        if !matches!(
            index,
            Expr::Tuple(..) | Expr::Slice { .. } | Expr::SliceSpec { .. }
        ) {
            let (iv, it) = self.expr(index)?;
            if let Ty::Array(idt, ind) = it {
                return self.lower_array_single_advanced(arr, d, ndim, iv, idt, ind);
            }
            if it.int_width().is_none() {
                return Err(err("array indices must be integers"));
            }
            let i = self.coerce_index(iv, it);
            if ndim == 1 {
                let dest = self.new_value(d.ty());
                // 1-D `a[i]`: out-of-range raises a catchable `IndexError` (#1566).
                self.throwing_call(
                    Some(dest),
                    &format!("zap_array_get_{}", d.suffix()),
                    vec![arr, i],
                );
                return Ok((dest, d.ty()));
            }
            let view = self.array_fold_rows(arr, d, ndim, &[i]);
            return Ok((view, Ty::Array(d, ndim - 1)));
        }

        let parts: Vec<&Expr> = match index {
            Expr::Tuple(elems, _) => elems.iter().collect(),
            other => vec![other],
        };
        self.lower_array_tuple(arr, d, ndim, &parts)
    }

    /// Lower a tuple subscript `a[p0, p1, ...]` (and the single-slice case),
    /// classifying every part with exactly one evaluation. If any part is an
    /// array it is advanced indexing (#510) routed to `lower_array_advanced`;
    /// otherwise it is the pre-#510 behavior — a strided view when any slice is
    /// present (#502), else a scalar get (full integer index) or a rank-reduced
    /// view (partial integer index, #500).
    fn lower_array_tuple(
        &mut self,
        arr: Value,
        d: Dtype,
        ndim: u8,
        parts: &[&Expr],
    ) -> Result<(Value, Ty), LowerError> {
        let n = ndim as usize;
        if parts.len() > n {
            return Err(err(format!(
                "too many indices for a {n}-dimensional array ({} given)",
                parts.len()
            )));
        }
        let mut cls: Vec<IdxPart> = Vec::with_capacity(parts.len());
        let mut has_adv = false;
        let mut has_slice = false;
        for p in parts {
            match p {
                Expr::SliceSpec { lo, hi, step, .. } => {
                    let lo = self.array_slice_bound(lo.as_deref())?;
                    let hi = self.array_slice_bound(hi.as_deref())?;
                    let st = self.array_slice_bound(step.as_deref())?;
                    has_slice = true;
                    cls.push(IdxPart::Slice { lo, hi, step: st });
                }
                Expr::Slice { lo, hi, .. } => {
                    let lo = self.array_slice_bound(lo.as_deref())?;
                    let hi = self.array_slice_bound(hi.as_deref())?;
                    let st = (self.iconst(Ty::I64, 0), self.iconst(Ty::I64, 0));
                    has_slice = true;
                    cls.push(IdxPart::Slice { lo, hi, step: st });
                }
                other => {
                    let (v, t) = self.expr(other)?;
                    if let Ty::Array(idt, ind) = t {
                        has_adv = true;
                        cls.push(IdxPart::Adv(v, idt, ind));
                    } else if t.int_width().is_some() {
                        cls.push(IdxPart::Int(v, t));
                    } else {
                        return Err(err("array indices must be integers or index arrays"));
                    }
                }
            }
        }
        if has_adv {
            return self.lower_array_advanced(arr, d, ndim, cls);
        }
        // No advanced parts: reproduce the pre-#510 slice / integer behavior.
        if has_slice {
            let mut cur = arr;
            let mut rank = ndim;
            let mut cur_axis: i64 = 0;
            for part in cls {
                match part {
                    IdxPart::Slice { lo, hi, step } => {
                        let axis = self.iconst(Ty::I64, cur_axis);
                        let v = self.new_value(Ty::Array(d, rank));
                        // A zero step raises a catchable `ValueError` (#1566).
                        self.throwing_call(
                            Some(v),
                            "zap_array_slice_axis",
                            vec![cur, axis, lo.0, hi.0, step.0, lo.1, hi.1, step.1],
                        );
                        cur = v;
                        cur_axis += 1;
                    }
                    IdxPart::Int(v, t) => {
                        let i = self.coerce_index(v, t);
                        let axis = self.iconst(Ty::I64, cur_axis);
                        rank -= 1;
                        let out = Ty::Array(d, rank);
                        let nv = self.new_value(out);
                        // Out-of-range axis index raises a catchable `IndexError` (#1566).
                        self.throwing_call(Some(nv), "zap_array_index_axis", vec![cur, axis, i]);
                        cur = nv;
                    }
                    IdxPart::Adv(..) => unreachable!("has_adv is false here"),
                }
            }
            return Ok((cur, Ty::Array(d, rank)));
        }
        // All-integer index: fold to a rank-1 view then a scalar get (full
        // index), or a rank-reduced view (partial index).
        let idxs: Vec<Value> = cls
            .into_iter()
            .map(|p| match p {
                IdxPart::Int(v, t) => self.coerce_index(v, t),
                _ => unreachable!(),
            })
            .collect();
        let k = idxs.len();
        if k == n {
            // Full integer index -> a single scalar get. Rank 1 is already a
            // direct get on the array; rank >= 2 computes the flat offset inline
            // (#894) instead of folding through an allocated rank-1 row view.
            let flat = if n == 1 {
                idxs[0]
            } else {
                self.array_full_flat_index(arr, ndim, &idxs)
            };
            let dest = self.new_value(d.ty());
            // Out-of-range flat index raises a catchable `IndexError` (#1566).
            self.throwing_call(
                Some(dest),
                &format!("zap_array_get_{}", d.suffix()),
                vec![arr, flat],
            );
            Ok((dest, d.ty()))
        } else {
            let view = self.array_fold_rows(arr, d, ndim, &idxs);
            Ok((view, Ty::Array(d, ndim - k as u8)))
        }
    }

    /// Lower general advanced indexing `a[p0, p1, ...]` (#510, D-0175) with at
    /// least one array part. Builds a per-axis spec buffer (7 i64 per axis:
    /// tag + slice/int/group params) and the ordered list of integer index
    /// arrays, then calls `zap_array_fancy`. A boolean array of rank k is
    /// expanded into k integer index arrays via `nonzero_axis`, consuming k
    /// axes. The result rank = (broadcast rank of the advanced arrays) + (number
    /// of kept FULL/SLICE axes). The kernel raises IndexError/ValueError, so it
    /// is a `throwing_call`.
    fn lower_array_advanced(
        &mut self,
        arr: Value,
        d: Dtype,
        ndim: u8,
        cls: Vec<IdxPart>,
    ) -> Result<(Value, Ty), LowerError> {
        let n = ndim as usize;
        let zero = self.iconst(Ty::I64, 0);
        let mut spec: Vec<[Value; 7]> = (0..n).map(|_| [zero; 7]).collect();
        let adv_list = self.new_value(Ty::List(Elem::Ptr));
        self.emit(Inst::Call {
            dest: Some(adv_list),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let mut axis = 0usize;
        let mut g: i64 = 0;
        let mut b: u8 = 0;
        let mut slices: u8 = 0;
        for part in cls {
            if axis >= n {
                return Err(err("too many indices for the array"));
            }
            match part {
                IdxPart::Slice { lo, hi, step } => {
                    let tag = self.iconst(Ty::I64, 1);
                    spec[axis] = [tag, lo.0, hi.0, step.0, lo.1, hi.1, step.1];
                    slices += 1;
                    axis += 1;
                }
                IdxPart::Int(v, t) => {
                    let iv = self.coerce(v, t, Ty::I64);
                    let tag = self.iconst(Ty::I64, 2);
                    spec[axis] = [tag, iv, zero, zero, zero, zero, zero];
                    axis += 1;
                }
                IdxPart::Adv(v, idt, ind) => {
                    if matches!(idt, Dtype::Bool) {
                        let k = ind as usize;
                        if axis + k > n {
                            return Err(err("boolean index spans more axes than the array has"));
                        }
                        for t in 0..k {
                            let axc = self.iconst(Ty::I64, t as i64);
                            let nz = self.new_value(Ty::Array(Dtype::I64, 1));
                            self.emit(Inst::Call {
                                dest: Some(nz),
                                func: "zap_array_nonzero_axis".to_string(),
                                args: vec![v, axc],
                            });
                            self.emit(Inst::Call {
                                dest: None,
                                func: "zap_list_push_ptr".to_string(),
                                args: vec![adv_list, nz],
                            });
                            let tag = self.iconst(Ty::I64, 3);
                            let gc = self.iconst(Ty::I64, g);
                            spec[axis] = [tag, gc, zero, zero, zero, zero, zero];
                            axis += 1;
                            g += 1;
                        }
                        if b < 1 {
                            b = 1;
                        }
                    } else {
                        self.emit(Inst::Call {
                            dest: None,
                            func: "zap_list_push_ptr".to_string(),
                            args: vec![adv_list, v],
                        });
                        let tag = self.iconst(Ty::I64, 3);
                        let gc = self.iconst(Ty::I64, g);
                        spec[axis] = [tag, gc, zero, zero, zero, zero, zero];
                        if ind > b {
                            b = ind;
                        }
                        axis += 1;
                        g += 1;
                    }
                }
            }
        }
        // Kept axes (preserved in the result) = trailing FULL axes + the SLICE
        // axes placed above. Integer and advanced axes are consumed.
        let kept: u8 = (n - axis) as u8 + slices;
        let rank = b + kept;
        let total_slots = (n * 7) as i64;
        let nd_slots = self.iconst(Ty::I64, total_slots);
        let buf = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(buf),
            func: "zap_i64buf".to_string(),
            args: vec![nd_slots],
        });
        for (ax, rec) in spec.iter().enumerate() {
            for (s, &val) in rec.iter().enumerate() {
                let slot = self.iconst(Ty::I64, (ax * 7 + s) as i64);
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_i64set".to_string(),
                    args: vec![buf, slot, val],
                });
            }
        }
        let ndim_v = self.iconst(Ty::I64, ndim as i64);
        let out = Ty::Array(d, rank);
        let dest = self.new_value(out);
        self.throwing_call(
            Some(dest),
            "zap_array_fancy",
            vec![arr, buf, ndim_v, adv_list],
        );
        Ok((dest, out))
    }

    /// Lower a single advanced subscript `a[ix]` where `ix` is an array (#510,
    /// D-0175): a boolean mask (`array[bool]`) selects via `zap_array_mask_select`
    /// (result rank `1 + ndim - mask.ndim`, a 1-D copy when the mask is
    /// full-shape); an integer index array gathers via `zap_array_take` (result
    /// rank `idx.ndim + ndim - 1`). Both produce a fresh copy and may raise a
    /// catchable `IndexError`, so the kernels are `throwing_call`s.
    fn lower_array_single_advanced(
        &mut self,
        arr: Value,
        d: Dtype,
        ndim: u8,
        ix: Value,
        ix_dtype: Dtype,
        ix_ndim: u8,
    ) -> Result<(Value, Ty), LowerError> {
        if matches!(ix_dtype, Dtype::Bool) {
            if ix_ndim > ndim {
                return Err(err("boolean index has more dimensions than the array"));
            }
            let rank = 1 + ndim - ix_ndim;
            let out = Ty::Array(d, rank);
            let dest = self.new_value(out);
            self.throwing_call(Some(dest), "zap_array_mask_select", vec![arr, ix]);
            Ok((dest, out))
        } else {
            // Integer index array -> fancy gather along axis 0.
            let rank = ix_ndim + ndim - 1;
            let out = Ty::Array(d, rank);
            let dest = self.new_value(out);
            self.throwing_call(Some(dest), "zap_array_take", vec![arr, ix]);
            Ok((dest, out))
        }
    }

    /// Lower `where(...)` (#510, D-0175). Three args: element-wise select from
    /// `x` where `cond` is true else `y`, with full NumPy broadcasting (result
    /// rank = max of the operand ranks; a scalar `x`/`y` is wrapped as a 1-element
    /// array so one runtime path serves scalars and arrays). The result dtype is
    /// the NEP-50 promotion of `x` and `y` (weak scalars). One arg: a tuple of
    /// `cond.ndim` index arrays (`cond.nonzero()`). The select kernel raises a
    /// catchable ValueError on incompatible shapes, so it is a `throwing_call`.
    pub(super) fn lower_array_where(
        &mut self,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        if args.iter().any(|a| a.name.is_some()) {
            return Err(err("where() takes positional arguments only"));
        }
        if args.len() == 1 {
            let (cv, ct) = self.expr(&args[0].value)?;
            let Ty::Array(_, cnd) = ct else {
                return Err(err("where(cond) requires an array condition"));
            };
            let mut vals = Vec::with_capacity(cnd as usize);
            let mut tys = Vec::with_capacity(cnd as usize);
            for k in 0..cnd as i64 {
                let axis = self.iconst(Ty::I64, k);
                let out = Ty::Array(Dtype::I64, 1);
                let d = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_array_nonzero_axis".to_string(),
                    args: vec![cv, axis],
                });
                vals.push(d);
                tys.push(out);
            }
            let sid = self.ctx.tuples.borrow_mut().intern(tys);
            let dest = self.new_value(Ty::Struct(sid));
            self.emit(Inst::StructNew { dest, sid });
            for (idx, v) in vals.into_iter().enumerate() {
                self.emit(Inst::FieldSet {
                    base: dest,
                    sid,
                    idx: idx as u32,
                    val: v,
                });
            }
            return Ok((dest, Ty::Struct(sid)));
        }
        if args.len() != 3 {
            return Err(err("where() takes 1 argument (nonzero) or 3 (cond, x, y)"));
        }
        let (cv, ct) = self.expr(&args[0].value)?;
        let Ty::Array(_, cnd) = ct else {
            return Err(err("where(cond, x, y): cond must be an array"));
        };
        let (xv, xt) = self.expr(&args[1].value)?;
        let (yv, yt) = self.expr(&args[2].value)?;
        // Operand -> (wrapped array value, its dtype-or-None-if-scalar, rank).
        let wrap =
            |s: &mut Self, v: Value, t: Ty| -> Result<(Value, Option<Dtype>, u8), LowerError> {
                match t {
                    Ty::Array(dd, nd) => Ok((v, Some(dd), nd)),
                    _ if t.is_float() => {
                        let f = s.coerce(v, t, Ty::F64);
                        let a = s.new_value(Ty::Array(Dtype::F64, 1));
                        s.emit(Inst::Call {
                            dest: Some(a),
                            func: "zap_array_from_scalar_f".to_string(),
                            args: vec![f],
                        });
                        Ok((a, None, 1))
                    }
                    _ if t.int_width().is_some() => {
                        let i = s.coerce(v, t, Ty::I64);
                        let a = s.new_value(Ty::Array(Dtype::I64, 1));
                        s.emit(Inst::Call {
                            dest: Some(a),
                            func: "zap_array_from_scalar_i".to_string(),
                            args: vec![i],
                        });
                        Ok((a, None, 1))
                    }
                    _ => Err(err(
                        "where(cond, x, y): x and y must be arrays or numeric scalars",
                    )),
                }
            };
        let x_is_float = xt.is_float();
        let y_is_float = yt.is_float();
        let (xa, xd, xnd) = wrap(self, xv, xt)?;
        let (ya, yd, ynd) = wrap(self, yv, yt)?;
        // NEP-50 result dtype: array⊗array promotes both dtypes; a scalar is weak.
        let out = match (xd, yd) {
            (Some(a), Some(b)) => Dtype::promote(a, b),
            (Some(a), None) => Dtype::promote_scalar(a, y_is_float),
            (None, Some(b)) => Dtype::promote_scalar(b, x_is_float),
            (None, None) => {
                if xt.is_float() || yt.is_float() {
                    Dtype::F64
                } else {
                    Dtype::I64
                }
            }
        };
        let rank = cnd.max(xnd).max(ynd);
        let out_ty = Ty::Array(out, rank);
        let code = self.iconst(Ty::I64, out.code());
        let dest = self.new_value(out_ty);
        self.throwing_call(Some(dest), "zap_array_where", vec![cv, xa, ya, code]);
        Ok((dest, out_ty))
    }

    /// Lower an advanced-index assignment `a[ix] = value` (#510, D-0175) where
    /// `ix` is an array. A boolean mask fills (scalar) or scatters (array of
    /// values) the selected elements; an integer index array scatters by index.
    /// The write goes into `a`'s real storage (so the runtime maps each slot
    /// through `zap_array_phys_nd`), and may raise a catchable IndexError/
    /// ValueError, so the kernels are `throwing_call`s. `value` is lowered once.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_array_advanced_assign(
        &mut self,
        target: Value,
        _d: Dtype,
        ndim: u8,
        ix: Value,
        ix_dtype: Dtype,
        ix_ndim: u8,
        value: &Expr,
    ) -> Result<(), LowerError> {
        let (v, vt) = self.expr(value)?;
        let is_mask = matches!(ix_dtype, Dtype::Bool);
        if is_mask && ix_ndim > ndim {
            return Err(err("boolean index has more dimensions than the array"));
        }
        let (scalar_fn, array_fn) = if is_mask {
            (
                "zap_array_mask_assign_scalar",
                "zap_array_mask_assign_array",
            )
        } else {
            ("zap_array_scatter_scalar", "zap_array_scatter_array")
        };
        match vt {
            Ty::Array(..) => {
                self.throwing_call(None, array_fn, vec![target, ix, v]);
                Ok(())
            }
            _ if vt.is_float() || vt.int_width().is_some() => {
                let (si, sf, isf) = self.array_scalar_words(v, vt);
                self.throwing_call(None, scalar_fn, vec![target, ix, si, sf, isf]);
                Ok(())
            }
            _ => Err(err(
                "advanced-index assignment requires a numeric scalar or an array of values",
            )),
        }
    }

    /// Resolve an N-D array subscript to the `(array, flat element index)` naming
    /// a single scalar slot, for `m[i, j] = v` / augmented assignment — read or
    /// written by the caller through `zap_array_get/set_<dtype>` (which address
    /// storage via `zap_array_phys_nd`). The index is evaluated exactly once.
    /// Only a full index names a slot; assigning a partial index (a whole-row
    /// store) is a documented follow-up (#501/#511). Rank 1 is the element
    /// index directly; rank >= 2 computes the flat offset inline (#894) instead
    /// of folding through an allocated rank-1 row view.
    pub(super) fn array_scalar_slot(
        &mut self,
        arr: Value,
        ndim: u8,
        index: &Expr,
    ) -> Result<(Value, Value), LowerError> {
        let idxs = self.array_axis_indices(index)?;
        let k = idxs.len();
        let n = ndim as usize;
        if k > n {
            return Err(err(format!(
                "too many indices for a {n}-dimensional array ({k} given)"
            )));
        }
        if k < n {
            return Err(err(format!(
                "assigning to a partial index of a {n}-dimensional array (a whole-row store) \
                 is not supported yet; index all {n} axes"
            )));
        }
        let flat = if n == 1 {
            idxs[0]
        } else {
            self.array_full_flat_index(arr, ndim, &idxs)
        };
        Ok((arr, flat))
    }

    /// Lower an array attribute read (#500): `a.ndim` (the static rank, a
    /// compile-time constant), `a.size` (total element count across all axes),
    /// and `a.shape` (a fixed-arity `tuple` of the per-axis lengths, one `int64`
    /// per axis read from the runtime `dims`). `len(a)` still gives the first
    /// axis; other attribute names are rejected with a pointer to these.
    pub(super) fn lower_array_attr(
        &mut self,
        arr: Value,
        d: Dtype,
        ndim: u8,
        name: &str,
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "ndim" => Ok((self.iconst(Ty::I64, ndim as i64), Ty::I64)),
            // `a.T` (#511): the full-axis-reverse transpose, a view (same as
            // `a.transpose()`).
            "T" => {
                let out = Ty::Array(d, ndim);
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_transpose".to_string(),
                    args: vec![arr],
                });
                Ok((dest, out))
            }
            "size" => {
                let dest = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_count".to_string(),
                    args: vec![arr],
                });
                Ok((dest, Ty::I64))
            }
            "shape" => {
                // A statically rank-`ndim` array has a fixed-arity shape tuple;
                // build it as a `tuple[int64; ndim]` from per-axis `zap_array_dim`
                // reads. (Rank 1 still yields a one-element tuple, matching NumPy.)
                let tys = vec![Ty::I64; ndim as usize];
                let sid = self.ctx.tuples.borrow_mut().intern(tys);
                let dest = self.new_value(Ty::Struct(sid));
                self.emit(Inst::StructNew { dest, sid });
                for axis in 0..ndim {
                    let ax = self.iconst(Ty::I64, axis as i64);
                    let dim = self.new_value(Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(dim),
                        func: "zap_array_dim".to_string(),
                        args: vec![arr, ax],
                    });
                    self.emit(Inst::FieldSet {
                        base: dest,
                        sid,
                        idx: axis as u32,
                        val: dim,
                    });
                }
                Ok((dest, Ty::Struct(sid)))
            }
            other => Err(err(format!(
                "array attribute '{other}' is unsupported; arrays expose '.shape', \
                 '.ndim', '.size', '.T' (transpose), and len(a) for the first axis"
            ))),
        }
    }

    /// Array reduction methods (#507, D-0170): `a.sum()`, `a.min()`, `a.max()`,
    /// `a.mean()`, `a.prod()`, each with an optional `axis=` keyword.
    ///
    /// Without `axis`, the whole array reduces to a scalar: `sum`/`min`/`max`/
    /// `prod` keep the dtype, `mean` yields a float (`f32` source → `f32`, else
    /// `f64`). With `axis=k`, one axis collapses: on a rank-1 array that is the
    /// full (scalar) reduction, and on a rank-`n` (n ≥ 2) array it produces a
    /// new rank-`(n-1)` array (`zap_array_reduce_axis` / `zap_array_mean_axis`).
    /// `min`/`max` raise a catchable `ValueError` on an empty array/axis, and an
    /// out-of-range `axis` raises too, so those forms lower through
    /// `throwing_call`; `sum`/`prod`/`mean` have defined empty results
    /// (`0`/`1`/`nan`) and never raise.
    pub(super) fn lower_array_method(
        &mut self,
        recv: Value,
        d: Dtype,
        ndim: u8,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        // Shape ops (#511): reshape / transpose / flatten are not reductions.
        match name {
            "reshape" => return self.lower_array_reshape(recv, d, ndim, args),
            "transpose" => {
                if !args.is_empty() {
                    return Err(err(
                        "transpose() takes no arguments yet (a permutation argument is a follow-up)",
                    ));
                }
                // Reverse all axes; a view (metadata only), rank preserved.
                let out = Ty::Array(d, ndim);
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_transpose".to_string(),
                    args: vec![recv],
                });
                return Ok((dest, out));
            }
            "flatten" => {
                if !args.is_empty() {
                    return Err(err("flatten() takes no arguments"));
                }
                let out = Ty::Array(d, 1); // always rank-1 copy
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_flatten".to_string(),
                    args: vec![recv],
                });
                return Ok((dest, out));
            }
            // Complex support (#938): `a.conj()` -> complex array (conjugate; a
            // real receiver lifts to complex with imag 0), `a.abs()` -> f64 array
            // of magnitudes (|z| for complex, |x| for a real dtype).
            "conj" => {
                if !args.is_empty() {
                    return Err(err("conj() takes no arguments"));
                }
                let out = Ty::Array(Dtype::Complex, ndim);
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_conj".to_string(),
                    args: vec![recv],
                });
                return Ok((dest, out));
            }
            "abs" => {
                if !args.is_empty() {
                    return Err(err("abs() takes no arguments"));
                }
                let out = Ty::Array(Dtype::F64, ndim);
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_cabs".to_string(),
                    args: vec![recv],
                });
                return Ok((dest, out));
            }
            // Sort (#932): `a.sort(axis=, kind=)` sorts IN PLACE (returns nothing);
            // `a.argsort(axis=, kind=)` returns a new i64 index array.
            "sort" => return self.lower_array_sortish(recv, d, ndim, false, true, args),
            "argsort" => return self.lower_array_sortish(recv, d, ndim, true, false, args),
            // Searching / selection (#934): methods. `a.partition()` sorts in
            // place; the rest return a new array/scalar.
            "argmin" => return self.lower_array_search(recv, d, ndim, "argmin", false, args),
            "argmax" => return self.lower_array_search(recv, d, ndim, "argmax", false, args),
            "searchsorted" => {
                return self.lower_array_search(recv, d, ndim, "searchsorted", false, args)
            }
            "partition" => return self.lower_array_search(recv, d, ndim, "partition", true, args),
            "argpartition" => {
                return self.lower_array_search(recv, d, ndim, "argpartition", false, args)
            }
            _ => {}
        }
        // op code shared with the runtime axis kernel: 0=sum,1=min,2=max,3=prod.
        let op_code = match name {
            "sum" => 0i64,
            "min" => 1,
            "max" => 2,
            "prod" => 3,
            "mean" => 4, // mean has its own entry points (float result)
            other => {
                return Err(err(format!(
                    "'{other}' is not an array method; the array methods are \
                     reductions sum/min/max/mean/prod (optional axis=), shape \
                     ops reshape/transpose/flatten, and conj/abs"
                )))
            }
        };
        // Complex reductions have no kernel (#1721, sibling of #1719): `min`/`max`/
        // `prod`/`mean` on a complex array emit undefined `zap_array_{…}_complex`
        // (a link error), and the axis forms route to `zap_array_reduce_axis` /
        // `zap_array_mean_axis`, whose dtype switch has no `DTYPE_COMPLEX` case
        // (`default: break`) → an unfilled result buffer (silently wrong). Reject
        // at lowering: `min`/`max` have no ordering (permanent, as `sort` rejects);
        // `prod`/`mean` are meaningful but their kernels are deferred to #938. Full
        // `sum` is supported (`zap_array_sum_complex`); its axis form is guarded in
        // the axis-reduction branch below.
        if matches!(d, Dtype::Complex) {
            match op_code {
                1 | 2 => {
                    return Err(err(format!(
                        "{name}: complex arrays have no ordering \
                         (operate on real/imag parts)"
                    )))
                }
                3 | 4 => {
                    return Err(err(format!(
                        "{name}: not yet supported for complex arrays \
                         (tracked under zaplang-io/zap#938)"
                    )))
                }
                _ => {}
            }
        }
        let is_mean = op_code == 4;
        let raises = matches!(op_code, 1 | 2); // min/max raise on empty
                                               // The float result dtype for `mean` (f32 source stays f32).
        let mean_dt = if d == Dtype::F32 {
            Dtype::F32
        } else {
            Dtype::F64
        };

        // Parse the single optional `axis=` keyword; no positional args.
        let mut axis: Option<&Expr> = None;
        for a in args {
            match &a.name {
                Some(k) if k == "axis" => {
                    if axis.is_some() {
                        return Err(err(format!("{name}() got axis= more than once")));
                    }
                    axis = Some(&a.value);
                }
                Some(k) => {
                    return Err(err(format!(
                        "{name}() got an unexpected keyword '{k}' (only axis= is supported)"
                    )))
                }
                None => {
                    return Err(err(format!(
                        "array {name}() takes no positional arguments; pass axis= as a keyword"
                    )))
                }
            }
        }

        // No axis, or axis on a rank-1 array -> a full reduction to a scalar.
        if axis.is_none() || ndim == 1 {
            // Evaluate (and discard) an axis expression on a 1-D array so a
            // runtime out-of-range axis still faults; its only valid value is 0
            // or -1, but we keep the full-reduction result either way.
            if let Some(ax) = axis {
                let (av, at) = self.expr(ax)?;
                let av = self.coerce(av, at, Ty::I64);
                self.throwing_call(None, "zap_array_axis_check", vec![recv, av]);
            }
            if is_mean {
                let raw = self.new_value(Ty::F64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: format!("zap_array_mean_{}", d.suffix()),
                    args: vec![recv],
                });
                let res = self.coerce(raw, Ty::F64, mean_dt.ty());
                return Ok((res, mean_dt.ty()));
            }
            let dest = self.new_value(d.ty());
            let func = format!("zap_array_{name}_{}", d.suffix());
            if raises {
                self.throwing_call(Some(dest), &func, vec![recv]);
            } else {
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func,
                    args: vec![recv],
                });
            }
            return Ok((dest, d.ty()));
        }

        // Axis reduction on a rank-(n>=2) array -> a rank-(n-1) array.
        // Only `sum` reaches here for complex (min/max/prod/mean were rejected
        // above); `zap_array_reduce_axis` has no `DTYPE_COMPLEX` case, so an axis
        // sum over a complex array would leave the result buffer unfilled (#1721).
        if matches!(d, Dtype::Complex) {
            return Err(err(
                "sum(axis=): axis reductions are not yet supported for complex \
                 arrays (tracked under zaplang-io/zap#938)",
            ));
        }
        let ax = axis.unwrap();
        let (av, at) = self.expr(ax)?;
        let av = self.coerce(av, at, Ty::I64);
        let out_rank = ndim - 1;
        if is_mean {
            let out_ty = Ty::Array(mean_dt, out_rank);
            let dest = self.new_value(out_ty);
            // Raises ValueError on an out-of-range axis.
            self.throwing_call(Some(dest), "zap_array_mean_axis", vec![recv, av]);
            return Ok((dest, out_ty));
        }
        let out_ty = Ty::Array(d, out_rank);
        let dest = self.new_value(out_ty);
        let opv = self.iconst(Ty::I64, op_code);
        // Raises ValueError on an out-of-range axis (min/max also on an empty axis).
        self.throwing_call(Some(dest), "zap_array_reduce_axis", vec![recv, av, opv]);
        Ok((dest, out_ty))
    }

    /// `sort` / `argsort` over a numeric array (#932, D-0190), as a method
    /// (`a.sort()` in place, `a.argsort()` -> indices) or a builtin (`sort(a)`
    /// copy, `argsort(a)`). `kw` holds the keyword arguments (`axis=`, `kind=`);
    /// the receiver array is already lowered. `in_place` is the sorting method
    /// form; `is_argsort` selects argsort. Unstable (introsort) is the default;
    /// `kind="stable"`/`"mergesort"` selects the stable floor. `axis=` is an int
    /// (default last axis) or `None` (flatten); the runtime raises ValueError on
    /// a bad axis. Complex has no ordering and is rejected.
    pub(super) fn lower_array_sortish(
        &mut self,
        recv: Value,
        d: Dtype,
        ndim: u8,
        is_argsort: bool,
        in_place: bool,
        kw: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        if matches!(d, Dtype::Complex) {
            return Err(err(
                "sort/argsort: complex arrays have no ordering (sort the real/imag parts)",
            ));
        }
        let mut stable = false;
        let mut axis_expr: Option<&Expr> = None;
        for a in kw {
            match a.name.as_deref() {
                Some("kind") => match &a.value {
                    Expr::Str {
                        value,
                        fstring: false,
                        ..
                    } => match value.as_str() {
                        "stable" | "mergesort" => stable = true,
                        "quicksort" | "heapsort" => stable = false,
                        _ => {
                            return Err(err(
                                "sort kind= must be \"quicksort\", \"stable\", \"mergesort\", or \"heapsort\"",
                            ))
                        }
                    },
                    _ => return Err(err("sort kind= must be a string literal")),
                },
                Some("axis") => axis_expr = Some(&a.value),
                Some(k) => {
                    return Err(err(format!(
                        "sort/argsort got an unexpected keyword '{k}' (only axis= and kind=)"
                    )))
                }
                None => {
                    return Err(err(
                        "array sort/argsort takes no extra positional arguments (pass axis=/kind=)",
                    ))
                }
            }
        }
        // axis: None -> flatten; an int -> that axis; absent -> last axis (-1).
        let (flatten, axis_val) = match axis_expr {
            Some(Expr::None(_)) => (true, self.iconst(Ty::I64, 0)),
            Some(e) => {
                let (v, t) = self.expr(e)?;
                (false, self.coerce(v, t, Ty::I64))
            }
            None => (false, self.iconst(Ty::I64, -1)),
        };
        let stable_v = self.iconst(Ty::I64, i64::from(stable));
        if in_place {
            if flatten {
                return Err(err(
                    "a.sort(axis=None) is not supported; use sort(a, axis=None)",
                ));
            }
            self.throwing_call(
                None,
                "zap_array_sort_inplace",
                vec![recv, axis_val, stable_v],
            );
            return Ok((self.iconst(Ty::I32, 0), Ty::I32));
        }
        let flatten_v = self.iconst(Ty::I64, i64::from(flatten));
        let out_dtype = if is_argsort { Dtype::I64 } else { d };
        let out_rank = if flatten { 1 } else { ndim };
        let out = Ty::Array(out_dtype, out_rank);
        let dest = self.new_value(out);
        let func = if is_argsort {
            "zap_array_argsort"
        } else {
            "zap_array_sorted"
        };
        self.throwing_call(Some(dest), func, vec![recv, axis_val, flatten_v, stable_v]);
        Ok((dest, out))
    }

    /// Build a fresh 1-element `array[d]` holding the already-lowered scalar `v`
    /// (used for a scalar `searchsorted` `v` / an int `partition` `kth`).
    fn build_scalar_array(&mut self, d: Dtype, v: Value, t: Ty) -> Value {
        let one = self.iconst(Ty::I64, 1);
        let arr = self.array_alloc_shaped(d, &[one]);
        let zero = self.iconst(Ty::I64, 0);
        let is_float = matches!(d, Dtype::F64 | Dtype::F32);
        let sv = self.coerce(v, t, if is_float { Ty::F64 } else { Ty::I64 });
        self.emit(Inst::Call {
            dest: None,
            func: format!("zap_array_set_{}", d.suffix()),
            args: vec![arr, zero, sv],
        });
        arr
    }

    /// A keyword argument's expression, if present.
    fn kw_arg<'b>(&self, args: &'b [ast::Arg], name: &str) -> Option<&'b Expr> {
        args.iter()
            .find(|a| a.name.as_deref() == Some(name))
            .map(|a| &a.value)
    }

    /// `argmin`/`argmax`/`searchsorted`/`partition`/`argpartition` over a numeric
    /// array (#934), as a method (receiver `recv`) or a builtin (first positional).
    /// `rest` is the remaining arguments. `in_place` marks the `a.partition()`
    /// method (sorts the receiver in place). Complex has no ordering → rejected.
    pub(super) fn lower_array_search(
        &mut self,
        recv: Value,
        d: Dtype,
        ndim: u8,
        which: &str,
        in_place: bool,
        rest: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        if matches!(d, Dtype::Complex) {
            return Err(err(format!(
                "{which}: complex arrays have no ordering (operate on real/imag parts)"
            )));
        }
        let positional: Vec<&Expr> = rest
            .iter()
            .filter(|a| a.name.is_none())
            .map(|a| &a.value)
            .collect();
        match which {
            "argmin" | "argmax" => {
                let is_max = i64::from(which == "argmax");
                let im = self.iconst(Ty::I64, is_max);
                // An axis on a rank-(>=2) array -> a rank-(n-1) index array; no
                // axis, or a rank-1 array -> a flattened scalar index.
                match self.kw_arg(rest, "axis") {
                    Some(ax) if ndim > 1 => {
                        let (v, t) = self.expr(ax)?;
                        let av = self.coerce(v, t, Ty::I64);
                        let out = Ty::Array(Dtype::I64, ndim - 1);
                        let dest = self.new_value(out);
                        self.throwing_call(
                            Some(dest),
                            "zap_array_argminmax_axis",
                            vec![recv, av, im],
                        );
                        Ok((dest, out))
                    }
                    axis => {
                        // A rank-1 axis is validated but still yields a scalar.
                        if let Some(ax) = axis {
                            let (v, t) = self.expr(ax)?;
                            let av = self.coerce(v, t, Ty::I64);
                            self.throwing_call(None, "zap_array_axis_check", vec![recv, av]);
                        }
                        let dest = self.new_value(Ty::I64);
                        self.throwing_call(Some(dest), "zap_array_argminmax", vec![recv, im]);
                        Ok((dest, Ty::I64))
                    }
                }
            }
            "searchsorted" => {
                if positional.len() != 1 {
                    return Err(err("searchsorted expects (v, side=...)"));
                }
                let side = match self.kw_arg(rest, "side") {
                    Some(Expr::Str {
                        value,
                        fstring: false,
                        ..
                    }) => match value.as_str() {
                        "left" => 0,
                        "right" => 1,
                        _ => return Err(err("searchsorted side= must be \"left\" or \"right\"")),
                    },
                    Some(_) => return Err(err("searchsorted side= must be a string literal")),
                    None => 0,
                };
                let sidev = self.iconst(Ty::I64, side);
                let (vv, vt) = self.expr(positional[0])?;
                if let Ty::Array(_, vnd) = vt {
                    let out = Ty::Array(Dtype::I64, vnd);
                    let dest = self.new_value(out);
                    self.emit(Inst::Call {
                        dest: Some(dest),
                        func: "zap_array_searchsorted".to_string(),
                        args: vec![recv, vv, sidev],
                    });
                    Ok((dest, out))
                } else {
                    // scalar v -> a 1-element array[d]; the result's element 0.
                    let varr = self.build_scalar_array(d, vv, vt);
                    let arr = self.new_value(Ty::Array(Dtype::I64, 1));
                    self.emit(Inst::Call {
                        dest: Some(arr),
                        func: "zap_array_searchsorted".to_string(),
                        args: vec![recv, varr, sidev],
                    });
                    let zero = self.iconst(Ty::I64, 0);
                    let dest = self.new_value(Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(dest),
                        func: "zap_array_get_i64".to_string(),
                        args: vec![arr, zero],
                    });
                    Ok((dest, Ty::I64))
                }
            }
            "partition" | "argpartition" => {
                if positional.len() != 1 {
                    return Err(err(format!("{which} expects (kth, axis=...)")));
                }
                let (kv, kt) = self.expr(positional[0])?;
                // kth: an int (-> 1-element array[i64]) or an array[i64] sequence.
                let karr = if matches!(kt, Ty::Array(..)) {
                    kv
                } else {
                    self.build_scalar_array(Dtype::I64, kv, kt)
                };
                let axisv = match self.kw_arg(rest, "axis") {
                    Some(e) => {
                        let (v, t) = self.expr(e)?;
                        self.coerce(v, t, Ty::I64)
                    }
                    None => self.iconst(Ty::I64, -1),
                };
                if which == "argpartition" {
                    let out = Ty::Array(Dtype::I64, ndim);
                    let dest = self.new_value(out);
                    self.throwing_call(
                        Some(dest),
                        "zap_array_argpartition",
                        vec![recv, karr, axisv],
                    );
                    Ok((dest, out))
                } else if in_place {
                    self.throwing_call(
                        None,
                        "zap_array_partition_inplace",
                        vec![recv, karr, axisv],
                    );
                    Ok((self.iconst(Ty::I32, 0), Ty::I32))
                } else {
                    let out = Ty::Array(d, ndim);
                    let dest = self.new_value(out);
                    self.throwing_call(
                        Some(dest),
                        "zap_array_partitioned",
                        vec![recv, karr, axisv],
                    );
                    Ok((dest, out))
                }
            }
            _ => Err(err(format!(
                "'{which}' is not an array search/select method"
            ))),
        }
    }

    /// `unique(a, return_index=, return_inverse=, return_counts=)` (#934): the
    /// sorted unique values, or a tuple (values[, index][, inverse][, counts]) in
    /// NumPy's order when any `return_*` is set. Each output recomputes the sort.
    pub(super) fn lower_unique(&mut self, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
        let positional: Vec<&Expr> = args
            .iter()
            .filter(|a| a.name.is_none())
            .map(|a| &a.value)
            .collect();
        if positional.len() != 1 {
            return Err(err(
                "unique() takes the array as its single positional argument",
            ));
        }
        let (recv, rt) = self.expr(positional[0])?;
        let Ty::Array(d, _) = rt else {
            return Err(err("unique() requires a numeric array"));
        };
        if matches!(d, Dtype::Complex) {
            return Err(err("unique: complex arrays have no ordering"));
        }
        let flag = |this: &Self, name: &str| -> Result<bool, LowerError> {
            match this.kw_arg(args, name) {
                Some(Expr::Bool(value, _)) => Ok(*value),
                Some(_) => Err(err(format!("unique {name}= must be True or False"))),
                None => Ok(false),
            }
        };
        let ri = flag(self, "return_index")?;
        let rv = flag(self, "return_inverse")?;
        let rc = flag(self, "return_counts")?;
        let values_ty = Ty::Array(d, 1);
        let values = self.new_value(values_ty);
        self.emit(Inst::Call {
            dest: Some(values),
            func: "zap_array_unique_values".to_string(),
            args: vec![recv],
        });
        if !ri && !rv && !rc {
            return Ok((values, values_ty));
        }
        // Build the tuple (values, [index], [inverse], [counts]) in NumPy's order.
        let mut elems = vec![values_ty];
        let mut vals = vec![values];
        let mut add = |this: &mut Self, func: &str| {
            let v = this.new_value(Ty::Array(Dtype::I64, 1));
            this.emit(Inst::Call {
                dest: Some(v),
                func: func.to_string(),
                args: vec![recv],
            });
            elems.push(Ty::Array(Dtype::I64, 1));
            vals.push(v);
        };
        if ri {
            add(self, "zap_array_unique_index");
        }
        if rv {
            add(self, "zap_array_unique_inverse");
        }
        if rc {
            add(self, "zap_array_unique_counts");
        }
        let sid = self.ctx.tuples.borrow_mut().intern(elems);
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        for (idx, v) in vals.into_iter().enumerate() {
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: idx as u32,
                val: v,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    /// Element-wise `+`/`-`/`*`/`/` on arrays, including scalar broadcast and
    /// mixed-dtype promotion (tickets #49, #506). Emits the [`Inst::ArrayBinOp`]
    /// node codegen vectorizes. Two arrays of different dtypes promote to the
    /// higher dtype ([`Dtype::promote`], so `i64 + f64 -> f64`); a scalar is
    /// weak ([`Dtype::promote_scalar`], so `uint8 + 1` stays `uint8`).
    ///
    /// `scalar op array` (#506): the commutative `+`/`*` are reordered to
    /// `array op scalar` (kernel computes `array[i] op scalar`); the
    /// non-commutative `-`/`/` are kept as scalar-on-left (`scalar_lhs`), so the
    /// kernel computes `scalar op array[i]`.
    ///
    /// `/` is NumPy **true division** (#506): the result dtype is always a float
    /// — an all-integer division lifts to `f64` (`int_array / 2 -> f64`), while a
    /// float operand keeps its width (`f32_array / 2 -> f32`). A zero divisor
    /// yields IEEE ±inf/nan, not a raise (D-0169). Floor division `//` is not yet
    /// supported on arrays.
    pub(super) fn array_binop(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let irop = match op {
            Add => BinOp::Add,
            Sub => BinOp::Sub,
            Mul => BinOp::Mul,
            Div => BinOp::Div,
            _ => {
                return Err(err(
                    "only +, -, *, and / are supported on arrays yet (//, reductions beyond sum, etc. are deferred)",
                ))
            }
        };
        // A `complex` operand (a complex array, or a complex scalar against an
        // array) routes to the complex runtime helpers (#938): a complex element
        // is a 16-byte 2×f64 slot, not a scalar SIMD lane, so it never flows
        // through `Inst::ArrayBinOp`.
        let is_cx = |t: Ty| matches!(t, Ty::Complex) || matches!(t, Ty::Array(Dtype::Complex, _));
        if is_cx(lt) || is_cx(rt) {
            return self.complex_array_binop(op, l, lt, r, rt);
        }
        let is_div = matches!(op, Div);
        // NumPy true division always yields a float: an integer elementwise
        // result dtype lifts to `f64` (#506). Float dtypes are unchanged.
        let lift = |d: Dtype| {
            if is_div && !d.is_float() {
                Dtype::F64
            } else {
                d
            }
        };
        let is_scalar = |t: Ty| t.is_float() || t.int_width().is_some();
        match (lt, rt) {
            (Ty::Array(ld, lnd), Ty::Array(rd, rnd)) => {
                // Broadcasting (#501, D-0165): shapes are right-aligned and each
                // aligned axis must be equal, or one side 1 (stretched), or
                // missing (the lower-rank operand gets implicit leading 1s). The
                // result rank is the larger rank (static); the per-axis
                // compatibility and result dims are a runtime concern. A throwing
                // check raises a catchable ValueError on incompatible shapes
                // before the kernel runs; codegen then picks the flat or strided
                // kernel by a runtime same-shape test.
                let rank = lnd.max(rnd);
                self.throwing_call(None, "zap_array_broadcast_check", vec![l, r]);
                let out = lift(Dtype::promote(ld, rd));
                let dest = self.new_value(Ty::Array(out, rank));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    out,
                    lhs: l,
                    lhs_dtype: ld,
                    rhs: r,
                    rhs_dtype: rd,
                    scalar: false,
                    scalar_lhs: false,
                });
                Ok((dest, Ty::Array(out, rank)))
            }
            (Ty::Array(ld, lnd), _) if is_scalar(rt) => {
                // Weak scalar (NEP 50): an int scalar keeps the array's dtype,
                // a float scalar forces `f64`. The scalar crosses to codegen as
                // its natural `i64`/`f64` word (`rhs_dtype`); codegen converts
                // it to `out` (possibly a narrowing `i64 -> u8`).
                let scalar_is_float = rt.is_float();
                let sdt = if scalar_is_float {
                    Dtype::F64
                } else {
                    Dtype::I64
                };
                let s = self.coerce(r, rt, sdt.ty());
                let out = lift(Dtype::promote_scalar(ld, scalar_is_float));
                let dest = self.new_value(Ty::Array(out, lnd));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    out,
                    lhs: l,
                    lhs_dtype: ld,
                    rhs: s,
                    rhs_dtype: sdt,
                    scalar: true,
                    scalar_lhs: false,
                });
                Ok((dest, Ty::Array(out, lnd)))
            }
            (_, Ty::Array(rd, rnd)) if is_scalar(lt) => {
                // The commutative `+`/`*` reorder to `array op scalar` (the
                // kernel computes `array[i] op scalar`); the non-commutative
                // `-`/`/` are kept as scalar-on-left (`scalar_lhs`), so the
                // kernel computes `scalar op array[i]` (#506).
                let scalar_lhs = !matches!(op, Add | Mul);
                let scalar_is_float = lt.is_float();
                let sdt = if scalar_is_float {
                    Dtype::F64
                } else {
                    Dtype::I64
                };
                let s = self.coerce(l, lt, sdt.ty());
                let out = lift(Dtype::promote_scalar(rd, scalar_is_float));
                let dest = self.new_value(Ty::Array(out, rnd));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    out,
                    lhs: r,
                    lhs_dtype: rd,
                    rhs: s,
                    rhs_dtype: sdt,
                    scalar: true,
                    scalar_lhs,
                });
                Ok((dest, Ty::Array(out, rnd)))
            }
            _ => Err(err(
                "unsupported array operand (element-wise ops take two arrays or an array and a numeric scalar)",
            )),
        }
    }

    /// Element-wise `+`/`-`/`*`/`/` where a `complex` operand is involved (#938),
    /// routed to the complex runtime helpers. `array ⊗ array` (either operand real
    /// or complex) → `zap_array_cbinop`; `array ⊗ scalar` → `zap_array_cbinop_cs`
    /// (a real scalar is boxed to a `complex` with a zero imaginary part, so one
    /// helper covers both). The result is always a `complex` array.
    fn complex_array_binop(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let opcode = match op {
            Add => 0,
            Sub => 1,
            Mul => 2,
            Div => 3,
            _ => return Err(err("only +, -, *, and / are supported on complex arrays")),
        };
        let opc = self.iconst(Ty::I64, opcode);
        let is_scalar = |t: Ty| t.is_float() || t.int_width().is_some() || matches!(t, Ty::Complex);
        // Box a scalar operand into a `complex` value (a real scalar → imag 0).
        let to_complex = |s: &mut FnLower, v: Value, t: Ty| -> Value {
            if matches!(t, Ty::Complex) {
                v
            } else {
                let f = s.coerce(v, t, Ty::F64);
                let zc = s.new_value(Ty::Complex);
                s.emit(Inst::Call {
                    dest: Some(zc),
                    func: "zap_complex_from_f64".to_string(),
                    args: vec![f],
                });
                zc
            }
        };
        match (lt, rt) {
            (Ty::Array(_, lnd), Ty::Array(_, rnd)) => {
                let rank = lnd.max(rnd);
                self.throwing_call(None, "zap_array_broadcast_check", vec![l, r]);
                let out = Ty::Array(Dtype::Complex, rank);
                let d = self.new_value(out);
                self.throwing_call(Some(d), "zap_array_cbinop", vec![l, r, opc]);
                Ok((d, out))
            }
            (Ty::Array(_, lnd), _) if is_scalar(rt) => {
                let zc = to_complex(self, r, rt);
                let scalar_lhs = self.iconst(Ty::I64, 0);
                let out = Ty::Array(Dtype::Complex, lnd);
                let d = self.new_value(out);
                self.throwing_call(Some(d), "zap_array_cbinop_cs", vec![l, zc, opc, scalar_lhs]);
                Ok((d, out))
            }
            (_, Ty::Array(_, rnd)) if is_scalar(lt) => {
                // `-`/`/` are non-commutative, so the scalar stays on the left.
                let scalar_lhs = self.iconst(Ty::I64, i64::from(!matches!(op, Add | Mul)));
                let zc = to_complex(self, l, lt);
                let out = Ty::Array(Dtype::Complex, rnd);
                let d = self.new_value(out);
                self.throwing_call(Some(d), "zap_array_cbinop_cs", vec![r, zc, opc, scalar_lhs]);
                Ok((d, out))
            }
            _ => Err(err(
                "unsupported complex array operand (element-wise ops take two arrays or an array and a numeric scalar)",
            )),
        }
    }

    /// Split a numeric scalar operand into the `(si, sf, is_float)` ABI words the
    /// array comparison kernel expects: a float scalar crosses as `sf` (`f64`)
    /// with `is_float = 1`; an int scalar as `si` (`i64`) with `is_float = 0`.
    /// The unused word is a zero constant.
    fn array_scalar_words(&mut self, v: Value, t: Ty) -> (Value, Value, Value) {
        if t.is_float() {
            let sf = self.coerce(v, t, Ty::F64);
            let si = self.iconst(Ty::I64, 0);
            let isf = self.iconst(Ty::I64, 1);
            (si, sf, isf)
        } else {
            let si = self.coerce(v, t, Ty::I64);
            let sf = self.fconst(Ty::F64, 0.0);
            let isf = self.iconst(Ty::I64, 0);
            (si, sf, isf)
        }
    }

    /// Element-wise comparison `a <op> b` / `a <op> scalar` → `array[bool]`
    /// (#510, D-0175), the mask source for boolean-mask indexing. Array⊗array
    /// broadcasts (result rank = max; the kernel validates shapes and raises a
    /// catchable `ValueError`, so it is a `throwing_call`); array⊗scalar keeps
    /// the array's shape. `scalar_lhs` records a scalar on the left so the kernel
    /// preserves operand order (`3 < a`), which matters for `<`/`<=`/`>`/`>=`.
    pub(super) fn lower_array_cmp(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let pred: i64 = match op {
            Eq => 0,
            Ne => 1,
            Lt => 2,
            Le => 3,
            Gt => 4,
            Ge => 5,
            _ => return Err(err("internal: non-comparison op in lower_array_cmp")),
        };
        let pred_v = self.iconst(Ty::I64, pred);
        // Complex arrays have no comparison kernel (#1721, sibling of #1719): a
        // complex element is a 16-byte `(re, im)` pointer-word, but `zap_array_cmp`
        // / `zap_array_cmp_scalar` dispatch via `mm_load_{f,u,i}` with no
        // `DTYPE_COMPLEX` case, so the word is compared as a raw `int64` → silent
        // garbage. Reject at lowering: ordering has no meaning for complex (Zap
        // rejects complex ordering, as `sort`/`argsort` do — a permanent rejection);
        // `==` / `!=` are meaningful but their kernel is deferred to the complex
        // epic #938.
        let is_complex_array = |t: &Ty| matches!(t, Ty::Array(Dtype::Complex, _));
        if is_complex_array(&lt) || is_complex_array(&rt) {
            return Err(err(match op {
                Lt | Le | Gt | Ge => {
                    "array comparison: complex arrays have no ordering \
                     (compare the real/imag parts)"
                }
                _ => {
                    "array `==` / `!=` is not yet supported for complex arrays \
                     (tracked under zaplang-io/zap#938)"
                }
            }));
        }
        let is_scalar = |t: &Ty| t.is_float() || t.int_width().is_some();
        match (lt, rt) {
            (Ty::Array(_, lnd), Ty::Array(_, rnd)) => {
                let rank = lnd.max(rnd);
                let out = Ty::Array(Dtype::Bool, rank);
                let dest = self.new_value(out);
                self.throwing_call(Some(dest), "zap_array_cmp", vec![l, r, pred_v]);
                Ok((dest, out))
            }
            (Ty::Array(_, lnd), _) if is_scalar(&rt) => {
                let (si, sf, isf) = self.array_scalar_words(r, rt);
                let lhs_flag = self.iconst(Ty::I64, 0);
                let out = Ty::Array(Dtype::Bool, lnd);
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_cmp_scalar".to_string(),
                    args: vec![l, si, sf, isf, pred_v, lhs_flag],
                });
                Ok((dest, out))
            }
            (_, Ty::Array(_, rnd)) if is_scalar(&lt) => {
                let (si, sf, isf) = self.array_scalar_words(l, lt);
                let lhs_flag = self.iconst(Ty::I64, 1);
                let out = Ty::Array(Dtype::Bool, rnd);
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_cmp_scalar".to_string(),
                    args: vec![r, si, sf, isf, pred_v, lhs_flag],
                });
                Ok((dest, out))
            }
            _ => Err(err(
                "array comparison takes two arrays or an array and a numeric scalar",
            )),
        }
    }

    /// `mask1 & mask2` / `|` / `^` → `array[bool]` (#510, D-0175): combine two
    /// boolean masks (NumPy's logical `&`/`|`/`^` on bool dtype), with
    /// broadcasting. Only `array[bool]` operands are allowed — integer-array
    /// bitwise is a separate element-wise op, outside #510's indexing charter.
    /// The kernel validates broadcast shapes and raises, so it is a
    /// `throwing_call`.
    pub(super) fn lower_array_logical(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let (Ty::Array(Dtype::Bool, lnd), Ty::Array(Dtype::Bool, rnd)) = (lt, rt) else {
            return Err(err(
                "'&' / '|' / '^' on arrays is only supported between boolean masks \
                 (array[bool]); integer-array bitwise operations are not supported yet",
            ));
        };
        let opc: i64 = match op {
            BitAnd => 0,
            BitOr => 1,
            BitXor => 2,
            _ => return Err(err("internal: non-bitwise op in lower_array_logical")),
        };
        let opv = self.iconst(Ty::I64, opc);
        let rank = lnd.max(rnd);
        let out = Ty::Array(Dtype::Bool, rank);
        let dest = self.new_value(out);
        self.throwing_call(Some(dest), "zap_array_logical", vec![l, r, opv]);
        Ok((dest, out))
    }

    /// Matrix multiply / dot product `a @ b` (and `dot(a, b)`) on numeric arrays
    /// (#509, D-0172, PEP 465). Both operands must be rank-1 or rank-2 arrays;
    /// the result dtype is the NEP-50 promotion of the operand dtypes.
    /// - 1-D · 1-D → a scalar dot product (`zap_array_dot{i,f}`);
    /// - 1-D · 2-D / 2-D · 1-D → a rank-1 array (the vector promoted to a row /
    ///   column and the singleton axis dropped);
    /// - 2-D · 2-D → a rank-2 matrix product.
    ///
    /// A shape mismatch raises a catchable `ValueError` from the runtime, so the
    /// calls are `throwing_call`s. Batched (≥ 3-D) matmul is a follow-up.
    /// Complex operands are rejected at lowering (#1719): the promoted dtype is
    /// `Complex`, which has no matmul kernel yet (#938), and the integer kernel
    /// would reinterpret the complex pointer-word as an `int64` → silent garbage.
    /// `a.reshape(d0, d1, ...)` (#511, D-0174): a view over the compacted buffer
    /// with the given dims, one of which may be the literal `-1` (inferred at
    /// runtime). The result's static rank is the number of dim arguments. A
    /// total-size mismatch raises a catchable `ValueError`, so the reshape call
    /// is a `throwing_call`.
    fn lower_array_reshape(
        &mut self,
        recv: Value,
        d: Dtype,
        _ndim: u8,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        if args.is_empty() {
            return Err(err("reshape() needs at least one dimension"));
        }
        if args.len() > u8::MAX as usize {
            return Err(err("reshape() rank is too large"));
        }
        let mut dims = Vec::with_capacity(args.len());
        for a in args {
            if a.name.is_some() {
                return Err(err("reshape() takes positional integer dimensions"));
            }
            let (v, t) = self.expr(&a.value)?;
            if t.int_width().is_none() {
                return Err(err("reshape() dimensions must be integers"));
            }
            dims.push(self.coerce(v, t, Ty::I64));
        }
        let rank = dims.len() as u8;
        // Build the i64 dims buffer the runtime reads.
        let nd_v = self.iconst(Ty::I64, rank as i64);
        let buf = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(buf),
            func: "zap_i64buf".to_string(),
            args: vec![nd_v],
        });
        for (k, &dv) in dims.iter().enumerate() {
            let kv = self.iconst(Ty::I64, k as i64);
            self.emit(Inst::Call {
                dest: None,
                func: "zap_i64set".to_string(),
                args: vec![buf, kv, dv],
            });
        }
        let out = Ty::Array(d, rank);
        let dest = self.new_value(out);
        // Raises ValueError on a size mismatch / bad -1.
        self.throwing_call(Some(dest), "zap_array_reshape", vec![recv, nd_v, buf]);
        Ok((dest, out))
    }

    /// `concatenate([a, …], axis=k)` / `stack([a, …], axis=k)` (#511, D-0174):
    /// join a `list` of arrays. The list is passed straight to the runtime (it
    /// is already GC-traced). `concatenate` keeps the input rank; `stack` adds a
    /// new axis (rank + 1). The result's static rank/dtype come from the list's
    /// element type `array[T]`. Raises `ValueError` on a shape/dtype/axis fault.
    pub(super) fn lower_array_join(
        &mut self,
        fname: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let mut list_arg: Option<&Expr> = None;
        let mut axis_arg: Option<&Expr> = None;
        for a in args {
            match &a.name {
                None if list_arg.is_none() => list_arg = Some(&a.value),
                Some(k) if k == "axis" => axis_arg = Some(&a.value),
                _ => {
                    return Err(err(format!(
                        "{fname}() takes a list of arrays and an optional axis="
                    )))
                }
            }
        }
        let Some(list_expr) = list_arg else {
            return Err(err(format!("{fname}() needs a list of arrays")));
        };
        // A `list[array[T]]` erases its element dtype/rank to a pointer in the
        // collection ABI, so the result rank/dtype can't come from the list
        // type. Require a list literal and read them from the elements
        // directly, lowering each exactly once into a fresh pointer list.
        let Expr::List(elems, _) = list_expr else {
            return Err(err(format!(
                "{fname}() needs a list literal of arrays, e.g. {fname}([a, b])"
            )));
        };
        if elems.is_empty() {
            return Err(err(format!("{fname}() needs at least one array")));
        }
        let listv = self.new_value(Ty::List(Elem::Ptr));
        self.emit(Inst::Call {
            dest: Some(listv),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let mut dr: Option<(Dtype, u8)> = None;
        for e in elems {
            let (v, t) = self.expr(e)?;
            let Ty::Array(d, ndim) = t else {
                return Err(err(format!("{fname}() elements must be arrays")));
            };
            if dr.is_none() {
                dr = Some((d, ndim));
            }
            self.emit(Inst::Call {
                dest: None,
                func: "zap_list_push_ptr".to_string(),
                args: vec![listv, v],
            });
        }
        let (d, ndim) = dr.unwrap();
        // concatenate keeps the rank; stack adds a new axis.
        let out_rank = if fname == "stack" { ndim + 1 } else { ndim };
        let axis = match axis_arg {
            Some(e) => {
                let (v, t) = self.expr(e)?;
                if t.int_width().is_none() {
                    return Err(err(format!("{fname}() axis must be an integer")));
                }
                self.coerce(v, t, Ty::I64)
            }
            None => self.iconst(Ty::I64, 0),
        };
        let func = if fname == "stack" {
            "zap_array_stack"
        } else {
            "zap_array_concat"
        };
        let out = Ty::Array(d, out_rank);
        let dest = self.new_value(out);
        self.throwing_call(Some(dest), func, vec![listv, axis]);
        Ok((dest, out))
    }

    pub(super) fn lower_array_matmul(
        &mut self,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        let (Ty::Array(ld, lnd), Ty::Array(rd, rnd)) = (lt, rt) else {
            return Err(err(
                "'@' (matrix multiply) requires two arrays; a scalar does not define '@'",
            ));
        };
        if lnd > 2 || rnd > 2 {
            return Err(err(
                "'@' (matrix multiply) supports only 1-D and 2-D arrays (batched matmul is a follow-up)",
            ));
        }
        let out = Dtype::promote(ld, rd);
        // Complex `@` / `dot` has no kernel yet (#938): the integer matmul path
        // (`zap_array_doti` / `zap_array_matmul`'s `zap_matmul_i`) reads/writes a
        // complex array's 16-byte `(re, im)` pointer-word as a raw `int64`, silently
        // corrupting the result. Reject it cleanly here — exactly as sort/argsort/
        // unique/ordering reject complex — until the real complex kernel
        // (`zap_matmul_c` / `zap_array_dotc`) lands under #938. `promote` makes
        // either-operand-complex collapse to `Complex`, so this single check also
        // covers complex@real and real@complex, for every rank pairing and for both
        // the `a @ b` and `dot(a, b)` spellings (zaplang-io/zap#1719).
        if matches!(out, Dtype::Complex) {
            return Err(err(
                "'@'/dot (matrix multiply) is not yet supported for complex arrays \
                 (tracked under zaplang-io/zap#938)",
            ));
        }
        if lnd == 1 && rnd == 1 {
            // Dot product -> scalar. Accumulate in the wide word, then narrow to
            // the promoted dtype (like `sum`).
            let is_float = out.is_float();
            let (func, raw_ty) = if is_float {
                ("zap_array_dotf", Ty::F64)
            } else {
                ("zap_array_doti", Ty::I64)
            };
            let raw = self.new_value(raw_ty);
            self.throwing_call(Some(raw), func, vec![l, r]);
            let res = self.coerce(raw, raw_ty, out.ty());
            return Ok((res, out.ty()));
        }
        // An array result: rank 2 only when both operands are 2-D, else rank 1.
        let rank = if lnd == 2 && rnd == 2 { 2u8 } else { 1u8 };
        let out_ty = Ty::Array(out, rank);
        let dest = self.new_value(out_ty);
        let code = self.iconst(Ty::I64, out.code());
        self.throwing_call(Some(dest), "zap_array_matmul", vec![l, r, code]);
        Ok((dest, out_ty))
    }
}
