//! The static comptime-purity **reject-lint** (#1254, locked decision 2a).
//!
//! Layer (a) of enforcement: before the interpreter runs, scan a function that
//! is used in a comptime context for any **mention** of an impure leaf (the
//! shared [`crate::leaf`] table), and reject it at the definition site with a
//! clean message. This moves common errors earlier; it is **not** the soundness
//! authority — the bounded interpreter (layer b) is.
//!
//! Design (locked):
//! - **Fail-closed / stricter (Philosophy 2):** reject if an impure leaf is
//!   reachable *at all* — the scan is a **join over every branch**, including a
//!   provably-dead one. A comptime-callable function may not even *mention*
//!   I/O / FFI, so the lint walks the whole AST without evaluating conditions.
//! - **Defer on the unresolved:** an unknown / dynamic / effect-polymorphic
//!   callee is **not** rejected ([`crate::leaf::classify_call`] returns `None`);
//!   the interpreter catches it. The lint only ever rejects, never accepts.
//! - **Best-effort transitive propagation (leaf-only first cut):** a call to a
//!   known comptime-callable function is followed into its body (cycle-guarded),
//!   so an impure leaf one or more call levels down is still reported.
//!
//! `spawn`/channel/`select` constructs and `dict`/`set`-literal iteration
//! (`Nondet`) are detected structurally; `Ffi` and `GlobalMutation` are, in this
//! v1 cut, left to the interpreter (the lint cannot resolve them without types /
//! scope) — documented, and consistent with "leaf-only acceptable first cut".

use std::collections::HashMap;

use zapc_syntax::ast::{BinOp, Block, Expr, Function, Stmt};
use zapc_syntax::span::Span;

use crate::leaf::{classify_call, LeafKind};
use crate::CtError;

/// Lint `func` (and, transitively, the comptime-callable functions it calls,
/// drawn from `others`) for impure-leaf mentions. Returns one [`CtError`] per
/// leaf site, in source order, each carrying the leaf kind and anchored at the
/// mention — empty when the function is pure as far as the lint can tell.
///
/// A clean result does **not** certify purity (the lint defers on unknown
/// callees); it only means no *statically-known* leaf is mentioned. The
/// interpreter remains the soundness authority.
pub fn lint_comptime_callable(func: &Function, others: &[Function]) -> Vec<CtError> {
    let fns: HashMap<&str, &Function> = others.iter().map(|f| (f.name.as_str(), f)).collect();
    let mut linter = Linter {
        fns,
        visiting: vec![func.name.as_str()],
        out: Vec::new(),
    };
    linter.visit_block(&func.body);
    linter.out
}

struct Linter<'a> {
    fns: HashMap<&'a str, &'a Function>,
    /// Call stack of function names, to guard against cycles in transitive
    /// propagation.
    visiting: Vec<&'a str>,
    out: Vec<CtError>,
}

impl<'a> Linter<'a> {
    fn reject(&mut self, kind: LeafKind, span: Span) {
        self.out.push(CtError::not_available(
            format!("{} in a comptime-callable function", kind.as_str()),
            span,
        ));
    }

    fn visit_block(&mut self, b: &Block) {
        for st in &b.stmts {
            self.visit_stmt(st);
        }
    }

    fn visit_stmt(&mut self, st: &Stmt) {
        match st {
            // Concurrency constructs are impure leaves outright.
            Stmt::Spawn { call, span, .. } => {
                self.reject(LeafKind::Spawn, *span);
                self.visit_expr(call);
            }
            Stmt::Select { cases, span } => {
                self.reject(LeafKind::Channel, *span);
                for c in cases {
                    self.visit_block(&c.body);
                }
            }
            // `for x in <dict/set literal>` is nondeterministic iteration.
            Stmt::For {
                iter, body, span, ..
            } => {
                if matches!(iter, Expr::Dict(..) | Expr::Set(..)) {
                    self.reject(LeafKind::Nondet, *span);
                }
                self.visit_expr(iter);
                self.visit_block(body);
            }
            // Recurse into every branch — including a provably-dead one — so a
            // leaf mentioned anywhere is rejected (join over branches).
            Stmt::If {
                branches,
                else_block,
                ..
            } => {
                for (cond, body) in branches {
                    self.visit_expr(cond);
                    self.visit_block(body);
                }
                if let Some(b) = else_block {
                    self.visit_block(b);
                }
            }
            Stmt::While { cond, body, .. } | Stmt::DoWhile { body, cond, .. } => {
                self.visit_expr(cond);
                self.visit_block(body);
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                self.visit_expr(scrutinee);
                for a in arms {
                    if let Some(g) = &a.guard {
                        self.visit_expr(g);
                    }
                    self.visit_block(&a.body);
                }
            }
            Stmt::Try {
                body,
                excepts,
                finally,
                ..
            } => {
                self.visit_block(body);
                for c in excepts {
                    self.visit_block(&c.body);
                }
                if let Some(f) = finally {
                    self.visit_block(f);
                }
            }
            Stmt::With { items, body, .. } => {
                for w in items {
                    self.visit_expr(&w.expr);
                }
                self.visit_block(body);
            }
            Stmt::Scope { body, .. } | Stmt::Unsafe { body, .. } => self.visit_block(body),
            Stmt::Let { value, .. } => self.visit_expr(value),
            Stmt::LetTuple { value, .. } | Stmt::AssignTuple { value, .. } => {
                self.visit_expr(value)
            }
            Stmt::Assign { target, value, .. } | Stmt::AugAssign { target, value, .. } => {
                self.visit_expr(target);
                self.visit_expr(value);
            }
            Stmt::AssignChain { targets, value, .. } => {
                for t in targets {
                    self.visit_expr(t);
                }
                self.visit_expr(value);
            }
            Stmt::Return { value, .. } | Stmt::Raise { value, .. } => {
                if let Some(e) = value {
                    self.visit_expr(e);
                }
            }
            Stmt::Assert { cond, msg, .. } => {
                self.visit_expr(cond);
                if let Some(m) = msg {
                    self.visit_expr(m);
                }
            }
            Stmt::Del { target, .. } => self.visit_expr(target),
            Stmt::Expr(e) => self.visit_expr(e),
            Stmt::Cfg(_) | Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) => {}
        }
    }

    fn visit_expr(&mut self, e: &Expr) {
        match e {
            // A call: classify the callee against the shared leaf table; then
            // follow a known comptime-callable user function transitively.
            Expr::Call { callee, args, span } => {
                if let Some(kind) = classify_call(callee) {
                    self.reject(kind, *span);
                }
                self.visit_expr(callee);
                for a in args {
                    self.visit_expr(&a.value);
                }
                self.follow_call(callee);
            }
            Expr::ChanRecv { chan, span } => {
                self.reject(LeafKind::Channel, *span);
                self.visit_expr(chan);
            }
            Expr::Binary {
                op: BinOp::Send,
                lhs,
                rhs,
                span,
            } => {
                self.reject(LeafKind::Channel, *span);
                self.visit_expr(lhs);
                self.visit_expr(rhs);
            }
            _ => {
                // Generic structural recursion into child expressions.
                each_child(e, &mut |c| self.visit_expr(c));
            }
        }
    }

    /// If `callee` names a known comptime-callable function not already on the
    /// stack, scan its body too (transitive, cycle-guarded).
    fn follow_call(&mut self, callee: &Expr) {
        if let Expr::Name(name, _) = callee {
            if !self.visiting.contains(&name.as_str()) {
                if let Some(f) = self.fns.get(name.as_str()).copied() {
                    self.visiting.push(f.name.as_str());
                    self.visit_block(&f.body);
                    self.visiting.pop();
                }
            }
        }
    }
}

/// Visit each direct child expression of `e` (read-only traversal), reusing the
/// shared child-mapping walk so the variant coverage stays in one place.
fn each_child(e: &Expr, f: &mut impl FnMut(&Expr)) {
    // `map_child_exprs` rebuilds; drive it purely for traversal and discard.
    let _ = crate::walk::map_child_exprs::<_>(e, &mut |c| {
        f(c);
        Ok::<Expr, CtError>(c.clone())
    });
}
