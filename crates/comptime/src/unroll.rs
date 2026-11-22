//! The `comptime for` loop unroller (#1251).
//!
//! `comptime for f in info.fields:` is a **compile-time-unrolled** loop: the
//! body is cloned once per field (or variant), and in each clone every
//! comptime-reducible sub-expression — the ones that depend on the loop
//! variable `f` — is folded to a literal. It is **not** a runtime loop over
//! `any`, and there is no boxing.
//!
//! # HQ3 — unroll ≠ forced inline
//!
//! The unroller only *folds the comptime operands* of the body; it never inlines
//! a called function. `emit(reflect.field(value, comptime f.index))` specializes
//! to `emit(reflect.field(value, 0))` — still an ordinary, outlineable **call**
//! to `emit`. So code size is bounded by field count, not blown up by inlining:
//! HQ3 is structural here, not an optimization choice.
//!
//! # Folding rule (engine-first cut)
//!
//! A sub-expression is folded to a literal iff it **evaluates** against the
//! current comptime environment to a **materializable** value (a scalar, or a
//! list/tuple of materializable values — see [`materialize`]). Anything that
//! references a runtime binding (e.g. the `value` being serialized) fails to
//! evaluate and is left as runtime AST, with its own comptime sub-operands
//! folded. A comptime user-function call that reduces to a constant *is* folded
//! (the HQ9 `rename_all` case) — that is not inlining a runtime call, it is
//! computing a compile-time constant.
//!
//! The `comptime <expr>` prefix that *forces* reduction (and surfaces an error
//! when a value cannot be reduced) is part of the deferred parser surface
//! (#1251 follow-up); this engine folds best-effort over the comptime env. The
//! iterable and the per-iteration binding are driven here; the mono call site
//! that supplies the concrete type's `info` lands with that surface.

use zapc_syntax::ast::{
    Arg, ArgSpread, Block, ExceptClause, Expr, MatchArm, SelectCase, SelectOp, Stmt, WithItem,
};
use zapc_syntax::span::Span;

use crate::value::Value;
use crate::{CtError, Env, Evaluator};

/// Reconstruct a comptime [`Value`] as an equivalent literal AST expression,
/// anchored at `span`. Scalars and `list`/`tuple` aggregates of materializable
/// values round-trip; reflection records and `TypeKind` tags have no runtime
/// representation and return `None` (so a body sub-expression yielding a record
/// is left for its materializable sub-accesses to fold instead).
pub fn materialize(v: &Value, span: Span) -> Option<Expr> {
    Some(match v {
        Value::Int(n) => int_literal(*n, span),
        Value::Bool(b) => Expr::Bool(*b, span),
        Value::Str(s) => Expr::Str {
            value: s.to_string(),
            raw: false,
            bytes: false,
            fstring: false,
            span,
            fstring_fields: vec![],
        },
        // No char literal surface; `char(codepoint)` reconstructs one.
        Value::Char(c) => Expr::Call {
            callee: Box::new(Expr::Name("char".to_string(), span)),
            args: vec![Arg {
                name: None,
                value: int_literal(*c as i64, span),
                spread: ArgSpread::None,
                span,
            }],
            span,
        },
        Value::List(xs) => Expr::List(materialize_all(xs, span)?, span),
        Value::Tuple(xs) => Expr::Tuple(materialize_all(xs, span)?, span),
        Value::Type(_)
        | Value::Field(_)
        | Value::Variant(_)
        | Value::Attr(_)
        | Value::AttrArg(_)
        | Value::Kind(_) => return None,
    })
}

fn materialize_all(xs: &[Value], span: Span) -> Option<Vec<Expr>> {
    xs.iter().map(|x| materialize(x, span)).collect()
}

/// A non-negative int is a bare literal; a negative int is `-<magnitude>`
/// (unary negation of a non-negative literal), exactly as the parser produces
/// signed literals — so `INT64_MIN` round-trips.
fn int_literal(n: i64, span: Span) -> Expr {
    if n >= 0 {
        Expr::Int(n.to_string(), span)
    } else {
        let magnitude = (n as i128).unsigned_abs().to_string();
        Expr::Unary {
            op: zapc_syntax::ast::UnOp::Neg,
            expr: Box::new(Expr::Int(magnitude, span)),
            span,
        }
    }
}

/// Unroll `comptime for <loop_var> in <iterable>:` over `body`, returning the
/// specialized statements to splice in place of the loop.
///
/// `iterable` is evaluated once in `base_env` and must be a comptime `list`
/// (e.g. `info.fields` / `info.variants`); the body is then cloned and
/// specialized once per element, with `loop_var` bound to that element. A
/// non-list iterable, or an evaluation failure, is a clean [`CtError`].
pub fn unroll(
    loop_var: &str,
    iterable: &Expr,
    body: &Block,
    base_env: &Env,
    ev: &mut Evaluator,
) -> Result<Vec<Stmt>, CtError> {
    let mut env = base_env.clone();
    let items = match ev.eval(iterable, &mut env)? {
        Value::List(items) => items,
        other => {
            return Err(CtError::value(
                format!(
                    "`comptime for` expects a comptime list, found `{}`",
                    other.type_name()
                ),
                crate::eval::expr_span(iterable),
            ))
        }
    };

    let mut out = Vec::new();
    for item in items.iter() {
        let mut iter_env = base_env.clone();
        iter_env.define(loop_var, item.clone());
        let specialized = specialize_block(body, &mut iter_env, ev)?;
        out.extend(specialized.stmts);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// specialization (comptime-constant folding over the body)
// ---------------------------------------------------------------------------

fn specialize_block(b: &Block, env: &mut Env, ev: &mut Evaluator) -> Result<Block, CtError> {
    let mut stmts = Vec::with_capacity(b.stmts.len());
    for st in &b.stmts {
        stmts.push(specialize_stmt(st, env, ev)?);
    }
    Ok(Block {
        stmts,
        span: b.span,
    })
}

fn fold(e: &Expr, env: &mut Env, ev: &mut Evaluator) -> Result<Expr, CtError> {
    specialize_expr(e, env, ev)
}

fn specialize_stmt(st: &Stmt, env: &mut Env, ev: &mut Evaluator) -> Result<Stmt, CtError> {
    Ok(match st {
        Stmt::Let {
            is_const,
            ty,
            name,
            value,
            span,
        } => Stmt::Let {
            is_const: *is_const,
            ty: ty.clone(),
            name: name.clone(),
            value: fold(value, env, ev)?,
            span: *span,
        },
        Stmt::Assign {
            target,
            value,
            span,
        } => Stmt::Assign {
            target: fold(target, env, ev)?,
            value: fold(value, env, ev)?,
            span: *span,
        },
        Stmt::AugAssign {
            target,
            op,
            value,
            span,
        } => Stmt::AugAssign {
            target: fold(target, env, ev)?,
            op: *op,
            value: fold(value, env, ev)?,
            span: *span,
        },
        Stmt::AssignChain {
            targets,
            value,
            span,
        } => Stmt::AssignChain {
            targets: fold_all(targets, env, ev)?,
            value: fold(value, env, ev)?,
            span: *span,
        },
        Stmt::Expr(e) => Stmt::Expr(fold(e, env, ev)?),
        Stmt::If {
            branches,
            else_block,
            span,
        } => {
            let mut nb = Vec::with_capacity(branches.len());
            for (cond, body) in branches {
                nb.push((fold(cond, env, ev)?, specialize_block(body, env, ev)?));
            }
            Stmt::If {
                branches: nb,
                else_block: match else_block {
                    Some(b) => Some(specialize_block(b, env, ev)?),
                    None => None,
                },
                span: *span,
            }
        }
        Stmt::While { cond, body, span } => Stmt::While {
            cond: fold(cond, env, ev)?,
            body: specialize_block(body, env, ev)?,
            span: *span,
        },
        Stmt::DoWhile { body, cond, span } => Stmt::DoWhile {
            body: specialize_block(body, env, ev)?,
            cond: fold(cond, env, ev)?,
            span: *span,
        },
        Stmt::For {
            target,
            iter,
            body,
            span,
        } => Stmt::For {
            target: fold(target, env, ev)?,
            iter: fold(iter, env, ev)?,
            body: specialize_block(body, env, ev)?,
            span: *span,
        },
        Stmt::Return { value, span } => Stmt::Return {
            value: match value {
                Some(e) => Some(fold(e, env, ev)?),
                None => None,
            },
            span: *span,
        },
        Stmt::Raise { value, cause, span } => Stmt::Raise {
            value: match value {
                Some(e) => Some(fold(e, env, ev)?),
                None => None,
            },
            cause: match cause {
                Some(e) => Some(fold(e, env, ev)?),
                None => None,
            },
            span: *span,
        },
        Stmt::Assert {
            cond,
            msg,
            cond_src,
            span,
        } => Stmt::Assert {
            cond: fold(cond, env, ev)?,
            msg: match msg {
                Some(e) => Some(fold(e, env, ev)?),
                None => None,
            },
            cond_src: cond_src.clone(),
            span: *span,
        },
        Stmt::Match {
            scrutinee,
            arms,
            span,
        } => Stmt::Match {
            scrutinee: fold(scrutinee, env, ev)?,
            arms: arms
                .iter()
                .map(|a| {
                    Ok(MatchArm {
                        pattern: a.pattern.clone(),
                        guard: match &a.guard {
                            Some(g) => Some(fold(g, env, ev)?),
                            None => None,
                        },
                        body: specialize_block(&a.body, env, ev)?,
                        span: a.span,
                    })
                })
                .collect::<Result<Vec<_>, CtError>>()?,
            span: *span,
        },
        Stmt::Try {
            body,
            excepts,
            finally,
            span,
        } => Stmt::Try {
            body: specialize_block(body, env, ev)?,
            excepts: excepts
                .iter()
                .map(|c| {
                    Ok(ExceptClause {
                        body: specialize_block(&c.body, env, ev)?,
                        ..c.clone()
                    })
                })
                .collect::<Result<Vec<_>, CtError>>()?,
            finally: match finally {
                Some(b) => Some(specialize_block(b, env, ev)?),
                None => None,
            },
            span: *span,
        },
        Stmt::With { items, body, span } => Stmt::With {
            items: items
                .iter()
                .map(|w| {
                    Ok(WithItem {
                        expr: fold(&w.expr, env, ev)?,
                        ..w.clone()
                    })
                })
                .collect::<Result<Vec<_>, CtError>>()?,
            body: specialize_block(body, env, ev)?,
            span: *span,
        },
        Stmt::Unsafe { body, span } => Stmt::Unsafe {
            body: specialize_block(body, env, ev)?,
            span: *span,
        },
        Stmt::Scope { opts, body, span } => Stmt::Scope {
            opts: opts
                .iter()
                .map(|a| fold_arg(a, env, ev))
                .collect::<Result<Vec<_>, CtError>>()?,
            body: specialize_block(body, env, ev)?,
            span: *span,
        },
        Stmt::Spawn {
            call,
            detached,
            span,
        } => Stmt::Spawn {
            call: fold(call, env, ev)?,
            detached: *detached,
            span: *span,
        },
        Stmt::Select { cases, span } => Stmt::Select {
            cases: cases
                .iter()
                .map(|c| specialize_select_case(c, env, ev))
                .collect::<Result<Vec<_>, CtError>>()?,
            span: *span,
        },
        Stmt::Del { target, span } => Stmt::Del {
            target: fold(target, env, ev)?,
            span: *span,
        },
        Stmt::LetTuple {
            is_const,
            ty,
            target,
            value,
            span,
        } => Stmt::LetTuple {
            is_const: *is_const,
            ty: ty.clone(),
            target: target.clone(),
            value: fold(value, env, ev)?,
            span: *span,
        },
        Stmt::AssignTuple {
            target,
            value,
            span,
        } => Stmt::AssignTuple {
            target: target.clone(),
            value: fold(value, env, ev)?,
            span: *span,
        },
        // Leaf / structure-only statements: nothing to fold.
        Stmt::Cfg(_) | Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) => st.clone(),
    })
}

fn specialize_select_case(
    c: &SelectCase,
    env: &mut Env,
    ev: &mut Evaluator,
) -> Result<SelectCase, CtError> {
    let op = match &c.op {
        SelectOp::Send { chan, value } => SelectOp::Send {
            chan: fold(chan, env, ev)?,
            value: fold(value, env, ev)?,
        },
        other => other.clone(),
    };
    Ok(SelectCase {
        op,
        body: specialize_block(&c.body, env, ev)?,
        span: c.span,
    })
}

fn fold_all(es: &[Expr], env: &mut Env, ev: &mut Evaluator) -> Result<Vec<Expr>, CtError> {
    es.iter().map(|e| fold(e, env, ev)).collect()
}

fn fold_arg(a: &Arg, env: &mut Env, ev: &mut Evaluator) -> Result<Arg, CtError> {
    Ok(Arg {
        name: a.name.clone(),
        value: fold(&a.value, env, ev)?,
        spread: a.spread,
        span: a.span,
    })
}

/// Specialize one expression: if it reduces to a materializable comptime value,
/// replace it with that literal; otherwise rebuild it with its sub-expressions
/// specialized (so a runtime expression keeps its structure while its comptime
/// operands fold).
fn specialize_expr(e: &Expr, env: &mut Env, ev: &mut Evaluator) -> Result<Expr, CtError> {
    if let Ok(v) = ev.eval(e, env) {
        if let Some(lit) = materialize(&v, crate::eval::expr_span(e)) {
            return Ok(lit);
        }
    }
    crate::walk::map_child_exprs(e, &mut |c| specialize_expr(c, env, ev))
}
