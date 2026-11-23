//! Python interop lowering (ticket #27, D-0012, design/0003-python-interop.md).
//!
//! `import python "mod" as m` makes `m` a Python module handle; attribute
//! access and calls dispatch through the embedded CPython C-API, and values
//! marshal across the boundary. Nothing here needs a new ZIR instruction (the
//! strings/collections/regex pattern): every operation lowers to a `zap_py_*`
//! runtime call. A Python value flows as `Ty::Py` — a GC-managed handle that
//! owns one CPython reference (decref'd by a finalizer). Calls that can surface
//! a Python exception use `throwing_call`, so a raised Python error becomes a
//! Zap exception catchable with `try`/`except` (D-0007): inside a `try` it is an
//! `invoke` to the handler, else it unwinds to the caller / top level like any
//! other `zap_panic`.
//!
//! Extracted from `lower.rs` (#1123); a child module of `lower`, so these
//! `impl super::FnLower` methods still see `lower`'s private items.

use zapc_syntax::ast::{self, Expr};

use super::{err, FnLower, LowerError};
use crate::ir::*;

impl<'a> FnLower<'a> {
    /// `import python "module"` → a module handle. Importing is idempotent
    /// (CPython caches modules), so each reference re-imports cheaply.
    pub(super) fn lower_py_import(&mut self, module: &str) -> Value {
        let name = self.strconst(module);
        let d = self.new_value(Ty::Py);
        self.throwing_call(Some(d), "zap_py_import", vec![name]);
        d
    }

    /// `obj.attr` on a Python handle → the attribute value (another handle).
    pub(super) fn lower_py_getattr(&mut self, base: Value, name: &str) -> Value {
        let nv = self.strconst(name);
        let d = self.new_value(Ty::Py);
        self.throwing_call(Some(d), "zap_py_getattr", vec![base, nv]);
        d
    }

    /// Call a Python callable with the given Zap argument expressions. Arguments
    /// are marshaled to Python values and collected into a `list[ptr]` the
    /// runtime turns into a call tuple (any arity; keyword arguments are a
    /// documented follow-up).
    pub(super) fn lower_py_call(
        &mut self,
        callable: Value,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let argl = self.new_value(Ty::List(Elem::Ptr));
        self.emit(Inst::Call {
            dest: Some(argl),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        for a in args {
            if a.name.is_some() {
                return Err(err(
                    "keyword arguments to Python calls are not supported yet",
                ));
            }
            let p = self.marshal_to_py(&a.value)?;
            self.emit(Inst::Call {
                dest: None,
                func: "zap_list_push_ptr".to_string(),
                args: vec![argl, p],
            });
        }
        let d = self.new_value(Ty::Py);
        self.throwing_call(Some(d), "zap_py_call", vec![callable, argl]);
        Ok((d, Ty::Py))
    }

    /// Marshal one Zap value into a Python handle (ticket #27 marshaling table).
    /// Scalars and `str` convert by value; a value already on the Python side is
    /// passed through. Collections and other types are a documented follow-up.
    fn marshal_to_py(&mut self, e: &Expr) -> Result<Value, LowerError> {
        let (v, vt) = self.expr(e)?;
        let (helper, arg) = if vt == Ty::Py {
            return Ok(v);
        } else if vt == Ty::I1 {
            ("zap_py_from_bool", self.coerce(v, vt, Ty::I64))
        } else if vt.int_width().is_some() {
            ("zap_py_from_i64", self.coerce(v, vt, Ty::I64))
        } else if vt.is_float() {
            ("zap_py_from_f64", self.as_f64(v, vt))
        } else if vt == Ty::Ptr {
            ("zap_py_from_str", v)
        } else {
            return Err(err(
                "cannot marshal this value to Python yet (supported: int, float, bool, str, and Python handles)",
            ));
        };
        let d = self.new_value(Ty::Py);
        self.emit(Inst::Call {
            dest: Some(d),
            func: helper.to_string(),
            args: vec![arg],
        });
        Ok(d)
    }

    /// Call a struct's dunder method (operator overloading, D-0047). `args` are
    /// `(value, type)` pairs starting with the receiver (`self`); each is coerced
    /// to the registered parameter type. A missing dunder is a clean error (sema
    /// normally catches it first, but the ir crate is exercised without sema).
    /// Whether an argument of ZIR type `from` may be passed where parameter type
    /// `to` is expected, at the representation level (#1103). Deliberately
    /// conservative: it rejects only a value that crosses the scalar/pointer
    /// boundary in a way [`coerce`](Self::coerce) cannot legitimately bridge —
    /// the index/value type confusion the index-overload rewrite would otherwise
    /// lower to a silent word reinterpret (an `iconst` into a pointer slot).
    ///
    /// Same-class conversions stay permissive, so the happy path and the D-0067
    /// signature-less round-trip are never over-rejected: two scalars
    /// (`int<->int`, a float widening), two pointers (`str`/struct/collection,
    /// including a bare `ptr` read back from a collection element), and a
    /// `struct -> interface` coercion all pass. This matches exactly the set of
    /// conversions `coerce` performs without falling through to a bitwise
    /// reinterpret — the one scalar->pointer case it *does* handle, a numeric
    /// widened into a boxed `bigint`/`complex`, is admitted explicitly.
    fn arg_repr_compatible(&self, from: Ty, to: Ty) -> bool {
        if from == to {
            return true;
        }
        // Dynamically-typed values stay permissive.
        if matches!(from, Ty::Py) || matches!(to, Ty::Py) {
            return true;
        }
        // The one scalar -> pointer conversion `coerce` performs: a numeric
        // value widened into a boxed `bigint`/`complex`.
        if matches!(to, Ty::Bigint | Ty::Complex) && (from.int_width().is_some() || from.is_float())
        {
            return true;
        }
        // Otherwise a value may only be passed when it stays on the same side of
        // the scalar/pointer representation boundary. A scalar (`i64`, a `char`,
        // an enum tag, a float) fed into a pointer parameter (a `str`, a struct,
        // any collection) — or the reverse — is the #1103 confusion.
        from.is_ptr() == to.is_ptr()
    }

    pub(super) fn call_dunder(
        &mut self,
        sid: u32,
        dunder: &str,
        args: Vec<(Value, Ty)>,
    ) -> Result<(Value, Ty), LowerError> {
        let mangled = format!("{}__{}", self.ctx.names[sid as usize], dunder);
        let (ptys, ret) = self.sigs.get(&mangled).cloned().ok_or_else(|| {
            err(format!(
                "type '{}' does not define '{}' (required by this operator; D-0047)",
                self.ctx.names[sid as usize], dunder
            ))
        })?;
        let mut argvals = Vec::with_capacity(args.len());
        for (i, (v, vt)) in args.into_iter().enumerate() {
            let target = *ptys.get(i).unwrap_or(&vt);
            // The index-overload dunders (D-0047) are reached by the `a[i]` /
            // `a[i] = v` rewrite, which — unlike a direct method call — has no
            // sema argument check to lean on: a generic receiver's instantiation
            // is erased in sema (`Box[str]` -> `Named("Box")`, the key variable
            // `K` -> `Unknown`), so a wrong index/value type slips through to
            // here (#1103). Verify the argument's representation is compatible
            // with the declared parameter before forming the call, so a mismatch
            // (e.g. an `i64` index fed into a `str`/pointer key slot) is a clean
            // `ZAP4021` type error — the same code a normal argument/parameter
            // mismatch produces — instead of a silent `coerce` word reinterpret.
            if matches!(dunder, "__getitem__" | "__setitem__")
                && !self.arg_repr_compatible(vt, target)
            {
                return Err(LowerError {
                    code: zapc_diag::ErrorCode::ZAP4021,
                    message: format!(
                        "argument {i} to `{}.{dunder}` has type `{}`, which is not \
                         compatible with the parameter type `{}` (the index/value \
                         type does not match the overload's declared type; D-0047)",
                        self.ctx.names[sid as usize],
                        self.render_ty(&vt),
                        self.render_ty(&target),
                    ),
                    derive: None,
                });
            }
            argvals.push(self.coerce(v, vt, target));
        }
        let dest = if ret == Ty::Void {
            None
        } else {
            Some(self.new_value(ret))
        };
        self.throwing_call(dest, &mangled, argvals);
        let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
        Ok((rv, if ret == Ty::Void { Ty::I32 } else { ret }))
    }
}
