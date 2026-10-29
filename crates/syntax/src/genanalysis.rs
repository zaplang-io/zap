//! Generator analysis over the AST (I#67, D-0026/D-0048).
//!
//! A `def` whose body contains a `yield` is a **generator function**: calling it
//! returns a value that auto-implements the `Iterator` protocol (D-0048). Both
//! the type checker (`crates/sema`) and AST→ZIR lowering (`crates/ir`) need to
//! agree on *which* functions are generators and on *where* the `yield`s are, so
//! the detection lives here, once, over the shared AST.
//!
//! `yield` inside a nested [`Expr::Lambda`] body is **not** attributed to the
//! enclosing function — a lambda is its own (single-expression) scope — so the
//! walkers never descend into lambda bodies.

use crate::ast::{Block, Expr, Function, Stmt};

/// The synthetic name of the compiler-generated **state struct** for a
/// generator function `fn_name` (I#67). Prefixed with `$gen:` so it can never
/// collide with a user type (user identifiers cannot contain `$` or `:`), the
/// same convention interface fat-pointer layouts use (`$iface:`).
pub fn gen_state_name(fn_name: &str) -> String {
    format!("$gen:{fn_name}")
}

/// Does this function's body contain a `yield` (making it a generator, D-0026)?
pub fn is_generator(f: &Function) -> bool {
    let mut found = false;
    each_yield_in_block(&f.body, &mut |_| found = true);
    found
}

/// Visit every `Expr::Yield` node lexically inside `f`'s body (not descending
/// into nested lambda bodies), in source order. The callback receives the whole
/// [`Expr::Yield`] node so callers can read its optional operand.
pub fn each_yield<'a>(f: &'a Function, visit: &mut dyn FnMut(&'a Expr)) {
    each_yield_in_block(&f.body, visit);
}

fn each_yield_in_block<'a>(b: &'a Block, visit: &mut dyn FnMut(&'a Expr)) {
    for st in &b.stmts {
        each_yield_in_stmt(st, visit);
    }
}

fn each_yield_in_stmt<'a>(st: &'a Stmt, visit: &mut dyn FnMut(&'a Expr)) {
    match st {
        Stmt::Let { value, .. } => each_yield_in_expr(value, visit),
        Stmt::Assign { target, value, .. } => {
            each_yield_in_expr(target, visit);
            each_yield_in_expr(value, visit);
        }
        Stmt::AugAssign { target, value, .. } => {
            each_yield_in_expr(target, visit);
            each_yield_in_expr(value, visit);
        }
        Stmt::Expr(e) => each_yield_in_expr(e, visit),
        Stmt::If {
            branches,
            else_block,
            ..
        } => {
            for (cond, body) in branches {
                each_yield_in_expr(cond, visit);
                each_yield_in_block(body, visit);
            }
            if let Some(b) = else_block {
                each_yield_in_block(b, visit);
            }
        }
        Stmt::While { cond, body, .. } => {
            each_yield_in_expr(cond, visit);
            each_yield_in_block(body, visit);
        }
        Stmt::DoWhile { body, cond, .. } => {
            each_yield_in_block(body, visit);
            each_yield_in_expr(cond, visit);
        }
        Stmt::For {
            target, iter, body, ..
        } => {
            each_yield_in_expr(target, visit);
            each_yield_in_expr(iter, visit);
            each_yield_in_block(body, visit);
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            each_yield_in_expr(scrutinee, visit);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    each_yield_in_expr(g, visit);
                }
                each_yield_in_block(&arm.body, visit);
            }
        }
        Stmt::Return { value, .. } | Stmt::Raise { value, .. } => {
            if let Some(e) = value {
                each_yield_in_expr(e, visit);
            }
        }
        Stmt::Assert { cond, msg, .. } => {
            each_yield_in_expr(cond, visit);
            if let Some(m) = msg {
                each_yield_in_expr(m, visit);
            }
        }
        Stmt::Try {
            body,
            excepts,
            finally,
            ..
        } => {
            each_yield_in_block(body, visit);
            for ex in excepts {
                each_yield_in_block(&ex.body, visit);
            }
            if let Some(f) = finally {
                each_yield_in_block(f, visit);
            }
        }
        Stmt::With { items, body, .. } => {
            for it in items {
                each_yield_in_expr(&it.expr, visit);
            }
            each_yield_in_block(body, visit);
        }
        Stmt::Spawn { call, .. } => each_yield_in_expr(call, visit),
        Stmt::Scope { opts, body, .. } => {
            for a in opts {
                each_yield_in_expr(&a.value, visit);
            }
            each_yield_in_block(body, visit);
        }
        Stmt::LetTuple { value, .. } | Stmt::AssignTuple { value, .. } => {
            each_yield_in_expr(value, visit)
        }
        // A `when target.*` block is normally cfg-resolved away before generator
        // analysis runs; if one is still present, visit every arm (a `yield`
        // inside any arm still belongs to this function).
        Stmt::Cfg(block) => {
            for arm in &block.arms {
                each_yield_in_block(&arm.body, visit);
            }
            if let Some(b) = &block.else_body {
                each_yield_in_block(b, visit);
            }
        }
        Stmt::Break(_) | Stmt::Continue(_) => {}
    }
}

fn each_yield_in_expr<'a>(e: &'a Expr, visit: &mut dyn FnMut(&'a Expr)) {
    match e {
        Expr::Yield { value, .. } => {
            // Visit the yield itself; also descend into its operand so a nested
            // `yield (yield x)` (pathological, but exhaustive) is still seen.
            visit(e);
            if let Some(v) = value {
                each_yield_in_expr(v, visit);
            }
        }
        // Leaves.
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Imaginary(..)
        | Expr::Str { .. }
        | Expr::Bool(..)
        | Expr::None(..)
        | Expr::Name(..)
        | Expr::RegexMatch { .. }
        | Expr::RegexSubst { .. } => {}
        // A lambda is its own scope — do not attribute its `yield`s here.
        Expr::Lambda { .. } => {}
        Expr::Tuple(es, _) | Expr::List(es, _) | Expr::Set(es, _) => {
            for e in es {
                each_yield_in_expr(e, visit);
            }
        }
        Expr::Dict(pairs, _) => {
            for (k, v) in pairs {
                each_yield_in_expr(k, visit);
                each_yield_in_expr(v, visit);
            }
        }
        Expr::ListComp {
            elem, iter, cond, ..
        }
        | Expr::SetComp {
            elem, iter, cond, ..
        } => {
            each_yield_in_expr(elem, visit);
            each_yield_in_expr(iter, visit);
            if let Some(c) = cond {
                each_yield_in_expr(c, visit);
            }
        }
        Expr::DictComp {
            key,
            value,
            iter,
            cond,
            ..
        } => {
            each_yield_in_expr(key, visit);
            each_yield_in_expr(value, visit);
            each_yield_in_expr(iter, visit);
            if let Some(c) = cond {
                each_yield_in_expr(c, visit);
            }
        }
        Expr::Unary { expr, .. } => each_yield_in_expr(expr, visit),
        Expr::Binary { lhs, rhs, .. } => {
            each_yield_in_expr(lhs, visit);
            each_yield_in_expr(rhs, visit);
        }
        Expr::Compare { first, rest, .. } => {
            each_yield_in_expr(first, visit);
            for (_, operand) in rest {
                each_yield_in_expr(operand, visit);
            }
        }
        Expr::Call { callee, args, .. } => {
            each_yield_in_expr(callee, visit);
            for a in args {
                each_yield_in_expr(&a.value, visit);
            }
        }
        Expr::Index { base, index, .. } => {
            each_yield_in_expr(base, visit);
            each_yield_in_expr(index, visit);
        }
        Expr::Slice {
            base, lo, hi, step, ..
        } => {
            each_yield_in_expr(base, visit);
            for part in [lo, hi, step].into_iter().flatten() {
                each_yield_in_expr(part, visit);
            }
        }
        Expr::Attr { base, .. } => each_yield_in_expr(base, visit),
        Expr::ChanRecv { chan, .. } => each_yield_in_expr(chan, visit),
        Expr::IfElse {
            cond, then, els, ..
        } => {
            each_yield_in_expr(cond, visit);
            each_yield_in_expr(then, visit);
            each_yield_in_expr(els, visit);
        }
    }
}
