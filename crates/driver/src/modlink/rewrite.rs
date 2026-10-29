//! The module-qualification rewrite (I#283/D-0081).
//!
//! Given one parsed module and facts about the modules it imports, this renames
//! the module's top-level symbols to their mangled form and rewrites every
//! reference — bare references to the module's own symbols and qualified
//! cross-module accesses (`alias.member`, `alias.Type`, `alias.Enum.Variant`
//! patterns) — to the mangled target symbol, enforcing `_`-privacy. The result
//! is an item list that merges into the single program module.
//!
//! Local shadowing is handled with a per-callable **flat** set of value
//! bindings ([`collect_bindings`]): a name that is bound anywhere in a callable
//! is treated as a local throughout it, so it never mangles to a module symbol.
//! This is a deliberate over-approximation (Stage A); the only programs it
//! misreads are those that use a module symbol *and* a local of the very same
//! name in one function, which is already shadowing-confusing code.

use std::collections::{HashMap, HashSet};

use zapc_diag::ErrorCode;
use zapc_syntax::ast::{
    BindTarget, Block, EnumDef, Expr, Function, InterfaceDef, Item, Pattern, Stmt, StructDef, Type,
    TypeParam,
};
use zapc_syntax::span::Span;

use super::{ModDiag, ModuleFacts};

pub(super) struct Rewriter<'a> {
    /// Mangle prefix for this module's own symbols (empty for the entry module).
    pub prefix: &'a str,
    /// This module's own top-level symbol names.
    pub own: &'a HashSet<String>,
    /// `import alias -> target module facts`.
    pub aliases: &'a HashMap<String, &'a ModuleFacts>,
    pub file: &'a str,
    pub diags: &'a mut Vec<ModDiag>,
}

impl Rewriter<'_> {
    fn diag(&mut self, code: ErrorCode, message: String, span: Span) {
        self.diags.push(ModDiag {
            code,
            message,
            span,
            file: self.file.to_string(),
        });
    }

    fn mangle_own(&self, name: &str) -> Option<String> {
        (!self.prefix.is_empty() && self.own.contains(name))
            .then(|| format!("{}{name}", self.prefix))
    }

    // --- items -------------------------------------------------------------

    pub(super) fn item(&mut self, it: &mut Item) {
        match it {
            Item::Function(f) => self.function(f, &HashSet::new()),
            Item::Struct(s) => self.struct_def(s),
            Item::Enum(e) => self.enum_def(e),
            Item::Interface(i) => self.interface_def(i),
            // Annotation/decorator *declarations* are not mangled in Stage A, but
            // any statement/field exprs they carry still need reference rewriting.
            Item::Annotation(a) => {
                for field in &mut a.fields {
                    let tl = HashSet::new();
                    self.ty(&mut field.ty, &tl);
                    if let Some(d) = &mut field.default {
                        self.expr(d, &HashSet::new());
                    }
                }
            }
            Item::Decorator(d) => {
                let locals = decorator_bindings(d);
                let tl = HashSet::new();
                for p in &mut d.params {
                    if let Some(t) = &mut p.ty {
                        self.ty(t, &tl);
                    }
                }
                self.block(&mut d.body, &locals, &tl);
            }
            // Top-level statements (entry module only — enforced earlier).
            Item::Stmt(_) => unreachable!("entry statements handled via merge list"),
            Item::Import(_) => {}
            Item::Cfg(_) => unreachable!("cfg resolved before linking"),
        }
    }

    fn rename_decl(&self, name: &mut String) {
        if let Some(m) = self.mangle_own(name) {
            *name = m;
        }
    }

    fn function(&mut self, f: &mut Function, extra_tlocals: &HashSet<String>) {
        self.rename_decl(&mut f.name);
        let tl = generics_set(&f.generics, extra_tlocals);
        let locals = collect_bindings_fn(f);
        for p in &mut f.params {
            if let Some(t) = &mut p.ty {
                self.ty(t, &tl);
            }
            if let Some(d) = &mut p.default {
                self.expr(d, &locals);
            }
        }
        if let Some(r) = &mut f.ret {
            self.ty(r, &tl);
        }
        self.block(&mut f.body, &locals, &tl);
    }

    fn struct_def(&mut self, s: &mut StructDef) {
        self.rename_decl(&mut s.name);
        let tl = generics_set(&s.generics, &HashSet::new());
        for field in &mut s.fields {
            self.ty(&mut field.ty, &tl);
            if let Some(d) = &mut field.default {
                self.expr(d, &HashSet::new());
            }
        }
        for m in &mut s.methods {
            self.function(m, &tl);
        }
    }

    fn enum_def(&mut self, e: &mut EnumDef) {
        self.rename_decl(&mut e.name);
        let tl = generics_set(&e.generics, &HashSet::new());
        for v in &mut e.variants {
            for vf in &mut v.fields {
                self.ty(&mut vf.ty, &tl);
            }
            if let Some(val) = &mut v.value {
                self.expr(val, &HashSet::new());
            }
        }
        for m in &mut e.methods {
            self.function(m, &tl);
        }
    }

    fn interface_def(&mut self, i: &mut InterfaceDef) {
        self.rename_decl(&mut i.name);
        let tl = generics_set(&i.generics, &HashSet::new());
        for sig in &mut i.methods {
            for p in &mut sig.params {
                if let Some(t) = &mut p.ty {
                    self.ty(t, &tl);
                }
            }
            if let Some(r) = &mut sig.ret {
                self.ty(r, &tl);
            }
        }
    }

    // --- statements & blocks ----------------------------------------------

    fn block(&mut self, b: &mut Block, locals: &HashSet<String>, tl: &HashSet<String>) {
        for st in &mut b.stmts {
            self.stmt(st, locals, tl);
        }
    }

    fn stmt(&mut self, st: &mut Stmt, locals: &HashSet<String>, tl: &HashSet<String>) {
        match st {
            Stmt::Let { ty, value, .. } => {
                if let Some(t) = ty {
                    self.ty(t, tl);
                }
                self.expr(value, locals);
            }
            Stmt::LetTuple { ty, value, .. } => {
                if let Some(t) = ty {
                    self.ty(t, tl);
                }
                self.expr(value, locals);
            }
            Stmt::Assign { target, value, .. } => {
                self.expr(target, locals);
                self.expr(value, locals);
            }
            Stmt::AugAssign { target, value, .. } => {
                self.expr(target, locals);
                self.expr(value, locals);
            }
            Stmt::Expr(e) => self.expr(e, locals),
            Stmt::If {
                branches,
                else_block,
                ..
            } => {
                for (c, body) in branches {
                    self.expr(c, locals);
                    self.block(body, locals, tl);
                }
                if let Some(b) = else_block {
                    self.block(b, locals, tl);
                }
            }
            Stmt::While { cond, body, .. } => {
                self.expr(cond, locals);
                self.block(body, locals, tl);
            }
            Stmt::DoWhile { body, cond, .. } => {
                self.block(body, locals, tl);
                self.expr(cond, locals);
            }
            Stmt::For {
                target, iter, body, ..
            } => {
                self.expr(target, locals);
                self.expr(iter, locals);
                self.block(body, locals, tl);
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee, locals);
                for arm in arms {
                    self.pattern(&mut arm.pattern, locals);
                    if let Some(g) = &mut arm.guard {
                        self.expr(g, locals);
                    }
                    self.block(&mut arm.body, locals, tl);
                }
            }
            Stmt::Return { value, .. } | Stmt::Raise { value, .. } => {
                if let Some(v) = value {
                    self.expr(v, locals);
                }
            }
            Stmt::Assert { cond, msg, .. } => {
                self.expr(cond, locals);
                if let Some(m) = msg {
                    self.expr(m, locals);
                }
            }
            Stmt::Try {
                body,
                excepts,
                finally,
                ..
            } => {
                self.block(body, locals, tl);
                for ex in excepts {
                    if let Some(t) = &mut ex.ty {
                        self.ty(t, tl);
                    }
                    self.block(&mut ex.body, locals, tl);
                }
                if let Some(f) = finally {
                    self.block(f, locals, tl);
                }
            }
            Stmt::With { items, body, .. } => {
                for item in items {
                    self.expr(&mut item.expr, locals);
                }
                self.block(body, locals, tl);
            }
            Stmt::Spawn { call, .. } => self.expr(call, locals),
            Stmt::Scope { opts, body, .. } => {
                for a in opts {
                    self.expr(&mut a.value, locals);
                }
                self.block(body, locals, tl);
            }
            Stmt::AssignTuple { target, value, .. } => {
                self.assign_target(target, locals);
                self.expr(value, locals);
            }
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::Cfg(_) => unreachable!("cfg resolved before linking"),
        }
    }

    /// Walk the lvalue expressions inside a tuple-assignment target.
    fn assign_target(&mut self, t: &mut BindTarget, locals: &HashSet<String>) {
        match t {
            BindTarget::Elem(e) => self.expr(e, locals),
            BindTarget::Rest(inner, _) => self.assign_target(inner, locals),
            BindTarget::Tuple(parts, _) => {
                for p in parts {
                    self.assign_target(p, locals);
                }
            }
        }
    }

    // --- expressions -------------------------------------------------------

    fn expr(&mut self, e: &mut Expr, locals: &HashSet<String>) {
        match e {
            Expr::Name(n, span) => {
                if locals.contains(n) {
                    return;
                }
                if let Some(m) = self.mangle_own(n) {
                    *n = m;
                } else if self.aliases.contains_key(n) {
                    // A bare module alias is not a value.
                    self.diag(
                        ErrorCode::ZAP3015,
                        format!("`{n}` is an imported module, not a value"),
                        *span,
                    );
                }
            }
            Expr::Attr { base, name, span } => {
                // Cross-module qualified access `alias.member`. Decide before
                // mutating `*e` so the match borrows are released first.
                let module_access = match base.as_ref() {
                    Expr::Name(a, _) if !locals.contains(a) => self
                        .aliases
                        .get(a)
                        .copied()
                        .map(|f| (f, name.clone(), *span)),
                    _ => None,
                };
                if let Some((facts, member, sp)) = module_access {
                    self.resolve_member(e, facts, member, sp);
                } else {
                    self.expr(base, locals);
                }
            }
            Expr::Call { callee, args, .. } => {
                self.expr(callee, locals);
                for a in args {
                    self.expr(&mut a.value, locals);
                }
            }
            Expr::Index { base, index, .. } => {
                self.expr(base, locals);
                self.expr(index, locals);
            }
            Expr::Slice {
                base, lo, hi, step, ..
            } => {
                self.expr(base, locals);
                for part in [lo, hi, step].into_iter().flatten() {
                    self.expr(part, locals);
                }
            }
            Expr::Unary { expr, .. } => self.expr(expr, locals),
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs, locals);
                self.expr(rhs, locals);
            }
            Expr::Compare { first, rest, .. } => {
                self.expr(first, locals);
                for (_, operand) in rest.iter_mut() {
                    self.expr(operand, locals);
                }
            }
            Expr::Tuple(xs, _) | Expr::List(xs, _) | Expr::Set(xs, _) => {
                for x in xs {
                    self.expr(x, locals);
                }
            }
            Expr::Dict(kvs, _) => {
                for (k, v) in kvs {
                    self.expr(k, locals);
                    self.expr(v, locals);
                }
            }
            Expr::ListComp {
                elem, iter, cond, ..
            }
            | Expr::SetComp {
                elem, iter, cond, ..
            } => {
                self.expr(elem, locals);
                self.expr(iter, locals);
                if let Some(c) = cond {
                    self.expr(c, locals);
                }
            }
            Expr::DictComp {
                key,
                value,
                iter,
                cond,
                ..
            } => {
                self.expr(key, locals);
                self.expr(value, locals);
                self.expr(iter, locals);
                if let Some(c) = cond {
                    self.expr(c, locals);
                }
            }
            Expr::Lambda { body, .. } => self.expr(body, locals),
            Expr::RegexMatch { subject, .. } | Expr::RegexSubst { subject, .. } => {
                self.expr(subject, locals)
            }
            Expr::ChanRecv { chan, .. } => self.expr(chan, locals),
            Expr::Yield { value, .. } => {
                if let Some(v) = value {
                    self.expr(v, locals);
                }
            }
            Expr::IfElse {
                cond, then, els, ..
            } => {
                self.expr(cond, locals);
                self.expr(then, locals);
                self.expr(els, locals);
            }
            // Literals carry nothing to rewrite.
            Expr::Int(..)
            | Expr::Float(..)
            | Expr::Imaginary(..)
            | Expr::Str { .. }
            | Expr::Bool(..)
            | Expr::None(..) => {}
        }
    }

    /// Resolve `alias.member` (where `alias` is a known import) to the target's
    /// mangled symbol, enforcing privacy. Replaces `*e` in place.
    fn resolve_member(&mut self, e: &mut Expr, facts: &ModuleFacts, member: String, span: Span) {
        if member.starts_with('_') {
            self.diag(
                ErrorCode::ZAP3012,
                format!("`{member}` is private to its module"),
                span,
            );
            return;
        }
        if facts.symbols.contains(&member) {
            *e = Expr::Name(format!("{}{member}", facts.prefix), span);
        } else {
            self.diag(
                ErrorCode::ZAP3013,
                format!("the imported module has no public `{member}`"),
                span,
            );
        }
    }

    // --- types -------------------------------------------------------------

    fn ty(&mut self, t: &mut Type, tl: &HashSet<String>) {
        match t {
            Type::Named { name, args, span } => {
                if let Some((first, rest)) = name.split_once('.') {
                    // Module-qualified type `alias.Type` (no deeper nesting in A).
                    if let Some(facts) = self.aliases.get(first).copied() {
                        let sp = *span;
                        if rest.contains('.') {
                            self.diag(
                                ErrorCode::ZAP3013,
                                format!("`{rest}` is not a type in the imported module"),
                                sp,
                            );
                        } else if rest.starts_with('_') {
                            self.diag(
                                ErrorCode::ZAP3012,
                                format!("`{rest}` is private to its module"),
                                sp,
                            );
                        } else if facts.symbols.contains(rest) {
                            *name = format!("{}{rest}", facts.prefix);
                        } else {
                            self.diag(
                                ErrorCode::ZAP3013,
                                format!("the imported module has no public type `{rest}`"),
                                sp,
                            );
                        }
                    }
                    // A dotted name whose head is not an import is left for sema.
                } else if !tl.contains(name) {
                    if let Some(m) = self.mangle_own(name) {
                        *name = m;
                    }
                }
                for a in args {
                    self.ty(a, tl);
                }
            }
            Type::Tuple { elems, .. } => {
                for e in elems {
                    self.ty(e, tl);
                }
            }
            Type::Func { params, ret, .. } => {
                for p in params {
                    self.ty(p, tl);
                }
                if let Some(r) = ret {
                    self.ty(r, tl);
                }
            }
        }
    }

    // --- patterns ----------------------------------------------------------

    fn pattern(&mut self, p: &mut Pattern, locals: &HashSet<String>) {
        match p {
            Pattern::Struct { path, args, span } => {
                self.rewrite_pat_path(path, *span);
                if let Some(args) = args {
                    for a in args {
                        self.pattern(&mut a.pattern, locals);
                    }
                }
            }
            Pattern::Literal(e) => self.expr(e, locals),
            Pattern::Or(ps, _) | Pattern::Sequence(ps, _) | Pattern::Tuple(ps, _) => {
                for p in ps {
                    self.pattern(p, locals);
                }
            }
            Pattern::Wildcard(_) | Pattern::Capture { .. } | Pattern::Rest { .. } => {}
        }
    }

    /// Rewrite the head of a qualified pattern path: `alias.Enum.Variant`
    /// (import) or `Enum.Variant` (own type).
    fn rewrite_pat_path(&mut self, path: &mut Vec<String>, span: Span) {
        if path.is_empty() {
            return;
        }
        if let Some(facts) = self.aliases.get(&path[0]).copied() {
            if path.len() < 2 {
                return;
            }
            let member = path[1].clone();
            if member.starts_with('_') {
                self.diag(
                    ErrorCode::ZAP3012,
                    format!("`{member}` is private to its module"),
                    span,
                );
                return;
            }
            if facts.symbols.contains(&member) {
                // Drop the alias segment; mangle the type segment.
                path.remove(0);
                path[0] = format!("{}{member}", facts.prefix);
            } else {
                self.diag(
                    ErrorCode::ZAP3013,
                    format!("the imported module has no public `{member}`"),
                    span,
                );
            }
        } else if let Some(m) = self.mangle_own(&path[0]) {
            path[0] = m;
        }
    }
}

// --- binding collection ------------------------------------------------------

fn generics_set(generics: &[TypeParam], extra: &HashSet<String>) -> HashSet<String> {
    let mut s = extra.clone();
    for g in generics {
        s.insert(g.name.clone());
    }
    s
}

fn decorator_bindings(d: &zapc_syntax::ast::DecoratorDef) -> HashSet<String> {
    let mut s = HashSet::new();
    for p in &d.params {
        s.insert(p.name.clone());
    }
    collect_block_bindings(&d.body, &mut s);
    s
}

/// The flat set of value bindings visible anywhere in a function (params + every
/// binding in its body). See the module doc for why this is flat.
fn collect_bindings_fn(f: &Function) -> HashSet<String> {
    let mut s = HashSet::new();
    for p in &f.params {
        s.insert(p.name.clone());
    }
    collect_block_bindings(&f.body, &mut s);
    s
}

fn collect_block_bindings(b: &Block, s: &mut HashSet<String>) {
    for st in &b.stmts {
        collect_stmt_bindings(st, s);
    }
}

fn collect_stmt_bindings(st: &Stmt, s: &mut HashSet<String>) {
    match st {
        Stmt::Let { name, value, .. } => {
            s.insert(name.clone());
            collect_expr_bindings(value, s);
        }
        Stmt::LetTuple { target, value, .. } => {
            collect_bindtarget(target, s);
            collect_expr_bindings(value, s);
        }
        Stmt::Assign { target, value, .. } => {
            if let Expr::Name(n, _) = target {
                s.insert(n.clone());
            }
            collect_expr_bindings(target, s);
            collect_expr_bindings(value, s);
        }
        Stmt::AugAssign { target, value, .. } => {
            collect_expr_bindings(target, s);
            collect_expr_bindings(value, s);
        }
        Stmt::Expr(e) => collect_expr_bindings(e, s),
        Stmt::If {
            branches,
            else_block,
            ..
        } => {
            for (c, body) in branches {
                collect_expr_bindings(c, s);
                collect_block_bindings(body, s);
            }
            if let Some(b) = else_block {
                collect_block_bindings(b, s);
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_expr_bindings(cond, s);
            collect_block_bindings(body, s);
        }
        Stmt::DoWhile { body, cond, .. } => {
            collect_block_bindings(body, s);
            collect_expr_bindings(cond, s);
        }
        Stmt::For {
            target, iter, body, ..
        } => {
            collect_target_names(target, s);
            collect_expr_bindings(iter, s);
            collect_block_bindings(body, s);
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            collect_expr_bindings(scrutinee, s);
            for arm in arms {
                collect_pattern_bindings(&arm.pattern, s);
                if let Some(g) = &arm.guard {
                    collect_expr_bindings(g, s);
                }
                collect_block_bindings(&arm.body, s);
            }
        }
        Stmt::Return { value, .. } | Stmt::Raise { value, .. } => {
            if let Some(v) = value {
                collect_expr_bindings(v, s);
            }
        }
        Stmt::Assert { cond, msg, .. } => {
            collect_expr_bindings(cond, s);
            if let Some(m) = msg {
                collect_expr_bindings(m, s);
            }
        }
        Stmt::Try {
            body,
            excepts,
            finally,
            ..
        } => {
            collect_block_bindings(body, s);
            for ex in excepts {
                if let Some(b) = &ex.binding {
                    s.insert(b.clone());
                }
                collect_block_bindings(&ex.body, s);
            }
            if let Some(f) = finally {
                collect_block_bindings(f, s);
            }
        }
        Stmt::With { items, body, .. } => {
            for item in items {
                if let Some(a) = &item.alias {
                    s.insert(a.clone());
                }
                collect_expr_bindings(&item.expr, s);
            }
            collect_block_bindings(body, s);
        }
        Stmt::Spawn { call, .. } => collect_expr_bindings(call, s),
        Stmt::Scope { opts, body, .. } => {
            for a in opts {
                collect_expr_bindings(&a.value, s);
            }
            collect_block_bindings(body, s);
        }
        Stmt::AssignTuple { target, value, .. } => {
            collect_bindtarget(target, s);
            collect_expr_bindings(value, s);
        }
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::Cfg(_) => {}
    }
}

fn collect_bindtarget(t: &BindTarget, s: &mut HashSet<String>) {
    match t {
        BindTarget::Elem(Expr::Name(n, _)) => {
            s.insert(n.clone());
        }
        BindTarget::Elem(_) => {}
        BindTarget::Rest(inner, _) => collect_bindtarget(inner, s),
        BindTarget::Tuple(parts, _) => {
            for p in parts {
                collect_bindtarget(p, s);
            }
        }
    }
}

/// Names bound by a `for` target expression (a name or a tuple of names).
fn collect_target_names(e: &Expr, s: &mut HashSet<String>) {
    match e {
        Expr::Name(n, _) => {
            s.insert(n.clone());
        }
        Expr::Tuple(xs, _) | Expr::List(xs, _) => {
            for x in xs {
                collect_target_names(x, s);
            }
        }
        _ => {}
    }
}

fn collect_pattern_bindings(p: &Pattern, s: &mut HashSet<String>) {
    match p {
        Pattern::Capture { name, .. } => {
            s.insert(name.clone());
        }
        Pattern::Rest { name: Some(n), .. } => {
            s.insert(n.clone());
        }
        Pattern::Rest { .. } | Pattern::Wildcard(_) | Pattern::Literal(_) => {}
        Pattern::Or(ps, _) | Pattern::Sequence(ps, _) | Pattern::Tuple(ps, _) => {
            for p in ps {
                collect_pattern_bindings(p, s);
            }
        }
        Pattern::Struct { args, .. } => {
            if let Some(args) = args {
                for a in args {
                    collect_pattern_bindings(&a.pattern, s);
                }
            }
        }
    }
}

/// Bindings introduced *inside* an expression (lambda params, comprehension
/// vars) — these shadow module symbols within the enclosing callable.
fn collect_expr_bindings(e: &Expr, s: &mut HashSet<String>) {
    match e {
        Expr::Lambda { params, body, .. } => {
            for p in params {
                s.insert(p.clone());
            }
            collect_expr_bindings(body, s);
        }
        Expr::ListComp {
            elem,
            var,
            iter,
            cond,
            ..
        }
        | Expr::SetComp {
            elem,
            var,
            iter,
            cond,
            ..
        } => {
            s.insert(var.clone());
            collect_expr_bindings(elem, s);
            collect_expr_bindings(iter, s);
            if let Some(c) = cond {
                collect_expr_bindings(c, s);
            }
        }
        Expr::DictComp {
            key,
            value,
            var,
            iter,
            cond,
            ..
        } => {
            s.insert(var.clone());
            collect_expr_bindings(key, s);
            collect_expr_bindings(value, s);
            collect_expr_bindings(iter, s);
            if let Some(c) = cond {
                collect_expr_bindings(c, s);
            }
        }
        Expr::Call { callee, args, .. } => {
            collect_expr_bindings(callee, s);
            for a in args {
                collect_expr_bindings(&a.value, s);
            }
        }
        Expr::Binary { lhs, rhs, .. } => {
            collect_expr_bindings(lhs, s);
            collect_expr_bindings(rhs, s);
        }
        Expr::Compare { first, rest, .. } => {
            collect_expr_bindings(first, s);
            for (_, operand) in rest {
                collect_expr_bindings(operand, s);
            }
        }
        Expr::Unary { expr, .. } => collect_expr_bindings(expr, s),
        Expr::Index { base, index, .. } => {
            collect_expr_bindings(base, s);
            collect_expr_bindings(index, s);
        }
        Expr::Slice {
            base, lo, hi, step, ..
        } => {
            collect_expr_bindings(base, s);
            for part in [lo, hi, step].iter().copied().flatten() {
                collect_expr_bindings(part, s);
            }
        }
        Expr::Attr { base, .. } => collect_expr_bindings(base, s),
        Expr::Tuple(xs, _) | Expr::List(xs, _) | Expr::Set(xs, _) => {
            for x in xs {
                collect_expr_bindings(x, s);
            }
        }
        Expr::Dict(kvs, _) => {
            for (k, v) in kvs {
                collect_expr_bindings(k, s);
                collect_expr_bindings(v, s);
            }
        }
        Expr::RegexMatch { subject, .. } | Expr::RegexSubst { subject, .. } => {
            collect_expr_bindings(subject, s)
        }
        Expr::ChanRecv { chan, .. } => collect_expr_bindings(chan, s),
        Expr::Yield { value: Some(v), .. } => collect_expr_bindings(v, s),
        Expr::IfElse {
            cond, then, els, ..
        } => {
            collect_expr_bindings(cond, s);
            collect_expr_bindings(then, s);
            collect_expr_bindings(els, s);
        }
        _ => {}
    }
}
