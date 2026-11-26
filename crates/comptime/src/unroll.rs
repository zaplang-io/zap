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
        Value::Type(..)
        | Value::Field(_)
        | Value::Variant(_)
        | Value::Attr(_)
        | Value::AttrArg(_)
        | Value::Kind(_)
        | Value::Span(_) => return None,
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

/// Specialize `body` for a single bound `item` (#1374): bind `loop_var` to
/// `item` and run the comptime specializer, returning the specialized [`Block`]
/// on its own (rather than flattening into a statement list like [`unroll`]).
/// Match-arm generation uses this to keep each variant's arm body separate while
/// still unrolling any inner `comptime for` and resolving `comptime <expr>`
/// inside it. Runtime constructs in the body are left untouched.
pub fn specialize_one(
    loop_var: &str,
    item: Value,
    payload: Option<&str>,
    body: &Block,
    base_env: &Env,
    ev: &mut Evaluator,
) -> Result<Block, CtError> {
    let mut env = base_env.clone();
    env.define(loop_var, item);
    // Record the arm's payload handle (#1374) so `reflect.field(<payload>, i)`
    // inside the body resolves to the i-th payload capture the arm pattern binds
    // (`$cvp{i}`), rather than being left as a struct field access.
    if let Some(p) = payload {
        env.define(PAYLOAD_HANDLE_KEY, Value::str(p));
    }
    specialize_block(body, &mut env, ev)
}

/// Env key under which [`specialize_one`] stashes the current comptime arm's
/// `as <payload>` binding name (#1374). The specializer reads it to rewrite
/// `reflect.field(<payload>, i)` into the arm pattern's i-th payload capture.
const PAYLOAD_HANDLE_KEY: &str = "$arm_payload";

/// The canonical name the arm pattern binds a variant's i-th payload field to
/// (#1374). Deterministic and arm-local (each match arm is its own scope), so no
/// gensym counter is needed.
pub fn payload_capture_name(index: usize) -> String {
    format!("$cvp{index}")
}

/// If `e` is `reflect.field(<payload>, <comptime index>)` where `<payload>` is
/// the current arm's payload handle (#1374), return the comptime-folded field
/// index. The specializer rewrites such a call to the arm's payload capture.
fn reflect_field_payload_index(
    e: &Expr,
    env: &mut Env,
    ev: &mut Evaluator,
) -> Result<Option<usize>, CtError> {
    let Expr::Call { callee, args, .. } = e else {
        return Ok(None);
    };
    let Expr::Attr { base, name, .. } = &**callee else {
        return Ok(None);
    };
    if name != "field" || !matches!(&**base, Expr::Name(n, _) if n == "reflect") || args.len() != 2
    {
        return Ok(None);
    }
    let Some(Value::Str(payload)) = env.lookup(PAYLOAD_HANDLE_KEY).cloned() else {
        return Ok(None);
    };
    if !matches!(&args[0].value, Expr::Name(n, _) if *n == *payload) {
        return Ok(None);
    }
    match ev.eval(&args[1].value, env)? {
        Value::Int(i) if i >= 0 => Ok(Some(i as usize)),
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// specialization (comptime-constant folding over the body)
// ---------------------------------------------------------------------------

fn specialize_block(b: &Block, env: &mut Env, ev: &mut Evaluator) -> Result<Block, CtError> {
    let mut stmts = Vec::with_capacity(b.stmts.len());
    for st in &b.stmts {
        // A comptime-constant `if` is resolved at specialization time: only the
        // taken branch is kept, and the untaken branch(es) are dropped entirely
        // (#1330). This is what makes a guarded `comptime.error` work — the abort
        // in a branch whose condition is comptime-false must *not* fire, and a
        // `.kind`/`f.index`-style dispatch must not emit the dead arms. An `if`
        // whose condition is not a comptime constant is left as an ordinary
        // runtime `if` (its branches still specialized).
        if let Stmt::If {
            branches,
            else_block,
            ..
        } = st
        {
            if let Some(mut taken) = prune_comptime_if(branches, else_block, env, ev)? {
                stmts.append(&mut taken);
                continue;
            }
        }
        // A nested `comptime for <v> in <iter>:` whose iterable is comptime-known
        // here is unrolled in place — binding `<v>` per item and splicing the
        // specialized body (#1374). This is what lets a comptime arm's inner
        // `comptime for f in v.fields:` unroll once `v` is a concrete variant
        // record. An iterable that is not a comptime list (e.g. a still-abstract
        // `reflect.type_info(T).fields` the outer mono pass unrolls) is left as a
        // `comptime for` for that later pass.
        if let Stmt::ComptimeFor {
            target: Expr::Name(lv, _),
            iter,
            body,
            ..
        } = st
        {
            let mut probe = env.clone();
            if matches!(ev.eval(iter, &mut probe), Ok(Value::List(_))) {
                let mut unrolled = unroll(lv, iter, body, env, ev)?;
                stmts.append(&mut unrolled);
                continue;
            }
        }
        stmts.push(specialize_stmt(st, env, ev)?);
    }
    Ok(Block {
        stmts,
        span: b.span,
    })
}

/// Resolve a comptime-constant `if`/`elif`/`else` chain: evaluate each branch
/// condition in order; the first that is a comptime `true` yields that branch's
/// specialized statements, a comptime `false` moves on, and exhausting them
/// yields the `else` branch (or nothing). Returns `None` — leaving an ordinary
/// runtime `if` — as soon as a condition is **not** a comptime boolean (so a
/// genuine runtime `if` in a comptime-for body is untouched). (#1330)
fn prune_comptime_if(
    branches: &[(Expr, Block)],
    else_block: &Option<Block>,
    env: &mut Env,
    ev: &mut Evaluator,
) -> Result<Option<Vec<Stmt>>, CtError> {
    for (cond, body) in branches {
        match ev.eval(cond, env) {
            Ok(Value::Bool(true)) => return Ok(Some(specialize_block(body, env, ev)?.stmts)),
            Ok(Value::Bool(false)) => continue,
            // Not a comptime boolean → this is a runtime `if`; leave it whole.
            _ => return Ok(None),
        }
    }
    Ok(Some(match else_block {
        Some(b) => specialize_block(b, env, ev)?.stmts,
        None => Vec::new(),
    }))
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
        Stmt::ComptimeFor {
            target,
            iter,
            body,
            span,
        } => Stmt::ComptimeFor {
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
    // `comptime.error(...)` is the library-author abort primitive (#1256, #1330):
    // its evaluation *always* fails (a `UserError` `CtError`), and that failure is
    // the intended compile error. Propagate it, rather than letting it fall
    // through the `if let Ok(..)` below — which would swallow the abort and emit
    // the call as ordinary (runtime) code, where `comptime` is an unknown name.
    if is_comptime_error_call(e) {
        ev.eval(e, env)?;
        // `comptime.error` never evaluates to a value; if that ever changes, keep
        // the call verbatim rather than silently dropping it.
        return Ok(e.clone());
    }
    // #1374: `reflect.field(<payload>, i)` in a comptime-generated variant arm
    // rewrites to the arm pattern's i-th payload capture (a runtime binding),
    // with the index comptime-folded.
    if let Some(i) = reflect_field_payload_index(e, env, ev)? {
        return Ok(Expr::Name(
            payload_capture_name(i),
            crate::eval::expr_span(e),
        ));
    }
    // #1575: an explicit type-argument position whose argument is a comptime
    // type — `gen[f.ty.elem](...)` / `gen[f.ty](...)` / `gen[f.ty.value](...)`.
    // The subscript folds to a `Value::Type` carrying the use-site declared
    // `AstType`; rewrite it to an `Expr::TypeArg` (which `mono::expr_to_type`
    // unwraps) so the enclosing generic call resolves against the concrete type
    // after specialization. Without this the subscript keeps its comptime `f`
    // reference, and sema reads the callee as an unknown value being indexed
    // (`unknown variable 'gen'`). This is the construction/decode counterpart to
    // the method recursion an encoder uses (`x.to_...()` on a known value): a
    // decoder must name the element type, which only reflection can supply here.
    // Only a type-valued subscript is rewritten — an ordinary runtime index
    // (`list[i]`, `dict[k]`, `array[dtype]`) never folds to a `Value::Type`.
    if let Expr::Index { base, index, span } = e {
        if let Ok(Value::Type(_, Some(decl))) = ev.eval(index, env) {
            return Ok(Expr::Index {
                base: Box::new(specialize_expr(base, env, ev)?),
                index: Box::new(Expr::TypeArg {
                    ty: Box::new((*decl).clone()),
                    span: crate::eval::expr_span(index),
                }),
                span: *span,
            });
        }
    }
    if let Ok(v) = ev.eval(e, env) {
        if let Some(lit) = materialize(&v, crate::eval::expr_span(e)) {
            return Ok(lit);
        }
    }
    crate::walk::map_child_exprs(e, &mut |c| specialize_expr(c, env, ev))
}

/// `comptime.error(...)` — a call whose callee is the `error` attribute of the
/// `comptime` name. Its abort must propagate out of specialization (#1330).
fn is_comptime_error_call(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Call { callee, .. }
            if matches!(&**callee, Expr::Attr { base, name, .. }
                if name == "error" && matches!(&**base, Expr::Name(n, _) if n == "comptime"))
    )
}
