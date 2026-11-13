//! `@scoped` affine resource marker checks (#876, D-0159).
//!
//! A `@scoped` struct is a with-bound *affine* resource: each value must be
//! consumed **exactly once** — by a `with` head or by a move into another
//! `@scoped` struct's field — and is torn down deterministically at scope exit.
//! This module implements the static guarantees (the normative spec is
//! `docs/design/scoped.md`):
//!
//! 1. **Contagion** ([`Checker::close_scoped_contagion`]): a struct owning a
//!    `@scoped` field is itself `@scoped`, closed under a fixpoint.
//! 2. **Well-formedness** ([`Checker::check_scoped_well_formed`], `ZAP5127`): a
//!    `@scoped` struct (marked or contagious) MUST define both `__enter__` and
//!    `__exit__`.
//! 3. **With-head-only construction** (`ZAP5128`): a freshly produced `@scoped`
//!    value may appear only as a `with` head or as the immediate RHS of an
//!    assignment into a `@scoped` field; anything else (detached construction,
//!    `return`, store, retaining-call argument) is an error.
//! 4. **No escape** (`ZAP5129`): a `with`-bound `@scoped` value may not outlive
//!    its scope — returned, stored, or carried across a `spawn`/`scope:` boundary.
//!
//! The affine check is scoped narrowly to `@scoped`-typed values (D-0159 Q3), not
//! general linear typing; the drop-glue lowering lives in `crates/ir/src/lower.rs`.

use std::collections::HashSet;

use zapc_diag::ErrorCode;
use zapc_sema_types::Ty;
use zapc_syntax::ast::*;
use zapc_syntax::span::Span;

use super::{expr_span, Checker};

impl Checker {
    /// Run every `@scoped` check (#876, D-0159). Called after decorator
    /// collection and validation, before the main `check_module` pass.
    pub(crate) fn check_scoped(&mut self, module: &Module) {
        self.close_scoped_contagion();
        self.check_scoped_well_formed(module);
        self.check_scoped_usage(module);
    }

    /// Close `scoped_types` under contagion: a struct is `@scoped` if any field's
    /// type is itself `@scoped` (direct `Ty::Named` fields). Iterated to a
    /// fixpoint so ownership chains of arbitrary depth propagate scopedness.
    fn close_scoped_contagion(&mut self) {
        loop {
            let mut added: Vec<String> = Vec::new();
            for (name, info) in &self.structs {
                if self.scoped_types.contains(name) {
                    continue;
                }
                let owns_scoped = info
                    .fields
                    .iter()
                    .any(|t| matches!(t, Ty::Named(n) if self.scoped_types.contains(n)));
                if owns_scoped {
                    added.push(name.clone());
                }
            }
            if added.is_empty() {
                break;
            }
            for n in added {
                self.scoped_types.insert(n);
            }
        }
    }

    /// `ZAP5127`: every `@scoped` struct (explicit or contagious) must define both
    /// `__enter__` and `__exit__` (D-0069 context-manager protocol, D-0159 Q5 —
    /// even an owner made `@scoped` only by contagion hand-writes both).
    fn check_scoped_well_formed(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Struct(s) = item else { continue };
            if !self.scoped_types.contains(&s.name) {
                continue;
            }
            let (has_enter, has_exit) = self
                .structs
                .get(&s.name)
                .map(|si| (si.has("__enter__"), si.has("__exit__")))
                .unwrap_or((false, false));
            if has_enter && has_exit {
                continue;
            }
            let missing = match (has_enter, has_exit) {
                (false, false) => "both `__enter__` and `__exit__`",
                (false, true) => "`__enter__`",
                (true, false) => "`__exit__`",
                (true, true) => unreachable!(),
            };
            self.error(
                ErrorCode::ZAP5127,
                format!(
                    "@scoped struct `{}` must define {} (it is a with-bound resource \
                     and must implement the context-manager protocol; #876, D-0159)",
                    s.name, missing
                ),
                s.span,
            );
        }
    }

    /// Walk every function/method body enforcing with-head-only construction
    /// (`ZAP5128`) and no-escape (`ZAP5129`).
    fn check_scoped_usage(&mut self, module: &Module) {
        if self.scoped_types.is_empty() {
            return; // no `@scoped` types in this module — nothing to check
        }
        let empty: HashSet<String> = HashSet::new();
        for item in &module.items {
            match item {
                Item::Function(f) => self.walk_scoped_block(&f.body, &empty),
                Item::Struct(s) => {
                    for m in &s.methods {
                        self.walk_scoped_block(&m.body, &empty);
                    }
                }
                Item::Enum(e) => {
                    for m in &e.methods {
                        self.walk_scoped_block(&m.body, &empty);
                    }
                }
                _ => {}
            }
        }
    }

    fn walk_scoped_block(&mut self, b: &Block, aliases: &HashSet<String>) {
        for s in &b.stmts {
            self.walk_scoped_stmt(s, aliases);
        }
    }

    fn walk_scoped_stmt(&mut self, s: &Stmt, aliases: &HashSet<String>) {
        match s {
            // `with <head> as a:` — the head is the one legal construction site.
            Stmt::With { items, body, .. } => {
                let mut inner = aliases.clone();
                for it in items {
                    if self.producer_span(&it.expr).is_some() {
                        // Legal `with`-head producer: scan only its children for
                        // nested illegal producers / escapes, not the head itself.
                        self.scan_children(&it.expr, aliases);
                        if let Some(a) = &it.alias {
                            inner.insert(a.clone());
                        }
                    } else {
                        self.scan_expr(&it.expr, aliases);
                    }
                }
                self.walk_scoped_block(body, &inner);
            }
            // `target = value` — the one other legal construction site is a move
            // of a fresh `@scoped` value into a field/slot (ownership transfer).
            Stmt::Assign { target, value, .. } => {
                self.scan_expr(target, aliases);
                let legal_move = matches!(target, Expr::Attr { .. } | Expr::Index { .. })
                    && self.producer_span(value).is_some();
                if legal_move {
                    self.scan_children(value, aliases);
                } else {
                    self.scan_expr(value, aliases);
                    self.check_alias_escape(value, aliases);
                }
            }
            Stmt::Let { value, .. } => {
                self.scan_expr(value, aliases);
                self.check_alias_escape(value, aliases);
            }
            Stmt::AugAssign { target, value, .. } => {
                self.scan_expr(target, aliases);
                self.scan_expr(value, aliases);
                self.check_alias_escape(value, aliases);
            }
            Stmt::AssignChain { targets, value, .. } => {
                for t in targets {
                    self.scan_expr(t, aliases);
                }
                self.scan_expr(value, aliases);
                self.check_alias_escape(value, aliases);
            }
            Stmt::LetTuple { value, .. } | Stmt::AssignTuple { value, .. } => {
                self.scan_expr(value, aliases);
                self.check_alias_escape(value, aliases);
            }
            Stmt::Return { value: Some(v), .. } => {
                self.scan_expr(v, aliases);
                self.check_alias_escape(v, aliases);
            }
            Stmt::Return { value: None, .. }
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Pass(_) => {}
            Stmt::Expr(e) => self.scan_expr(e, aliases),
            Stmt::Spawn { call, .. } => self.scan_expr(call, aliases),
            Stmt::Scope { opts, body, .. } => {
                for o in opts {
                    self.scan_expr(&o.value, aliases);
                }
                // Aliases stay in scope: a `@scoped` value referenced inside a
                // `scope:` is fine as a receiver, but passing it into a `spawn`
                // there is caught by the call-argument escape check in `scan_expr`.
                self.walk_scoped_block(body, aliases);
            }
            Stmt::If {
                branches,
                else_block,
                ..
            } => {
                for (c, b) in branches {
                    self.scan_expr(c, aliases);
                    self.walk_scoped_block(b, aliases);
                }
                if let Some(b) = else_block {
                    self.walk_scoped_block(b, aliases);
                }
            }
            Stmt::While { cond, body, .. } => {
                self.scan_expr(cond, aliases);
                self.walk_scoped_block(body, aliases);
            }
            Stmt::DoWhile { body, cond, .. } => {
                self.walk_scoped_block(body, aliases);
                self.scan_expr(cond, aliases);
            }
            Stmt::For {
                target, iter, body, ..
            } => {
                self.scan_expr(target, aliases);
                self.scan_expr(iter, aliases);
                self.walk_scoped_block(body, aliases);
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                self.scan_expr(scrutinee, aliases);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.scan_expr(g, aliases);
                    }
                    self.walk_scoped_block(&arm.body, aliases);
                }
            }
            Stmt::Try {
                body,
                excepts,
                finally,
                ..
            } => {
                self.walk_scoped_block(body, aliases);
                for ex in excepts {
                    self.walk_scoped_block(&ex.body, aliases);
                }
                if let Some(f) = finally {
                    self.walk_scoped_block(f, aliases);
                }
            }
            Stmt::Raise { value, cause, .. } => {
                if let Some(v) = value {
                    self.scan_expr(v, aliases);
                }
                if let Some(c) = cause {
                    self.scan_expr(c, aliases);
                }
            }
            Stmt::Assert { cond, msg, .. } => {
                self.scan_expr(cond, aliases);
                if let Some(m) = msg {
                    self.scan_expr(m, aliases);
                }
            }
            Stmt::Del { target, .. } => self.scan_expr(target, aliases),
            Stmt::Cfg(_) => {}
        }
    }

    /// Deep-scan an expression for illegal `@scoped` producers (`ZAP5128`) and
    /// `@scoped` aliases escaping via a call argument (`ZAP5129`).
    fn scan_expr(&mut self, e: &Expr, aliases: &HashSet<String>) {
        if let Some(span) = self.producer_span(e) {
            self.error(
                ErrorCode::ZAP5128,
                "a `@scoped` value must be bound by a `with` block or moved into a \
                 `@scoped` field; it may not be constructed detached, returned, or \
                 stored (#876, D-0159)"
                    .to_string(),
                span,
            );
        }
        // A `@scoped` alias handed as an argument to any call (including the call
        // under a `spawn`) flows out of its `with` scope — a retaining-call /
        // cross-boundary escape.
        if let Expr::Call { args, .. } = e {
            for a in args {
                if alias_outflow(&a.value, aliases) {
                    self.escape_error(expr_span(&a.value));
                }
            }
        }
        self.scan_children(e, aliases);
    }

    /// Scan only the immediate children of `e` (used when `e` itself is a legal
    /// producer site whose head must not be re-reported).
    fn scan_children(&mut self, e: &Expr, aliases: &HashSet<String>) {
        let mut kids: Vec<&Expr> = Vec::new();
        child_exprs(e, &mut kids);
        for k in kids {
            self.scan_expr(k, aliases);
        }
    }

    /// Report `ZAP5129` if `e` lets a `with`-bound `@scoped` alias flow out by
    /// value (a bare alias, or one inside a tuple/list/set display).
    fn check_alias_escape(&mut self, e: &Expr, aliases: &HashSet<String>) {
        if alias_outflow(e, aliases) {
            self.escape_error(expr_span(e));
        }
    }

    fn escape_error(&mut self, span: Span) {
        self.error(
            ErrorCode::ZAP5129,
            "a `@scoped` value may not escape its `with` scope — it cannot be \
             returned, stored in a longer-lived slot, or carried across a \
             `spawn`/`scope:` boundary (#876, D-0159)"
                .to_string(),
            span,
        );
    }

    /// If `e` is a constructor call of a `@scoped` struct (`Arena()`), its span;
    /// else `None`. This is the one way a `@scoped` *value* is produced.
    fn producer_span(&self, e: &Expr) -> Option<Span> {
        if let Expr::Call { callee, span, .. } = e {
            if let Expr::Name(n, _) = &**callee {
                if self.scoped_types.contains(n) {
                    return Some(*span);
                }
            }
        }
        None
    }
}

/// Does `e` let a `@scoped` alias flow out *by value*? True for a bare alias name
/// or an alias inside a transparent display (tuple/list/set/`*`-splat). It does
/// **not** descend into a call/attribute/index, so a method-receiver use
/// (`a.alloc(...)`) or a field read (`a.x`) is correctly not an outflow.
fn alias_outflow(e: &Expr, aliases: &HashSet<String>) -> bool {
    match e {
        Expr::Name(n, _) => aliases.contains(n),
        Expr::Tuple(xs, _) | Expr::List(xs, _) | Expr::Set(xs, _) => {
            xs.iter().any(|x| alias_outflow(x, aliases))
        }
        Expr::Starred { value, .. } => alias_outflow(value, aliases),
        _ => false,
    }
}

/// Push every immediate child expression of `e` into `out` (a shallow,
/// exhaustive visitor; `scan_expr` drives the recursion).
fn child_exprs<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match e {
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Imaginary(..)
        | Expr::Bool(..)
        | Expr::None(_)
        | Expr::Name(..)
        | Expr::RegexMatch { .. }
        | Expr::RegexSubst { .. }
        | Expr::Lambda { .. } => {}
        Expr::Str { fstring_fields, .. } => {
            for f in fstring_fields {
                out.push(f);
            }
        }
        Expr::Tuple(xs, _) | Expr::List(xs, _) | Expr::Set(xs, _) => {
            for x in xs {
                out.push(x);
            }
        }
        Expr::Dict(items, _) => {
            for it in items {
                match it {
                    DictItem::Pair(k, v) => {
                        out.push(k);
                        out.push(v);
                    }
                    DictItem::Spread(v) => out.push(v),
                }
            }
        }
        Expr::Starred { value, .. } => out.push(value),
        Expr::ListComp { elem, clauses, .. } | Expr::SetComp { elem, clauses, .. } => {
            out.push(elem);
            push_clauses(clauses, out);
        }
        Expr::GenExp { elem, clauses, .. } => {
            out.push(elem);
            push_clauses(clauses, out);
        }
        Expr::DictComp {
            key,
            value,
            clauses,
            ..
        } => {
            out.push(key);
            out.push(value);
            push_clauses(clauses, out);
        }
        Expr::Unary { expr, .. } => out.push(expr),
        Expr::Binary { lhs, rhs, .. } => {
            out.push(lhs);
            out.push(rhs);
        }
        Expr::Compare { first, rest, .. } => {
            out.push(first);
            for (_, e) in rest {
                out.push(e);
            }
        }
        Expr::Call { callee, args, .. } => {
            out.push(callee);
            for a in args {
                out.push(&a.value);
            }
        }
        Expr::Index { base, index, .. } => {
            out.push(base);
            out.push(index);
        }
        Expr::Slice {
            base, lo, hi, step, ..
        } => {
            out.push(base);
            if let Some(x) = lo {
                out.push(x);
            }
            if let Some(x) = hi {
                out.push(x);
            }
            if let Some(x) = step {
                out.push(x);
            }
        }
        Expr::Attr { base, .. } => out.push(base),
        Expr::ChanRecv { chan, .. } => out.push(chan),
        Expr::Yield { value, .. } => {
            if let Some(v) = value {
                out.push(v);
            }
        }
        Expr::IfElse {
            cond, then, els, ..
        } => {
            out.push(cond);
            out.push(then);
            out.push(els);
        }
        Expr::Named { value, .. } => out.push(value),
    }
}

fn push_clauses<'a>(clauses: &'a [CompClause], out: &mut Vec<&'a Expr>) {
    for c in clauses {
        out.push(&c.iter);
        for f in &c.ifs {
            out.push(f);
        }
    }
}
