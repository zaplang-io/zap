//! The tree-walking evaluator over the comptime fragment (#1250).
//!
//! `impl Evaluator` lives here: expression and statement evaluation, the record
//! accessors, the pure builtins, and the non-recursive user-function call path.
//! Every unsupported construct routes to [`CtError::not_available`] at the
//! construct's own span — the physical soundness backstop (epic #1249, decision
//! 2b).

use std::rc::Rc;

use zapc_sema_types::TypeShape;
use zapc_syntax::ast::{Arg, ArgSpread, BinOp, Block, Expr, Stmt, UnOp};
use zapc_syntax::span::Span;

use crate::error::CtError;
use crate::ops;
use crate::value::Value;
use crate::{Env, Evaluator, Flow};

impl<'a> Evaluator<'a> {
    // -----------------------------------------------------------------------
    // budget
    // -----------------------------------------------------------------------

    /// Charge one evaluation step, failing fast when the fixed budget is spent.
    fn bump(&mut self, span: Span) -> Result<(), CtError> {
        self.steps += 1;
        if self.steps > self.step_cap {
            return Err(CtError::budget(
                format!(
                    "comptime evaluation exceeded the step limit ({} steps)",
                    self.step_cap
                ),
                span,
            ));
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // expressions
    // -----------------------------------------------------------------------

    pub(crate) fn eval_expr(&mut self, expr: &Expr, env: &mut Env) -> Result<Value, CtError> {
        self.bump(expr_span(expr))?;
        match expr {
            Expr::Int(lex, span) => match parse_int_lit(lex) {
                Some(mag) if mag <= i64::MAX as u128 => Ok(Value::Int(mag as i64)),
                // `9223372036854775808` is only legal as the operand of unary
                // `-` (→ INT64_MIN), handled in the `Unary`/`Neg` arm. On its
                // own it overflows int64 and so is a bigint value, deferred.
                _ => Err(CtError::not_available(
                    format!("integer literal `{lex}` outside the int64 range (bigint)"),
                    *span,
                )),
            },
            Expr::Float(_, span) => Err(CtError::not_available("floating-point value", *span)),
            Expr::Imaginary(_, span) => {
                Err(CtError::not_available("imaginary/complex value", *span))
            }
            Expr::Str {
                value,
                bytes,
                fstring,
                span,
                ..
            } => {
                if *fstring {
                    Err(CtError::not_available("f-string", *span))
                } else if *bytes {
                    Err(CtError::not_available("bytes literal", *span))
                } else {
                    Ok(Value::str(value.as_str()))
                }
            }
            Expr::Bool(b, _) => Ok(Value::Bool(*b)),
            Expr::None(span) => Err(CtError::not_available("`None` value (optionals)", *span)),
            Expr::Name(name, span) => self
                .lookup_name(name, env)
                .ok_or_else(|| CtError::not_available(format!("name `{name}`"), *span)),
            Expr::Tuple(elems, _) => {
                let vals = self.eval_all(elems, env)?;
                Ok(Value::tuple(vals))
            }
            Expr::List(elems, span) => {
                for e in elems {
                    if matches!(e, Expr::Starred { .. }) {
                        return Err(CtError::not_available("list spread (`*`) element", *span));
                    }
                }
                let vals = self.eval_all(elems, env)?;
                Ok(Value::list(vals))
            }
            Expr::Dict(_, span) => Err(CtError::not_available("dict value", *span)),
            Expr::Set(_, span) => Err(CtError::not_available("set value", *span)),
            Expr::Starred { span, .. } => {
                Err(CtError::not_available("spread (`*`) expression", *span))
            }
            Expr::ListComp { span, .. }
            | Expr::SetComp { span, .. }
            | Expr::DictComp { span, .. }
            | Expr::GenExp { span, .. } => Err(CtError::not_available(
                "comprehension / generator (use `comptime for`, #1251)",
                *span,
            )),
            Expr::Unary { op, expr, span } => self.eval_unary(*op, expr, *span, env),
            Expr::Binary { op, lhs, rhs, span } => self.eval_binary(*op, lhs, rhs, *span, env),
            Expr::Compare { first, rest, span } => self.eval_compare(first, rest, *span, env),
            Expr::Call { callee, args, span } => self.eval_call(callee, args, *span, env),
            Expr::Index { base, index, span } => self.eval_index(base, index, *span, env),
            Expr::Slice {
                base,
                lo,
                hi,
                step,
                span,
            } => self.eval_slice(base, lo, hi, step, *span, env),
            Expr::SliceSpec { span, .. } => {
                Err(CtError::not_available("bare slice specifier", *span))
            }
            Expr::Attr { base, name, span } => {
                let base_val = self.eval_expr(base, env)?;
                self.eval_attr(&base_val, name, *span)
            }
            Expr::Lambda { span, .. } => Err(CtError::not_available("lambda / closure", *span)),
            Expr::RegexMatch { span, .. } | Expr::RegexSubst { span, .. } => {
                Err(CtError::not_available("regular expression", *span))
            }
            Expr::ChanRecv { span, .. } => {
                Err(CtError::not_available("channel receive (`<-`)", *span))
            }
            Expr::Yield { span, .. } => Err(CtError::not_available("`yield`", *span)),
            Expr::IfElse {
                cond,
                then,
                els,
                span,
            } => {
                let c = self.eval_expr(cond, env)?;
                if self.as_bool(&c, *span)? {
                    self.eval_expr(then, env)
                } else {
                    self.eval_expr(els, env)
                }
            }
            Expr::Named { span, .. } => {
                Err(CtError::not_available("walrus assignment (`:=`)", *span))
            }
        }
    }

    /// Evaluate a slice of expressions in order.
    fn eval_all(&mut self, exprs: &[Expr], env: &mut Env) -> Result<Vec<Value>, CtError> {
        exprs.iter().map(|e| self.eval_expr(e, env)).collect()
    }

    fn lookup_name(&self, name: &str, env: &Env) -> Option<Value> {
        env.lookup(name).cloned()
    }

    // -----------------------------------------------------------------------
    // operators
    // -----------------------------------------------------------------------

    fn eval_unary(
        &mut self,
        op: UnOp,
        operand: &Expr,
        span: Span,
        env: &mut Env,
    ) -> Result<Value, CtError> {
        // `-9223372036854775808` is the one way to spell INT64_MIN: the literal
        // magnitude overflows int64 on its own but negates to a valid value.
        if let (UnOp::Neg, Expr::Int(lex, _)) = (op, operand) {
            if parse_int_lit(lex) == Some(1u128 << 63) {
                return Ok(Value::Int(i64::MIN));
            }
        }
        let v = self.eval_expr(operand, env)?;
        match op {
            UnOp::Neg => Ok(Value::Int(self.as_int(&v, span)?.wrapping_neg())),
            UnOp::Pos => Ok(Value::Int(self.as_int(&v, span)?)),
            UnOp::Not => Ok(Value::Bool(!self.as_bool(&v, span)?)),
            UnOp::BitNot => Ok(Value::Int(!self.as_int(&v, span)?)),
        }
    }

    fn eval_binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
        env: &mut Env,
    ) -> Result<Value, CtError> {
        // Short-circuiting boolean operators evaluate the right operand only
        // when needed (both operands are `bool` in Zap — `And | Or => Bool`).
        match op {
            BinOp::And => {
                let l = self.eval_expr(lhs, env)?;
                if !self.as_bool(&l, span)? {
                    return Ok(Value::Bool(false));
                }
                let r = self.eval_expr(rhs, env)?;
                return Ok(Value::Bool(self.as_bool(&r, span)?));
            }
            BinOp::Or => {
                let l = self.eval_expr(lhs, env)?;
                if self.as_bool(&l, span)? {
                    return Ok(Value::Bool(true));
                }
                let r = self.eval_expr(rhs, env)?;
                return Ok(Value::Bool(self.as_bool(&r, span)?));
            }
            _ => {}
        }
        let l = self.eval_expr(lhs, env)?;
        let r = self.eval_expr(rhs, env)?;
        self.apply_binop(op, &l, &r, span)
    }

    fn apply_binop(&self, op: BinOp, l: &Value, r: &Value, span: Span) -> Result<Value, CtError> {
        use BinOp::*;
        match op {
            Add => match (l, r) {
                (Value::Int(a), Value::Int(b)) => Ok(Value::Int(ops::int_add(*a, *b))),
                (Value::Str(a), Value::Str(b)) => Ok(Value::str(format!("{a}{b}"))),
                (Value::List(a), Value::List(b)) => {
                    let mut v = Vec::with_capacity(a.len() + b.len());
                    v.extend(a.iter().cloned());
                    v.extend(b.iter().cloned());
                    Ok(Value::list(v))
                }
                _ => Err(self.binop_type_err("+", l, r, span)),
            },
            Sub => self.int_binop(l, r, span, "-", |a, b| Ok(Value::Int(ops::int_sub(a, b)))),
            Mul => self.int_binop(l, r, span, "*", |a, b| Ok(Value::Int(ops::int_mul(a, b)))),
            FloorDiv => self.int_binop(l, r, span, "//", |a, b| {
                Ok(Value::Int(ops::int_floordiv(a, b, span)?))
            }),
            Mod => self.int_binop(l, r, span, "%", |a, b| {
                Ok(Value::Int(ops::int_mod(a, b, span)?))
            }),
            Shl => self.int_binop(l, r, span, "<<", |a, b| {
                Ok(Value::Int(ops::int_shl(a, b, span)?))
            }),
            Shr => self.int_binop(l, r, span, ">>", |a, b| {
                Ok(Value::Int(ops::int_shr(a, b, span)?))
            }),
            BitAnd => self.bitwise(l, r, span, "&", |a, b| a & b, |a, b| a & b),
            BitOr => self.bitwise(l, r, span, "|", |a, b| a | b, |a, b| a | b),
            BitXor => self.bitwise(l, r, span, "^", |a, b| a ^ b, |a, b| a ^ b),
            Eq => Ok(Value::Bool(ops::value_eq(l, r, span)?)),
            Ne => Ok(Value::Bool(!ops::value_eq(l, r, span)?)),
            Lt => Ok(Value::Bool(ops::value_cmp(l, r, span)?.is_lt())),
            Le => Ok(Value::Bool(ops::value_cmp(l, r, span)?.is_le())),
            Gt => Ok(Value::Bool(ops::value_cmp(l, r, span)?.is_gt())),
            Ge => Ok(Value::Bool(ops::value_cmp(l, r, span)?.is_ge())),
            In => Ok(Value::Bool(self.contains(r, l, span)?)),
            NotIn => Ok(Value::Bool(!self.contains(r, l, span)?)),
            // Out of the B1 fragment.
            Div => Err(CtError::not_available(
                "true division `/` (a float result)",
                span,
            )),
            Pow => Err(CtError::not_available("exponentiation `**`", span)),
            MatMul => Err(CtError::not_available("matrix multiply `@`", span)),
            And | Or => unreachable!("handled in eval_binary"),
            Is => Err(CtError::not_available("identity comparison `is`", span)),
            IsNot => Err(CtError::not_available("identity comparison `is not`", span)),
            Send => Err(CtError::not_available("channel send (`<-`)", span)),
        }
    }

    /// Apply an `int64`-only binary op, type-checking both operands.
    fn int_binop(
        &self,
        l: &Value,
        r: &Value,
        span: Span,
        sym: &str,
        f: impl FnOnce(i64, i64) -> Result<Value, CtError>,
    ) -> Result<Value, CtError> {
        match (l, r) {
            (Value::Int(a), Value::Int(b)) => f(*a, *b),
            _ => Err(self.binop_type_err(sym, l, r, span)),
        }
    }

    /// Apply a bitwise op defined on both `int64` and `bool` operands.
    fn bitwise(
        &self,
        l: &Value,
        r: &Value,
        span: Span,
        sym: &str,
        fi: impl FnOnce(i64, i64) -> i64,
        fb: impl FnOnce(bool, bool) -> bool,
    ) -> Result<Value, CtError> {
        match (l, r) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(fi(*a, *b))),
            (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(fb(*a, *b))),
            _ => Err(self.binop_type_err(sym, l, r, span)),
        }
    }

    fn binop_type_err(&self, sym: &str, l: &Value, r: &Value, span: Span) -> CtError {
        CtError::value(
            format!(
                "operator `{sym}` is not defined for `{}` and `{}` at comptime",
                l.type_name(),
                r.type_name()
            ),
            span,
        )
    }

    /// `needle in haystack`: substring for `str`, membership for `list`/`tuple`.
    fn contains(&self, haystack: &Value, needle: &Value, span: Span) -> Result<bool, CtError> {
        match haystack {
            Value::Str(s) => match needle {
                Value::Str(sub) => Ok(s.contains(sub.as_ref())),
                _ => Err(CtError::value(
                    format!(
                        "`in` on a `str` expects a `str`, found `{}` at comptime",
                        needle.type_name()
                    ),
                    span,
                )),
            },
            Value::List(xs) | Value::Tuple(xs) => {
                for x in xs.iter() {
                    if ops::value_eq(x, needle, span)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => Err(CtError::not_available(
                format!("`in` over a `{}`", haystack.type_name()),
                span,
            )),
        }
    }

    fn eval_compare(
        &mut self,
        first: &Expr,
        rest: &[(BinOp, Expr)],
        span: Span,
        env: &mut Env,
    ) -> Result<Value, CtError> {
        // Chained comparison with Python semantics: `(x0 op1 x1) and (x1 op2 x2)
        // ...`, each operand evaluated once, short-circuiting at the first link
        // that does not hold.
        let mut prev = self.eval_expr(first, env)?;
        for (op, rhs_expr) in rest {
            let rhs = self.eval_expr(rhs_expr, env)?;
            let link = self.apply_binop(*op, &prev, &rhs, span)?;
            if !self.as_bool(&link, span)? {
                return Ok(Value::Bool(false));
            }
            prev = rhs;
        }
        Ok(Value::Bool(true))
    }

    // -----------------------------------------------------------------------
    // indexing / slicing
    // -----------------------------------------------------------------------

    fn eval_index(
        &mut self,
        base: &Expr,
        index: &Expr,
        span: Span,
        env: &mut Env,
    ) -> Result<Value, CtError> {
        let base_val = self.eval_expr(base, env)?;
        let idx_val = self.eval_expr(index, env)?;
        let i = self.as_int(&idx_val, span)?;
        match &base_val {
            Value::Str(s) => Ok(Value::Char(ops::str_index(s, i, span)?)),
            Value::List(xs) => ops::seq_index(xs, i, span, "list"),
            Value::Tuple(xs) => ops::seq_index(xs, i, span, "tuple"),
            _ => Err(CtError::not_available(
                format!("indexing a `{}`", base_val.type_name()),
                span,
            )),
        }
    }

    fn eval_slice(
        &mut self,
        base: &Expr,
        lo: &Option<Box<Expr>>,
        hi: &Option<Box<Expr>>,
        step: &Option<Box<Expr>>,
        span: Span,
        env: &mut Env,
    ) -> Result<Value, CtError> {
        let base_val = self.eval_expr(base, env)?;
        let lo = self.opt_int(lo, env, span)?;
        let hi = self.opt_int(hi, env, span)?;
        let step = self.opt_int(step, env, span)?;
        match &base_val {
            Value::Str(s) => {
                let chars: Vec<char> = s.chars().collect();
                let idxs = ops::slice_indices(chars.len() as i64, lo, hi, step, span)?;
                Ok(Value::str(
                    idxs.into_iter().map(|k| chars[k]).collect::<String>(),
                ))
            }
            Value::List(xs) => {
                let idxs = ops::slice_indices(xs.len() as i64, lo, hi, step, span)?;
                Ok(Value::list(
                    idxs.into_iter().map(|k| xs[k].clone()).collect(),
                ))
            }
            Value::Tuple(xs) => {
                let idxs = ops::slice_indices(xs.len() as i64, lo, hi, step, span)?;
                Ok(Value::tuple(
                    idxs.into_iter().map(|k| xs[k].clone()).collect(),
                ))
            }
            _ => Err(CtError::not_available(
                format!("slicing a `{}`", base_val.type_name()),
                span,
            )),
        }
    }

    fn opt_int(
        &mut self,
        e: &Option<Box<Expr>>,
        env: &mut Env,
        span: Span,
    ) -> Result<Option<i64>, CtError> {
        match e {
            None => Ok(None),
            Some(expr) => {
                let v = self.eval_expr(expr, env)?;
                Ok(Some(self.as_int(&v, span)?))
            }
        }
    }

    // -----------------------------------------------------------------------
    // reflection-record accessors
    // -----------------------------------------------------------------------

    fn eval_attr(&self, base: &Value, name: &str, span: Span) -> Result<Value, CtError> {
        match base {
            Value::Type(ti) => match name {
                "kind" => Ok(Value::Kind(ti.kind())),
                "name" => ti.name().map(Value::str).ok_or_else(|| {
                    CtError::value(
                        "`.name` is not available for this type kind at comptime",
                        span,
                    )
                }),
                "fields" => Ok(Value::list(
                    ti.fields()
                        .iter()
                        .map(|f| Value::Field(Rc::new(f.clone())))
                        .collect(),
                )),
                "variants" => Ok(Value::list(
                    ti.variants()
                        .iter()
                        .map(|v| Value::Variant(Rc::new(v.clone())))
                        .collect(),
                )),
                // Container element accessors (#1253): resolve the element/key/
                // value type back to a nested `TypeInfo` so a comptime
                // kind-switch can recurse on it.
                "elem" => match &ti.shape {
                    TypeShape::List { elem } => self.resolve_ty_value(elem, span),
                    _ => Err(container_accessor_err("elem", "list", ti, span)),
                },
                "key" => match &ti.shape {
                    TypeShape::Dict { key, .. } => self.resolve_ty_value(key, span),
                    _ => Err(container_accessor_err("key", "dict", ti, span)),
                },
                "value" => match &ti.shape {
                    TypeShape::Dict { value, .. } => self.resolve_ty_value(value, span),
                    _ => Err(container_accessor_err("value", "dict", ti, span)),
                },
                "inner" => match &ti.shape {
                    TypeShape::Optional { elem } => {
                        if matches!(elem, zapc_sema_types::Ty::Unknown) {
                            Err(CtError::not_available(
                                "a bare `Optional`'s element type at comptime",
                                span,
                            ))
                        } else {
                            self.resolve_ty_value(elem, span)
                        }
                    }
                    _ => Err(container_accessor_err("inner", "optional", ti, span)),
                },
                "elems" => match &ti.shape {
                    TypeShape::Tuple { elems } => {
                        let mut out = Vec::with_capacity(elems.len());
                        for e in elems {
                            out.push(self.resolve_ty_value(e, span)?);
                        }
                        Ok(Value::list(out))
                    }
                    _ => Err(container_accessor_err("elems", "tuple", ti, span)),
                },
                _ => Err(no_field_err("TypeInfo", name, span)),
            },
            Value::Field(f) => match name {
                "name" => Ok(Value::str(f.name.as_str())),
                "index" => Ok(Value::Int(f.index as i64)),
                "attrs" => Ok(Value::list(
                    f.attrs
                        .iter()
                        .map(|a| Value::Attr(Rc::new(a.clone())))
                        .collect(),
                )),
                // A field's own comptime type (#1253): resolve from the declared
                // type (which preserves an `Optional`/container element the
                // resolved `Ty` erases), so a kind-switch can recurse on it.
                "ty" | "type_info" => self.resolve_type_value(&f.decl_ty, span),
                _ => Err(no_field_err("FieldInfo", name, span)),
            },
            Value::Variant(v) => match name {
                "name" => Ok(Value::str(v.name.as_str())),
                "index" => Ok(Value::Int(v.index as i64)),
                "fields" => Ok(Value::list(
                    v.fields
                        .iter()
                        .map(|f| Value::Field(Rc::new(f.clone())))
                        .collect(),
                )),
                "attrs" => Ok(Value::list(
                    v.attrs
                        .iter()
                        .map(|a| Value::Attr(Rc::new(a.clone())))
                        .collect(),
                )),
                _ => Err(no_field_err("VariantInfo", name, span)),
            },
            Value::Attr(a) => match name {
                "name" => Ok(Value::str(a.name.as_str())),
                "args" => Ok(Value::list(
                    a.args
                        .iter()
                        .map(|x| Value::AttrArg(Rc::new(x.clone())))
                        .collect(),
                )),
                _ => Err(no_field_err("AttrInfo", name, span)),
            },
            Value::AttrArg(a) => match name {
                "value" => Ok(Value::str(a.value.as_str())),
                "is_named" => Ok(Value::Bool(a.name.is_some())),
                "name" => a.name.as_deref().map(Value::str).ok_or_else(|| {
                    CtError::value(
                        "positional attribute argument has no `.name` at comptime",
                        span,
                    )
                }),
                _ => Err(no_field_err("AttrArg", name, span)),
            },
            Value::Kind(k) => match name {
                "name" => Ok(Value::str(k.as_str())),
                _ => Err(no_field_err("TypeKind", name, span)),
            },
            _ => Err(CtError::not_available(
                format!("attribute access `.{name}` on a `{}`", base.type_name()),
                span,
            )),
        }
    }

    // -----------------------------------------------------------------------
    // calls (builtins, string methods, user functions)
    // -----------------------------------------------------------------------

    fn eval_call(
        &mut self,
        callee: &Expr,
        args: &[Arg],
        span: Span,
        env: &mut Env,
    ) -> Result<Value, CtError> {
        for a in args {
            if !matches!(a.spread, ArgSpread::None) {
                return Err(CtError::not_available(
                    "argument unpacking (`*` / `**`)",
                    span,
                ));
            }
        }
        match callee {
            Expr::Attr { base, name, .. } => {
                // `reflect.<fn>(...)` module builtins (#1253 override hook).
                if matches!(&**base, Expr::Name(m, _) if m == "reflect") {
                    return self.eval_reflect_builtin(name, args, span, env);
                }
                self.eval_method(base, name, args, span, env)
            }
            Expr::Name(fname, _) => self.eval_name_call(fname, args, span, env),
            _ => Err(CtError::not_available("call of a computed callee", span)),
        }
    }

    /// `reflect.<fn>(...)` comptime builtins. For now the override hook:
    /// `reflect.has_override(info, "serialize")` -> `bool`, true when the type
    /// `info` describes declares that method, so a kind-switch can short-circuit
    /// to a user-supplied handler instead of recursing (#1248 resolved decision;
    /// full override *policy* is E3).
    fn eval_reflect_builtin(
        &mut self,
        name: &str,
        args: &[Arg],
        span: Span,
        env: &mut Env,
    ) -> Result<Value, CtError> {
        match name {
            "has_override" => {
                let argv = self.eval_args(args, env)?;
                if argv.len() != 2 {
                    return Err(CtError::value(
                        "`reflect.has_override(info, name)` takes exactly 2 arguments",
                        span,
                    ));
                }
                let type_name = match &argv[0] {
                    Value::Type(ti) => ti.name().map(|s| s.to_string()),
                    other => {
                        return Err(CtError::value(
                            format!(
                                "`reflect.has_override` expects a `TypeInfo`, found `{}`",
                                other.type_name()
                            ),
                            span,
                        ))
                    }
                };
                let method = match &argv[1] {
                    Value::Str(s) => s.to_string(),
                    other => {
                        return Err(CtError::value(
                            format!(
                                "`reflect.has_override` method name must be a `str`, found `{}`",
                                other.type_name()
                            ),
                            span,
                        ))
                    }
                };
                let reg = self.registry.ok_or_else(|| {
                    CtError::not_available("`reflect.has_override` without a type registry", span)
                })?;
                Ok(Value::Bool(
                    type_name.is_some_and(|t| reg.has_method(&t, &method)),
                ))
            }
            other => Err(CtError::not_available(
                format!("`reflect.{other}(...)`"),
                span,
            )),
        }
    }

    /// Resolve a resolved `Ty` (from a container shape) to a nested `TypeInfo`
    /// value via the registry (#1253).
    fn resolve_ty_value(&self, ty: &zapc_sema_types::Ty, span: Span) -> Result<Value, CtError> {
        let reg = self.registry.ok_or_else(|| no_registry_err(span))?;
        Ok(Value::Type(Rc::new(reg.resolve_ty(ty, span)?)))
    }

    /// Resolve a declared `Type` (e.g. a field's `decl_ty`) to a nested
    /// `TypeInfo` value via the registry (#1253).
    fn resolve_type_value(&self, t: &zapc_syntax::ast::Type, span: Span) -> Result<Value, CtError> {
        let reg = self.registry.ok_or_else(|| no_registry_err(span))?;
        Ok(Value::Type(Rc::new(reg.resolve_type(t, span)?)))
    }

    fn eval_method(
        &mut self,
        base: &Expr,
        name: &str,
        args: &[Arg],
        span: Span,
        env: &mut Env,
    ) -> Result<Value, CtError> {
        let base_val = self.eval_expr(base, env)?;
        // B1 string methods: case folding only (concat is `+`, slice is `[:]`).
        if let Value::Str(s) = &base_val {
            match name {
                "upper" | "lower" if args.is_empty() => {
                    let out = if name == "upper" {
                        ops::str_upper(s)
                    } else {
                        ops::str_lower(s)
                    };
                    return Ok(Value::str(out));
                }
                _ => {}
            }
        }
        Err(CtError::not_available(
            format!("method `.{name}()` on a `{}`", base_val.type_name()),
            span,
        ))
    }

    fn eval_name_call(
        &mut self,
        fname: &str,
        args: &[Arg],
        span: Span,
        env: &mut Env,
    ) -> Result<Value, CtError> {
        // Pure builtins first; then user functions.
        match fname {
            "len" | "ord" | "chr" | "char" | "str" => {
                let argv = self.eval_args(args, env)?;
                self.eval_builtin(fname, argv, span)
            }
            _ => {
                let argv = self.eval_args(args, env)?;
                self.call_fn(fname, argv, span)
            }
        }
    }

    fn eval_args(&mut self, args: &[Arg], env: &mut Env) -> Result<Vec<Value>, CtError> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            if a.name.is_some() {
                return Err(CtError::not_available(
                    "keyword argument in a comptime call",
                    a.span,
                ));
            }
            out.push(self.eval_expr(&a.value, env)?);
        }
        Ok(out)
    }

    fn eval_builtin(&self, fname: &str, argv: Vec<Value>, span: Span) -> Result<Value, CtError> {
        let arity_err = |n: usize| {
            CtError::value(
                format!("`{fname}()` takes exactly {n} argument(s) at comptime"),
                span,
            )
        };
        match fname {
            "len" => {
                if argv.len() != 1 {
                    return Err(arity_err(1));
                }
                let n = match &argv[0] {
                    Value::Str(s) => s.chars().count(),
                    Value::List(xs) | Value::Tuple(xs) => xs.len(),
                    other => {
                        return Err(CtError::value(
                            format!("`len()` of a `{}` at comptime", other.type_name()),
                            span,
                        ))
                    }
                };
                Ok(Value::Int(n as i64))
            }
            "ord" => {
                if argv.len() != 1 {
                    return Err(arity_err(1));
                }
                match &argv[0] {
                    Value::Char(c) => Ok(Value::Int(*c as i64)),
                    other => Err(CtError::value(
                        format!(
                            "`ord()` expects a `char`, found `{}` at comptime",
                            other.type_name()
                        ),
                        span,
                    )),
                }
            }
            "chr" | "char" => {
                if argv.len() != 1 {
                    return Err(arity_err(1));
                }
                let n = self.as_int(&argv[0], span)?;
                Ok(Value::Char(ops::int_to_char(n, span)?))
            }
            "str" => {
                if argv.len() != 1 {
                    return Err(arity_err(1));
                }
                Ok(Value::str(ops::value_to_str(&argv[0], span)?))
            }
            _ => unreachable!("eval_builtin called with non-builtin"),
        }
    }

    /// Call a registered user function with evaluated positional `args`.
    /// Non-recursive only: a direct or transitive self-call is out of fragment.
    pub(crate) fn call_fn(
        &mut self,
        name: &str,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, CtError> {
        let func = match self.funcs.get(name) {
            Some(f) => *f,
            None => {
                return Err(CtError::not_available(
                    format!("call to `{name}` (not a comptime-reducible function)"),
                    span,
                ))
            }
        };
        if self.active.contains(&func.name.as_str()) {
            return Err(CtError::not_available(
                format!("recursion (call to `{name}`)"),
                span,
            ));
        }
        if self.active.len() >= self.depth_cap {
            return Err(CtError::budget(
                format!(
                    "comptime call depth exceeded the limit ({} frames)",
                    self.depth_cap
                ),
                span,
            ));
        }

        // Bind parameters positionally into a fresh function scope (comptime
        // functions do not close over their caller's locals). The `TypeKind`
        // constants are comptime globals, so seed them first (a parameter of the
        // same name still shadows, bound next) — this lets a kind-switch inside
        // a comptime function write `if info.kind == Struct:` (#1253).
        let mut fenv = Env::new();
        crate::seed_kind_constants(&mut fenv);
        self.bind_params(func, &args, &mut fenv, span)?;

        self.active.push(func.name.as_str());
        let result = self.eval_block(&func.body, &mut fenv);
        self.active.pop();

        match result? {
            Flow::Return(v) => Ok(v),
            Flow::Normal => Err(CtError::value(
                format!("comptime function `{name}` did not return a value"),
                span,
            )),
        }
    }

    fn bind_params(
        &mut self,
        func: &'a zapc_syntax::ast::Function,
        args: &[Value],
        fenv: &mut Env,
        span: Span,
    ) -> Result<(), CtError> {
        for (i, p) in func.params.iter().enumerate() {
            if p.variadic || p.kwargs {
                return Err(CtError::not_available(
                    format!(
                        "variadic / keyword-collector parameter `{}` in a comptime function",
                        p.name
                    ),
                    span,
                ));
            }
            if let Some(v) = args.get(i) {
                fenv.define(&p.name, v.clone());
            } else if let Some(default) = &p.default {
                // Defaults see the parameters bound before them.
                let dv = self.eval_expr(default, fenv)?;
                fenv.define(&p.name, dv);
            } else {
                return Err(CtError::value(
                    format!(
                        "comptime call to `{}` is missing argument `{}`",
                        func.name, p.name
                    ),
                    span,
                ));
            }
        }
        if args.len() > func.params.len() {
            return Err(CtError::value(
                format!(
                    "comptime call to `{}` passed {} arguments but it takes {}",
                    func.name,
                    args.len(),
                    func.params.len()
                ),
                span,
            ));
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // statements / blocks
    // -----------------------------------------------------------------------

    pub(crate) fn eval_block(&mut self, block: &Block, env: &mut Env) -> Result<Flow, CtError> {
        env.push();
        let mut flow = Flow::Normal;
        for stmt in &block.stmts {
            match self.eval_stmt(stmt, env) {
                Ok(Flow::Normal) => {}
                Ok(Flow::Return(v)) => {
                    flow = Flow::Return(v);
                    break;
                }
                Err(e) => {
                    env.pop();
                    return Err(e);
                }
            }
        }
        env.pop();
        Ok(flow)
    }

    fn eval_stmt(&mut self, stmt: &Stmt, env: &mut Env) -> Result<Flow, CtError> {
        self.bump(stmt_span(stmt))?;
        match stmt {
            Stmt::Let { name, value, .. } => {
                let v = self.eval_expr(value, env)?;
                env.define(name, v);
                Ok(Flow::Normal)
            }
            Stmt::Expr(e) => {
                self.eval_expr(e, env)?;
                Ok(Flow::Normal)
            }
            Stmt::If {
                branches,
                else_block,
                span,
            } => {
                // `comptime if`: pick the first branch whose condition holds,
                // short-circuiting the rest.
                for (cond, body) in branches {
                    let c = self.eval_expr(cond, env)?;
                    if self.as_bool(&c, *span)? {
                        return self.eval_block(body, env);
                    }
                }
                if let Some(body) = else_block {
                    return self.eval_block(body, env);
                }
                Ok(Flow::Normal)
            }
            Stmt::Return { value, span } => match value {
                Some(e) => Ok(Flow::Return(self.eval_expr(e, env)?)),
                None => Err(CtError::value(
                    "a comptime function must `return` a value",
                    *span,
                )),
            },
            Stmt::Pass(_) => Ok(Flow::Normal),
            // Everything else is out of the B1 fragment.
            Stmt::Match { span, .. } => Err(CtError::not_available(
                "`match` over comptime values",
                *span,
            )),
            Stmt::For { span, .. } => Err(CtError::not_available(
                "`for` loop (use `comptime for`, #1251)",
                *span,
            )),
            Stmt::While { span, .. } | Stmt::DoWhile { span, .. } => {
                Err(CtError::not_available("`while` loop", *span))
            }
            Stmt::Assign { span, .. }
            | Stmt::AugAssign { span, .. }
            | Stmt::AssignChain { span, .. }
            | Stmt::AssignTuple { span, .. } => Err(CtError::not_available(
                "assignment (comptime uses `let` bindings)",
                *span,
            )),
            Stmt::LetTuple { span, .. } => {
                Err(CtError::not_available("tuple-destructuring `let`", *span))
            }
            Stmt::Raise { span, .. } => Err(CtError::not_available("`raise`", *span)),
            Stmt::Assert { span, .. } => Err(CtError::not_available("`assert`", *span)),
            Stmt::Try { span, .. } => Err(CtError::not_available("`try`", *span)),
            Stmt::With { span, .. } => Err(CtError::not_available("`with`", *span)),
            Stmt::Unsafe { span, .. } => Err(CtError::not_available("`unsafe`", *span)),
            Stmt::Spawn { span, .. } => Err(CtError::not_available("`spawn`", *span)),
            Stmt::Scope { span, .. } => Err(CtError::not_available("`scope`", *span)),
            Stmt::Select { span, .. } => Err(CtError::not_available("`select`", *span)),
            Stmt::Del { span, .. } => Err(CtError::not_available("`del`", *span)),
            Stmt::Cfg(c) => Err(CtError::not_available("`when` cfg block", c.span)),
            Stmt::Break(span) => Err(CtError::not_available("`break`", *span)),
            Stmt::Continue(span) => Err(CtError::not_available("`continue`", *span)),
        }
    }

    // -----------------------------------------------------------------------
    // coercions
    // -----------------------------------------------------------------------

    fn as_bool(&self, v: &Value, span: Span) -> Result<bool, CtError> {
        v.as_bool().ok_or_else(|| {
            CtError::value(
                format!("expected a `bool` at comptime, found `{}`", v.type_name()),
                span,
            )
        })
    }

    fn as_int(&self, v: &Value, span: Span) -> Result<i64, CtError> {
        v.as_int().ok_or_else(|| {
            CtError::value(
                format!("expected an `int64` at comptime, found `{}`", v.type_name()),
                span,
            )
        })
    }
}

fn no_field_err(record: &str, name: &str, span: Span) -> CtError {
    CtError::value(format!("`{record}` has no comptime field `{name}`"), span)
}

/// A container element accessor used on the wrong kind (`.elem` on a non-list, …).
fn container_accessor_err(
    accessor: &str,
    expected_kind: &str,
    ti: &zapc_sema_types::TypeInfo,
    span: Span,
) -> CtError {
    CtError::value(
        format!(
            "`.{accessor}` is only available on a `{expected_kind}`, not a `{}`",
            ti.kind().as_str()
        ),
        span,
    )
}

/// The registry is required for container recursion / `field.ty` but absent.
fn no_registry_err(span: Span) -> CtError {
    CtError::not_available(
        "a comptime type reference without a type registry (container recursion / `.ty`)",
        span,
    )
}

/// Parse an integer literal lexeme to its non-negative magnitude. Handles the
/// `0x`/`0o`/`0b` radix prefixes and `_` digit separators. Unary `-` is a
/// separate AST node, so the lexeme is always non-negative.
fn parse_int_lit(lex: &str) -> Option<u128> {
    let cleaned: String = lex.chars().filter(|c| *c != '_').collect();
    let (radix, digits) = if let Some(rest) = cleaned
        .strip_prefix("0x")
        .or_else(|| cleaned.strip_prefix("0X"))
    {
        (16, rest)
    } else if let Some(rest) = cleaned
        .strip_prefix("0o")
        .or_else(|| cleaned.strip_prefix("0O"))
    {
        (8, rest)
    } else if let Some(rest) = cleaned
        .strip_prefix("0b")
        .or_else(|| cleaned.strip_prefix("0B"))
    {
        (2, rest)
    } else {
        (10, cleaned.as_str())
    };
    if digits.is_empty() {
        return None;
    }
    u128::from_str_radix(digits, radix).ok()
}

/// The source span of an expression (mirrors `mono::expr_span`).
pub(crate) fn expr_span(e: &Expr) -> Span {
    match e {
        Expr::Int(_, s)
        | Expr::Float(_, s)
        | Expr::Imaginary(_, s)
        | Expr::Bool(_, s)
        | Expr::None(s)
        | Expr::Name(_, s)
        | Expr::Tuple(_, s)
        | Expr::List(_, s)
        | Expr::Dict(_, s)
        | Expr::Set(_, s) => *s,
        Expr::Str { span, .. }
        | Expr::Starred { span, .. }
        | Expr::ListComp { span, .. }
        | Expr::DictComp { span, .. }
        | Expr::SetComp { span, .. }
        | Expr::GenExp { span, .. }
        | Expr::Unary { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Compare { span, .. }
        | Expr::Call { span, .. }
        | Expr::Index { span, .. }
        | Expr::Slice { span, .. }
        | Expr::SliceSpec { span, .. }
        | Expr::Attr { span, .. }
        | Expr::Lambda { span, .. }
        | Expr::RegexMatch { span, .. }
        | Expr::RegexSubst { span, .. }
        | Expr::ChanRecv { span, .. }
        | Expr::Yield { span, .. }
        | Expr::IfElse { span, .. }
        | Expr::Named { span, .. } => *span,
    }
}

/// The source span of a statement.
fn stmt_span(s: &Stmt) -> Span {
    match s {
        Stmt::Let { span, .. }
        | Stmt::Assign { span, .. }
        | Stmt::AugAssign { span, .. }
        | Stmt::AssignChain { span, .. }
        | Stmt::If { span, .. }
        | Stmt::While { span, .. }
        | Stmt::DoWhile { span, .. }
        | Stmt::For { span, .. }
        | Stmt::Match { span, .. }
        | Stmt::Select { span, .. }
        | Stmt::Return { span, .. }
        | Stmt::Raise { span, .. }
        | Stmt::Assert { span, .. }
        | Stmt::Try { span, .. }
        | Stmt::With { span, .. }
        | Stmt::Unsafe { span, .. }
        | Stmt::Spawn { span, .. }
        | Stmt::Scope { span, .. }
        | Stmt::LetTuple { span, .. }
        | Stmt::AssignTuple { span, .. }
        | Stmt::Del { span, .. } => *span,
        Stmt::Expr(e) => expr_span(e),
        Stmt::Cfg(c) => c.span,
        Stmt::Break(span) | Stmt::Continue(span) | Stmt::Pass(span) => *span,
    }
}
