//! User-defined **behavioral** decorator expansion — the `before`/`after`
//! convenience form (D-0036, `design/0004-decorators.md`; epic #651, slice
//! #652).
//!
//! A `decorator` whose body declares `before(ctx)` and/or `after(ctx, result)`
//! hooks generates code *around* a decorated free function. Like the
//! registering-decorator desugar (`decorator_desugar.rs`), this is a
//! whole-program AST pass run **before sema**, so the generated wrapper is
//! type-checked exactly like hand-written code and is visible through
//! `zap expand`.
//!
//! ## What is generated
//!
//! ```text
//! decorator logged:
//!     def before(ctx):
//!         print("calling", ctx.name)
//!     def after(ctx, result):
//!         print("->", result)
//!
//! @logged
//! def fetch(str url) -> str:
//!     return "data:" + url
//! ```
//!
//! expands to (names with `$` cannot collide with source identifiers — the lexer
//! admits no `$`):
//!
//! ```text
//! def fetch$orig(str url) -> str:
//!     return "data:" + url
//!
//! def fetch(str url) -> str:          # the decorated function's EXACT signature
//!     print("calling", "fetch")       # before; ctx.name -> the function's name
//!     str $result = fetch$orig(url)   # original call; arguments thread through
//!     print("->", $result)            # after; result -> the captured value
//!     return $result
//! ```
//!
//! Because the wrapper carries the decorated function's exact signature, the
//! "`@wraps` is automatic" contract holds for free: callers, overload resolution
//! and tooling see an unchanged `fetch`.
//!
//! ## Stacking (bottom-up)
//!
//! `@a` over `@b` over `def f` applies **bottom-up**: `b` (nearest the
//! declaration) wraps the body first, then `a` wraps that — matching Python's
//! `a(b(f))`. The chain is built as `f -> f$deco1 -> f$orig`, so at run time
//! `a.before`, `b.before`, body, `b.after`, `a.after` fire in that order.
//!
//! ## Context and arguments
//!
//! `ctx` exposes exactly one field, `ctx.name` (the decorated function's name),
//! substituted to a string literal; any other use of `ctx` is ZAP5053. A
//! decorator's own arguments (`decorator tag(str label)` applied as `@tag("x")`)
//! are bound to synthetic locals the hook bodies reference.
//!
//! The general `wrap(call)` form is deferred to #653; applying a `wrap`
//! decorator reports ZAP5052.

use std::collections::HashMap;

use zapc_diag::ErrorCode;
use zapc_sema::TypeError;
use zapc_syntax::ast::{
    Arg, ArgSpread, Block, CompClause, DecoratorDef, DecoratorMember, DictItem, ExceptClause, Expr,
    Function, Item, MatchArm, Module, Param, Stmt, WithItem,
};
use zapc_syntax::span::Span;

/// Expand every applied behavioral (`before`/`after`) decorator in `module` in
/// place. A decorator is *behavioral* when its declaration has at least one
/// member hook; a `pass`-bodied (declared-only) decorator is metadata and is
/// left untouched for the reflection path (D-0087). Returns any diagnostics
/// raised; on a non-empty result the module is left partially transformed and
/// the caller aborts before sema.
pub fn expand_behavioral_decorators(module: &mut Module) -> Vec<TypeError> {
    // Index behavioral decorator declarations by name.
    let behavioral: HashMap<String, DecoratorDef> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Decorator(d) if !d.members.is_empty() => Some((d.name.clone(), d.clone())),
            _ => None,
        })
        .collect();

    let mut diags: Vec<TypeError> = Vec::new();

    if behavioral.is_empty() {
        return diags;
    }

    // A behavioral decorator on anything but a free top-level function is not
    // supported in this slice (#651): report each such use precisely.
    report_unsupported_targets(module, &behavioral, &mut diags);

    // Rewrite the top-level function list, expanding each free function that
    // carries at least one behavioral decorator into its `$orig` + wrapper chain.
    let mut new_items: Vec<Item> = Vec::with_capacity(module.items.len());
    for item in std::mem::take(&mut module.items) {
        let Item::Function(f) = item else {
            new_items.push(item);
            continue;
        };
        if f.decorators
            .iter()
            .all(|d| !behavioral.contains_key(&d.name))
        {
            new_items.push(Item::Function(f));
            continue;
        }
        expand_function(f, &behavioral, &mut new_items, &mut diags);
    }
    module.items = new_items;

    diags
}

/// Expand one free function carrying one or more behavioral decorators, pushing
/// the renamed original, any intermediate wrappers, and the final wrapper (which
/// keeps the function's original name and its non-behavioral decorators) onto
/// `out`.
fn expand_function(
    mut f: Function,
    behavioral: &HashMap<String, DecoratorDef>,
    out: &mut Vec<Item>,
    diags: &mut Vec<TypeError>,
) {
    let real_name = f.name.clone();

    // Split decorators into the behavioral ones to apply (source order) and the
    // metadata ones kept on the outermost wrapper.
    let mut applied: Vec<&DecoratorDef> = Vec::new();
    let mut applied_calls: Vec<Vec<Arg>> = Vec::new();
    let mut kept = Vec::new();
    for d in &f.decorators {
        if let Some(def) = behavioral.get(&d.name) {
            applied.push(def);
            applied_calls.push(d.args.clone());
        } else {
            kept.push(d.clone());
        }
    }

    // The renamed original holds the untouched body + signature, no decorators.
    let orig_name = format!("{real_name}$orig");
    let orig = Function {
        decorators: Vec::new(),
        name: orig_name.clone(),
        generics: f.generics.clone(),
        params: f.params.clone(),
        ret: f.ret.clone(),
        body: std::mem::replace(
            &mut f.body,
            Block {
                stmts: Vec::new(),
                span: f.span,
            },
        ),
        span: f.span,
    };
    out.push(Item::Function(orig));

    // Fold the behavioral decorators bottom-up: the innermost (last written) is
    // nearest the declaration and wraps the original first.
    let mut inner_name = orig_name;
    let last = applied.len().saturating_sub(1);
    for (idx, (def, call_args)) in applied.iter().zip(applied_calls.iter()).enumerate().rev() {
        let is_outer = idx == 0;
        let wrapper_name = if is_outer {
            real_name.clone()
        } else {
            format!("{real_name}$deco{idx}")
        };
        let decorators = if is_outer {
            std::mem::take(&mut kept)
        } else {
            Vec::new()
        };
        let wrapper = build_wrapper(
            &wrapper_name,
            &real_name,
            &inner_name,
            &f,
            def,
            call_args,
            decorators,
            diags,
        );
        out.push(Item::Function(wrapper));
        inner_name = wrapper_name;
        let _ = last; // keep the binding readable; the fold drives naming.
    }
}

/// Build a single wrapper function named `wrapper_name` that applies `def`
/// (a behavioral decorator) around a call to `inner_name`. `sig` supplies the
/// wrapped function's signature (params/ret/generics); `real_name` is the
/// decorated function's own name (for `ctx.name`). `decorators` are the
/// metadata decorators to carry on this wrapper (non-empty only for the outer).
#[allow(clippy::too_many_arguments)]
fn build_wrapper(
    wrapper_name: &str,
    real_name: &str,
    inner_name: &str,
    sig: &Function,
    def: &DecoratorDef,
    call_args: &[Arg],
    decorators: Vec<zapc_syntax::ast::Decorator>,
    diags: &mut Vec<TypeError>,
) -> Function {
    let sp = sig.span;
    let has_ret = sig.ret.is_some();

    let mut stmts: Vec<Stmt> = Vec::new();

    // Bind the decorator's own arguments to synthetic locals the hook bodies
    // reference (renamed so they cannot clash with the wrapped function's
    // parameters). `@tag("x")` with `decorator tag(str label)` emits
    // `str label$arg = "x"`.
    let mut renames: HashMap<String, String> = HashMap::new();
    bind_decorator_args(def, call_args, &mut stmts, &mut renames, diags);

    // Locate the hooks; validate their shape.
    let mut before: Option<&DecoratorMember> = None;
    let mut after: Option<&DecoratorMember> = None;
    for m in &def.members {
        match m.name.as_str() {
            "before" => {
                check_arity(m, 1, diags);
                before = Some(m);
            }
            "after" => {
                check_arity(m, 2, diags);
                after = Some(m);
            }
            "wrap" => {
                diags.push(err(
                    ErrorCode::ZAP5052,
                    format!(
                        "the `wrap(call)` form of decorator `@{}` is not generated yet (#653); \
                         the `before`/`after` form is available now",
                        def.name
                    ),
                    m.span,
                ));
            }
            other => {
                diags.push(err(
                    ErrorCode::ZAP5049,
                    format!(
                        "decorator `@{}` declares an unknown hook `{other}` \
                         (expected `before`, `after`, or `wrap`)",
                        def.name
                    ),
                    m.span,
                ));
            }
        }
    }

    // `before`: run first, with `ctx.name` substituted and no `result` in scope.
    if let Some(m) = before {
        let ctx = m.params.first().cloned().unwrap_or_default();
        let mut rw = Rewriter {
            ctx,
            result_from: None,
            result_to: RESULT_LOCAL.to_string(),
            renames: &renames,
            fn_name: real_name,
            diags,
        };
        if let Some(body) = &m.body {
            for s in &body.stmts {
                stmts.push(rw.stmt(s.clone()));
            }
        }
    }

    // The original call, threading the wrapper's arguments through unchanged.
    let call = Expr::Call {
        callee: Box::new(Expr::Name(inner_name.to_string(), sp)),
        args: forward_args(&sig.params, sp),
        span: sp,
    };
    if has_ret {
        stmts.push(Stmt::Let {
            is_const: false,
            ty: sig.ret.clone(),
            name: RESULT_LOCAL.to_string(),
            value: call,
            span: sp,
        });
    } else {
        stmts.push(Stmt::Expr(call));
    }

    // `after`: sees the captured return value as its second parameter.
    if let Some(m) = after {
        if !has_ret {
            diags.push(err(
                ErrorCode::ZAP5053,
                format!(
                    "decorator `@{}`'s `after(ctx, result)` needs `{real_name}` to return a \
                     value, but it returns nothing",
                    def.name
                ),
                m.span,
            ));
        }
        let ctx = m.params.first().cloned().unwrap_or_default();
        let result_from = m.params.get(1).cloned();
        let mut rw = Rewriter {
            ctx,
            result_from,
            result_to: RESULT_LOCAL.to_string(),
            renames: &renames,
            fn_name: real_name,
            diags,
        };
        if let Some(body) = &m.body {
            for s in &body.stmts {
                stmts.push(rw.stmt(s.clone()));
            }
        }
    }

    if has_ret {
        stmts.push(Stmt::Return {
            value: Some(Expr::Name(RESULT_LOCAL.to_string(), sp)),
            span: sp,
        });
    }

    Function {
        decorators,
        name: wrapper_name.to_string(),
        generics: sig.generics.clone(),
        params: sig.params.clone(),
        ret: sig.ret.clone(),
        body: Block { stmts, span: sp },
        span: sp,
    }
}

/// The synthetic local holding the wrapped call's return value.
const RESULT_LOCAL: &str = "$result";

/// Bind a decorator's declared parameters to `<ty> <name>$arg = <value>` locals,
/// matching supplied `@deco(...)` arguments positionally then by keyword, and
/// record the `name -> name$arg` rename used inside the hook bodies.
fn bind_decorator_args(
    def: &DecoratorDef,
    call_args: &[Arg],
    stmts: &mut Vec<Stmt>,
    renames: &mut HashMap<String, String>,
    diags: &mut Vec<TypeError>,
) {
    // Map each decorator parameter to the supplied argument expression.
    let mut by_name: HashMap<String, Expr> = HashMap::new();
    let mut pos = 0usize;
    for a in call_args {
        match &a.name {
            Some(n) => {
                by_name.insert(n.clone(), a.value.clone());
            }
            None => {
                if let Some(p) = def.params.get(pos) {
                    by_name.insert(p.name.clone(), a.value.clone());
                }
                pos += 1;
            }
        }
    }

    for p in &def.params {
        let local = format!("{}$arg", p.name);
        renames.insert(p.name.clone(), local.clone());
        let value = by_name.remove(&p.name).or_else(|| p.default.clone());
        match value {
            Some(v) => stmts.push(Stmt::Let {
                is_const: false,
                ty: p.ty.clone(),
                name: local,
                value: v,
                span: p.span,
            }),
            None => diags.push(err(
                ErrorCode::ZAP5053,
                format!(
                    "decorator `@{}` is missing a value for argument `{}`",
                    def.name, p.name
                ),
                def.span,
            )),
        }
    }
}

/// Positional/keyword arguments that forward a wrapper's parameters unchanged to
/// the inner function — a variadic collector re-splats, a kwargs collector
/// re-maps, and a keyword-only parameter is passed by name.
fn forward_args(params: &[Param], sp: Span) -> Vec<Arg> {
    params
        .iter()
        .filter(|p| p.name != "self")
        .map(|p| {
            let value = Expr::Name(p.name.clone(), sp);
            if p.variadic {
                Arg {
                    name: None,
                    value,
                    spread: ArgSpread::Iterable,
                    span: sp,
                }
            } else if p.kwargs {
                Arg {
                    name: None,
                    value,
                    spread: ArgSpread::Mapping,
                    span: sp,
                }
            } else if p.keyword_only {
                Arg {
                    name: Some(p.name.clone()),
                    value,
                    spread: ArgSpread::None,
                    span: sp,
                }
            } else {
                Arg {
                    name: None,
                    value,
                    spread: ArgSpread::None,
                    span: sp,
                }
            }
        })
        .collect()
}

fn check_arity(m: &DecoratorMember, want: usize, diags: &mut Vec<TypeError>) {
    if m.params.len() != want {
        let shape = if want == 1 {
            "before(ctx)"
        } else {
            "after(ctx, result)"
        };
        diags.push(err(
            ErrorCode::ZAP5051,
            format!(
                "decorator hook `{}` takes {want} parameter(s) ({shape}), found {}",
                m.name,
                m.params.len()
            ),
            m.span,
        ));
    }
}

/// Report a behavioral decorator applied to anything but a free top-level
/// function (ZAP5050). Free functions are expanded elsewhere; here we only scan
/// the other decoratable sites.
fn report_unsupported_targets(
    module: &Module,
    behavioral: &HashMap<String, DecoratorDef>,
    diags: &mut Vec<TypeError>,
) {
    let mut flag = |name: &str, span: Span, what: &str| {
        if behavioral.contains_key(name) {
            diags.push(err(
                ErrorCode::ZAP5050,
                format!(
                    "behavioral decorator `@{name}` cannot be applied to a {what} yet \
                     (epic #651 supports free functions)"
                ),
                span,
            ));
        }
    };
    for it in &module.items {
        match it {
            Item::Struct(s) => {
                for d in &s.decorators {
                    flag(&d.name, d.span, "struct");
                }
                for fld in &s.fields {
                    for d in &fld.decorators {
                        flag(&d.name, d.span, "field");
                    }
                }
                for m in &s.methods {
                    for d in &m.decorators {
                        flag(&d.name, d.span, "method");
                    }
                    for p in &m.params {
                        for d in &p.decorators {
                            flag(&d.name, d.span, "parameter");
                        }
                    }
                }
            }
            Item::Enum(e) => {
                for d in &e.decorators {
                    flag(&d.name, d.span, "enum");
                }
            }
            Item::Function(f) => {
                for p in &f.params {
                    for d in &p.decorators {
                        flag(&d.name, d.span, "parameter");
                    }
                }
            }
            _ => {}
        }
    }
}

fn err(code: ErrorCode, message: String, span: Span) -> TypeError {
    TypeError {
        code,
        message,
        span,
    }
}

// ----------------------------------------------------------------------------
// AST rewriter for hook bodies
// ----------------------------------------------------------------------------

/// Rewrites a `before`/`after` hook body for a particular decoration site:
/// `ctx.name` becomes a string literal, the `result` binding and the decorator's
/// own argument names become their synthetic locals, and any other use of `ctx`
/// is a stable diagnostic (ZAP5053).
struct Rewriter<'a> {
    /// The hook's `ctx` parameter name.
    ctx: String,
    /// The hook's `result` parameter name (only `after` has one).
    result_from: Option<String>,
    /// The synthetic local the `result` parameter maps to.
    result_to: String,
    /// Decorator-argument renames (`times -> times$arg`).
    renames: &'a HashMap<String, String>,
    /// The decorated function's name, substituted for `ctx.name`.
    fn_name: &'a str,
    diags: &'a mut Vec<TypeError>,
}

impl Rewriter<'_> {
    /// Map a bare identifier used as a *value* in a hook body.
    fn map_name(&mut self, n: String, span: Span) -> Expr {
        if n == self.ctx {
            self.diags.push(err(
                ErrorCode::ZAP5053,
                "a decorator's `ctx` exposes only `ctx.name`; it cannot be used as a value"
                    .to_string(),
                span,
            ));
            return Expr::Name(n, span);
        }
        if self.result_from.as_deref() == Some(n.as_str()) {
            return Expr::Name(self.result_to.clone(), span);
        }
        if let Some(local) = self.renames.get(&n) {
            return Expr::Name(local.clone(), span);
        }
        Expr::Name(n, span)
    }

    fn stmt(&mut self, s: Stmt) -> Stmt {
        match s {
            Stmt::Let {
                is_const,
                ty,
                name,
                value,
                span,
            } => Stmt::Let {
                is_const,
                ty,
                name,
                value: self.expr(value),
                span,
            },
            Stmt::Assign {
                target,
                value,
                span,
            } => Stmt::Assign {
                target: self.expr(target),
                value: self.expr(value),
                span,
            },
            Stmt::AugAssign {
                target,
                op,
                value,
                span,
            } => Stmt::AugAssign {
                target: self.expr(target),
                op,
                value: self.expr(value),
                span,
            },
            Stmt::AssignChain {
                targets,
                value,
                span,
            } => Stmt::AssignChain {
                targets: targets.into_iter().map(|t| self.expr(t)).collect(),
                value: self.expr(value),
                span,
            },
            Stmt::Expr(e) => Stmt::Expr(self.expr(e)),
            Stmt::If {
                branches,
                else_block,
                span,
            } => Stmt::If {
                branches: branches
                    .into_iter()
                    .map(|(c, b)| (self.expr(c), self.block(b)))
                    .collect(),
                else_block: else_block.map(|b| self.block(b)),
                span,
            },
            Stmt::While { cond, body, span } => Stmt::While {
                cond: self.expr(cond),
                body: self.block(body),
                span,
            },
            Stmt::DoWhile { body, cond, span } => Stmt::DoWhile {
                body: self.block(body),
                cond: self.expr(cond),
                span,
            },
            Stmt::For {
                target,
                iter,
                body,
                span,
            } => Stmt::For {
                target: self.expr(target),
                iter: self.expr(iter),
                body: self.block(body),
                span,
            },
            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => Stmt::Match {
                scrutinee: self.expr(scrutinee),
                arms: arms.into_iter().map(|a| self.arm(a)).collect(),
                span,
            },
            Stmt::Return { value, span } => Stmt::Return {
                value: value.map(|v| self.expr(v)),
                span,
            },
            Stmt::Raise { value, cause, span } => Stmt::Raise {
                value: value.map(|v| self.expr(v)),
                cause: cause.map(|c| self.expr(c)),
                span,
            },
            Stmt::Assert {
                cond,
                msg,
                cond_src,
                span,
            } => Stmt::Assert {
                cond: self.expr(cond),
                msg: msg.map(|m| self.expr(m)),
                cond_src,
                span,
            },
            Stmt::Try {
                body,
                excepts,
                finally,
                span,
            } => Stmt::Try {
                body: self.block(body),
                excepts: excepts
                    .into_iter()
                    .map(|e| ExceptClause {
                        ty: e.ty,
                        binding: e.binding,
                        body: self.block(e.body),
                        span: e.span,
                    })
                    .collect(),
                finally: finally.map(|b| self.block(b)),
                span,
            },
            Stmt::With { items, body, span } => Stmt::With {
                items: items
                    .into_iter()
                    .map(|w| WithItem {
                        expr: self.expr(w.expr),
                        alias: w.alias,
                        span: w.span,
                    })
                    .collect(),
                body: self.block(body),
                span,
            },
            Stmt::Spawn {
                call,
                detached,
                span,
            } => Stmt::Spawn {
                call: self.expr(call),
                detached,
                span,
            },
            Stmt::Scope { opts, body, span } => Stmt::Scope {
                opts: opts.into_iter().map(|a| self.arg(a)).collect(),
                body: self.block(body),
                span,
            },
            Stmt::LetTuple {
                is_const,
                ty,
                target,
                value,
                span,
            } => Stmt::LetTuple {
                is_const,
                ty,
                target,
                value: self.expr(value),
                span,
            },
            Stmt::AssignTuple {
                target,
                value,
                span,
            } => Stmt::AssignTuple {
                target,
                value: self.expr(value),
                span,
            },
            Stmt::Del { target, span } => Stmt::Del {
                target: self.expr(target),
                span,
            },
            // No sub-expressions to rewrite.
            s @ (Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) | Stmt::Cfg(_)) => s,
        }
    }

    fn block(&mut self, b: Block) -> Block {
        Block {
            stmts: b.stmts.into_iter().map(|s| self.stmt(s)).collect(),
            span: b.span,
        }
    }

    fn arm(&mut self, a: MatchArm) -> MatchArm {
        MatchArm {
            pattern: a.pattern,
            guard: a.guard.map(|g| self.expr(g)),
            body: self.block(a.body),
            span: a.span,
        }
    }

    fn arg(&mut self, a: Arg) -> Arg {
        Arg {
            name: a.name,
            value: self.expr(a.value),
            spread: a.spread,
            span: a.span,
        }
    }

    fn comp(&mut self, c: CompClause) -> CompClause {
        CompClause {
            var: c.var,
            iter: Box::new(self.expr(*c.iter)),
            ifs: c.ifs.into_iter().map(|e| self.expr(e)).collect(),
            span: c.span,
        }
    }

    fn expr(&mut self, e: Expr) -> Expr {
        match e {
            // `ctx.name` -> the decorated function's name, as a string literal.
            Expr::Attr { base, name, span } => {
                if let Expr::Name(b, _) = base.as_ref() {
                    if *b == self.ctx {
                        if name == "name" {
                            return Expr::Str {
                                value: self.fn_name.to_string(),
                                raw: false,
                                bytes: false,
                                fstring: false,
                                span,
                                fstring_fields: Vec::new(),
                            };
                        }
                        self.diags.push(err(
                            ErrorCode::ZAP5053,
                            format!(
                                "a decorator's `ctx` exposes only `ctx.name`, not `ctx.{name}`"
                            ),
                            span,
                        ));
                        return Expr::Attr { base, name, span };
                    }
                }
                Expr::Attr {
                    base: Box::new(self.expr(*base)),
                    name,
                    span,
                }
            }
            Expr::Name(n, span) => self.map_name(n, span),
            // Leaf literals.
            e @ (Expr::Int(..)
            | Expr::Float(..)
            | Expr::Imaginary(..)
            | Expr::Bool(..)
            | Expr::None(..)) => e,
            Expr::Str {
                value,
                raw,
                bytes,
                fstring,
                span,
                fstring_fields,
            } => Expr::Str {
                value,
                raw,
                bytes,
                fstring,
                span,
                fstring_fields: fstring_fields.into_iter().map(|e| self.expr(e)).collect(),
            },
            Expr::Tuple(xs, s) => Expr::Tuple(xs.into_iter().map(|e| self.expr(e)).collect(), s),
            Expr::List(xs, s) => Expr::List(xs.into_iter().map(|e| self.expr(e)).collect(), s),
            Expr::Set(xs, s) => Expr::Set(xs.into_iter().map(|e| self.expr(e)).collect(), s),
            Expr::Dict(items, s) => Expr::Dict(
                items
                    .into_iter()
                    .map(|it| match it {
                        DictItem::Pair(k, v) => DictItem::Pair(self.expr(k), self.expr(v)),
                        DictItem::Spread(e) => DictItem::Spread(self.expr(e)),
                    })
                    .collect(),
                s,
            ),
            Expr::Starred { value, span } => Expr::Starred {
                value: Box::new(self.expr(*value)),
                span,
            },
            Expr::ListComp {
                elem,
                clauses,
                span,
            } => Expr::ListComp {
                elem: Box::new(self.expr(*elem)),
                clauses: clauses.into_iter().map(|c| self.comp(c)).collect(),
                span,
            },
            Expr::DictComp {
                key,
                value,
                clauses,
                span,
            } => Expr::DictComp {
                key: Box::new(self.expr(*key)),
                value: Box::new(self.expr(*value)),
                clauses: clauses.into_iter().map(|c| self.comp(c)).collect(),
                span,
            },
            Expr::SetComp {
                elem,
                clauses,
                span,
            } => Expr::SetComp {
                elem: Box::new(self.expr(*elem)),
                clauses: clauses.into_iter().map(|c| self.comp(c)).collect(),
                span,
            },
            Expr::GenExp {
                elem,
                clauses,
                span,
            } => Expr::GenExp {
                elem: Box::new(self.expr(*elem)),
                clauses: clauses.into_iter().map(|c| self.comp(c)).collect(),
                span,
            },
            Expr::Unary { op, expr, span } => Expr::Unary {
                op,
                expr: Box::new(self.expr(*expr)),
                span,
            },
            Expr::Binary { op, lhs, rhs, span } => Expr::Binary {
                op,
                lhs: Box::new(self.expr(*lhs)),
                rhs: Box::new(self.expr(*rhs)),
                span,
            },
            Expr::Compare { first, rest, span } => Expr::Compare {
                first: Box::new(self.expr(*first)),
                rest: rest.into_iter().map(|(op, e)| (op, self.expr(e))).collect(),
                span,
            },
            Expr::Call { callee, args, span } => Expr::Call {
                callee: Box::new(self.expr(*callee)),
                args: args.into_iter().map(|a| self.arg(a)).collect(),
                span,
            },
            Expr::Index { base, index, span } => Expr::Index {
                base: Box::new(self.expr(*base)),
                index: Box::new(self.expr(*index)),
                span,
            },
            Expr::Slice {
                base,
                lo,
                hi,
                step,
                span,
            } => Expr::Slice {
                base: Box::new(self.expr(*base)),
                lo: lo.map(|e| Box::new(self.expr(*e))),
                hi: hi.map(|e| Box::new(self.expr(*e))),
                step: step.map(|e| Box::new(self.expr(*e))),
                span,
            },
            Expr::Lambda { params, body, span } => Expr::Lambda {
                params,
                body: Box::new(self.expr(*body)),
                span,
            },
            Expr::RegexMatch {
                subject,
                pattern,
                flags,
                negated,
                span,
            } => Expr::RegexMatch {
                subject: Box::new(self.expr(*subject)),
                pattern,
                flags,
                negated,
                span,
            },
            Expr::RegexSubst {
                subject,
                pattern,
                replacement,
                flags,
                span,
            } => Expr::RegexSubst {
                subject: Box::new(self.expr(*subject)),
                pattern,
                replacement,
                flags,
                span,
            },
            Expr::ChanRecv { chan, span } => Expr::ChanRecv {
                chan: Box::new(self.expr(*chan)),
                span,
            },
            Expr::Yield { value, span } => Expr::Yield {
                value: value.map(|v| Box::new(self.expr(*v))),
                span,
            },
            Expr::IfElse {
                cond,
                then,
                els,
                span,
            } => Expr::IfElse {
                cond: Box::new(self.expr(*cond)),
                then: Box::new(self.expr(*then)),
                els: Box::new(self.expr(*els)),
                span,
            },
            Expr::Named {
                name,
                name_span,
                value,
                span,
            } => Expr::Named {
                name,
                name_span,
                value: Box::new(self.expr(*value)),
                span,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zapc_syntax::parser::parse_str;

    fn module(src: &str) -> Module {
        let (m, perrs, lerrs) = parse_str(src);
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        m
    }

    fn func<'a>(m: &'a Module, name: &str) -> Option<&'a Function> {
        m.items.iter().find_map(|it| match it {
            Item::Function(f) if f.name == name => Some(f),
            _ => None,
        })
    }

    const LOGGED: &str = "decorator logged:\n    def before(ctx):\n        print(\"calling\", ctx.name)\n    def after(ctx, result):\n        print(\"->\", result)\n";

    #[test]
    fn before_after_generates_orig_and_wrapper() {
        let mut m = module(&format!(
            "{LOGGED}@logged\ndef fetch(str url) -> str:\n    return url\ndef main():\n    print(fetch(\"x\"))\n"
        ));
        let diags = expand_behavioral_decorators(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");

        // Original renamed, keeps its signature and body.
        let orig = func(&m, "fetch$orig").expect("renamed original");
        assert_eq!(orig.params.len(), 1);
        assert!(orig.decorators.is_empty());

        // Wrapper keeps the real name + exact signature; decorator gone.
        let wrapper = func(&m, "fetch").expect("wrapper");
        assert!(
            wrapper.decorators.is_empty(),
            "behavioral decorator consumed"
        );
        assert_eq!(wrapper.params.len(), 1);
        assert_eq!(wrapper.params[0].name, "url");
        assert!(
            matches!(&wrapper.ret, Some(zapc_syntax::ast::Type::Named { name, .. }) if name == "str")
        );

        // Body shape: before (print), $result = fetch$orig(url), after, return.
        let b = &wrapper.body.stmts;
        assert!(
            matches!(&b[0], Stmt::Expr(Expr::Call { .. })),
            "before: {:?}",
            b[0]
        );
        assert!(
            matches!(&b[1], Stmt::Let { name, value: Expr::Call { callee, .. }, .. }
                if name == "$result" && matches!(callee.as_ref(), Expr::Name(n, _) if n == "fetch$orig")),
            "orig call: {:?}",
            b[1]
        );
        assert!(
            matches!(&b[2], Stmt::Expr(Expr::Call { .. })),
            "after: {:?}",
            b[2]
        );
        assert!(
            matches!(&b[3], Stmt::Return { value: Some(Expr::Name(n, _)), .. } if n == "$result"),
            "return: {:?}",
            b[3]
        );
    }

    #[test]
    fn ctx_name_is_substituted_to_literal() {
        let mut m = module(&format!(
            "{LOGGED}@logged\ndef fetch(str url) -> str:\n    return url\n"
        ));
        expand_behavioral_decorators(&mut m);
        let wrapper = func(&m, "fetch").unwrap();
        // The before `print("calling", ctx.name)` -> ("calling", "fetch").
        let Stmt::Expr(Expr::Call { args, .. }) = &wrapper.body.stmts[0] else {
            panic!("expected before call");
        };
        assert!(
            matches!(&args[1].value, Expr::Str { value, .. } if value == "fetch"),
            "ctx.name -> literal: {:?}",
            args[1].value
        );
    }

    #[test]
    fn stacking_is_bottom_up() {
        let src = "decorator a:\n    def before(ctx):\n        print(\"a\")\n\
                   decorator b:\n    def before(ctx):\n        print(\"b\")\n\
                   @a\n@b\ndef f() -> int64:\n    return 0\n";
        let mut m = module(src);
        let diags = expand_behavioral_decorators(&mut m);
        assert!(diags.is_empty(), "{diags:?}");
        // Chain: f (a, outer) -> f$deco1 (b) -> f$orig.
        let outer = func(&m, "f").expect("outer");
        let Stmt::Let {
            value: Expr::Call { callee, .. },
            ..
        } = &outer.body.stmts[1]
        else {
            panic!("expected call in outer");
        };
        assert!(matches!(callee.as_ref(), Expr::Name(n, _) if n == "f$deco1"));
        let mid = func(&m, "f$deco1").expect("inner wrapper");
        let Stmt::Let {
            value: Expr::Call { callee, .. },
            ..
        } = &mid.body.stmts[1]
        else {
            panic!("expected call in mid");
        };
        assert!(matches!(callee.as_ref(), Expr::Name(n, _) if n == "f$orig"));
    }

    #[test]
    fn decorator_arguments_bind_locals() {
        let src = "decorator tag(str label):\n    def before(ctx):\n        print(label)\n\
                   @tag(\"hi\")\ndef f() -> int64:\n    return 0\n";
        let mut m = module(src);
        let diags = expand_behavioral_decorators(&mut m);
        assert!(diags.is_empty(), "{diags:?}");
        let f = func(&m, "f").unwrap();
        // First stmt binds `str label$arg = "hi"`; before's `print(label)` is
        // rewritten to `print(label$arg)`.
        assert!(
            matches!(&f.body.stmts[0], Stmt::Let { name, .. } if name == "label$arg"),
            "arg bind: {:?}",
            f.body.stmts[0]
        );
        let Stmt::Expr(Expr::Call { args, .. }) = &f.body.stmts[1] else {
            panic!("expected before call");
        };
        assert!(matches!(&args[0].value, Expr::Name(n, _) if n == "label$arg"));
    }

    #[test]
    fn wrap_form_is_deferred() {
        let src = "decorator retry(int64 times):\n    def wrap(call):\n        def inner():\n            return call()\n        return inner\n\
                   @retry(times=3)\ndef f() -> int64:\n    return 0\n";
        let mut m = module(src);
        let diags = expand_behavioral_decorators(&mut m);
        assert!(
            diags.iter().any(|d| d.code == ErrorCode::ZAP5052),
            "wrap should report ZAP5052: {diags:?}"
        );
    }

    #[test]
    fn unknown_hook_is_rejected() {
        let src = "decorator bad:\n    def frobnicate(ctx):\n        print(ctx.name)\n\
                   @bad\ndef f() -> int64:\n    return 0\n";
        let mut m = module(src);
        let diags = expand_behavioral_decorators(&mut m);
        assert!(
            diags.iter().any(|d| d.code == ErrorCode::ZAP5049),
            "unknown hook should report ZAP5049: {diags:?}"
        );
    }

    #[test]
    fn behavioral_on_struct_is_unsupported() {
        let src = "decorator logged:\n    def before(ctx):\n        print(ctx.name)\n\
                   @logged\nstruct S:\n    int64 x\n";
        let mut m = module(src);
        let diags = expand_behavioral_decorators(&mut m);
        assert!(
            diags.iter().any(|d| d.code == ErrorCode::ZAP5050),
            "decorator on struct should report ZAP5050: {diags:?}"
        );
    }

    #[test]
    fn declared_only_decorator_is_left_untouched() {
        let src = "decorator audited:\n    pass\n@audited\ndef f() -> int64:\n    return 0\n";
        let mut m = module(src);
        let diags = expand_behavioral_decorators(&mut m);
        assert!(diags.is_empty(), "{diags:?}");
        // No expansion: the function keeps its name and its metadata decorator.
        let f = func(&m, "f").expect("f stays");
        assert_eq!(f.decorators.len(), 1, "metadata decorator preserved");
        assert!(func(&m, "f$orig").is_none(), "no wrapper generated");
    }
}
