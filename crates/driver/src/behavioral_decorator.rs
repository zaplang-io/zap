//! User-defined **behavioral** decorator expansion — the `before`/`after`
//! convenience form (D-0036, `design/0004-decorators.md`; epic #651, slice
//! #652) and the general `wrap(call)` form (slice #653).
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
//! ## The general `wrap(call)` form (#653)
//!
//! ```text
//! decorator retry(int64 times):
//!     def wrap(call):
//!         def inner(*args):
//!             for i in range(times):
//!                 try:
//!                     return call(*args)
//!                 except Error:
//!                     if i == times - 1: raise
//!         return inner
//!
//! @retry(times=3)
//! def fetch(str url) -> str: ...
//! ```
//!
//! expands to a wrapper carrying `fetch`'s exact signature, with the `inner`
//! body inlined and `call(*args)` rewritten to the typed forwarding call
//! `fetch$orig(url)`:
//!
//! ```text
//! def fetch(str url) -> str:
//!     int64 times$arg = 3
//!     for i in range(times$arg):
//!         try:
//!             return fetch$orig(url)
//!         except Error:
//!             if i == times$arg - 1: raise
//!     raise RuntimeError("…")          # implicit fall-through guard
//! ```
//!
//! `*args` is a **compile-time forwarding template**, not a runtime varargs
//! value (D-0108): it exists only to spell `call(*args)`, which forwards the
//! decorated function's exact typed parameters. A `wrap` is mutually exclusive
//! with `before`/`after`, and stacks with other decorators bottom-up like any
//! wrapper.
//!
//! ## The built-in `@cache` decorator (#666, D-0115, epic #661)
//!
//! `@cache` is a *built-in* wrapper decorator — memoization by the decorated
//! function's typed argument tuple — handled in this same pass (not a separate
//! one) precisely so it stacks with user `before`/`after`/`wrap` decorators in
//! correct bottom-up source order through the one shared fold. It generates
//! ordinary, `zap expand`-visible, type-checked Zap over a **module-level dict**
//! — never a hidden compiler store (a hard project rule). For
//!
//! ```text
//! @cache
//! def fib(int64 n) -> int64:
//!     return n if n < 2 else fib(n - 1) + fib(n - 2)
//! ```
//!
//! it synthesizes (with `$` names that no source identifier can collide with):
//!
//! ```text
//! @derive(Hash, Eq)                 # the typed argument tuple, as a named struct
//! struct fib$Key:
//!     int64 n
//!
//! @shareable                        # guarded store → silences the #664 ZAP5125 lint
//! struct fib$Cache:
//!     bytes lock                    # a sync.mutex() handle
//!     dict[fib$Key, int64] data
//!
//! fib$Cache fib$cache = fib$Cache(sync.mutex(), {})   # immortal module global
//!
//! def fib$orig(int64 n) -> int64: ...                 # the renamed body
//!
//! def fib(int64 n) -> int64:                          # the wrapper (exact signature)
//!     auto $k = fib$Key(n)
//!     sync.lock(fib$cache.lock)
//!     if $k in fib$cache.data:
//!         auto $hit = fib$cache.data[$k]
//!         sync.unlock(fib$cache.lock)
//!         return $hit
//!     sync.unlock(fib$cache.lock)
//!     auto $result = fib$orig(n)                       # computed OUTSIDE the lock
//!     sync.lock(fib$cache.lock)
//!     fib$cache.data[$k] = $result
//!     sync.unlock(fib$cache.lock)
//!     return $result
//! ```
//!
//! The result is computed *outside* the lock so a recursive `@cache` function
//! (`fib` calls `fib`, the wrapper) cannot self-deadlock, and the lock only
//! guards the dict bookkeeping — the same shape as CPython's `functools.lru_cache`
//! (lock around the table, not around the user call). A key-struct keyed dict
//! (named, `@derive(Hash, Eq)`) is used rather than a raw tuple key, which is the
//! fully-supported hashable path (a bare tuple key currently ICEs — #680). The
//! store is unbounded for the process lifetime (the global is an immortal GC root,
//! #661/#663): a documented memory cost, not an eviction policy (D-0115). A
//! signature that cannot be a stable key — a non-hashable, variadic/`**kwargs`,
//! zero-parameter, generic, or no-return function — is rejected with ZAP5056.

use std::collections::HashMap;

use zapc_diag::ErrorCode;
use zapc_sema::TypeError;
use zapc_syntax::ast::{
    Arg, ArgSpread, BinOp, Block, CompClause, Decorator, DecoratorDef, DecoratorMember, DictItem,
    ExceptClause, Expr, Field, Function, Item, MatchArm, Module, Param, Stmt, StructDef, Type,
    WithItem,
};
use zapc_syntax::span::Span;
use zapc_syntax::SHAREABLE_MARKER;

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

    // The built-in `@cache` wrapper (#666) is handled by this same pass, so it
    // must run whenever `@cache` appears anywhere — on a free function (to
    // expand it) or on a method/parameter (to report the ZAP5056 misuse) — even
    // with no user behavioral decorators at all.
    let has_cache = module.items.iter().any(|it| match it {
        Item::Function(f) => {
            f.decorators.iter().any(is_cache_deco)
                || f.params
                    .iter()
                    .any(|p| p.decorators.iter().any(is_cache_deco))
        }
        Item::Struct(s) => s
            .methods
            .iter()
            .any(|m| m.decorators.iter().any(is_cache_deco)),
        _ => false,
    });
    if behavioral.is_empty() && !has_cache {
        return diags;
    }

    // A behavioral decorator on anything but a free top-level function is not
    // supported in this slice (#651): report each such use precisely.
    report_unsupported_targets(module, &behavioral, &mut diags);
    // `@cache` is likewise a free-function decorator in v1 (#666); a method or
    // parameter application is reported here rather than reaching lowering.
    report_unsupported_cache_targets(module, &mut diags);

    // Rewrite the top-level function list, expanding each free function that
    // carries at least one behavioral decorator — or the built-in `@cache` — into
    // its `$orig` + wrapper chain (plus, for `@cache`, the generated store items).
    let mut new_items: Vec<Item> = Vec::with_capacity(module.items.len());
    for item in std::mem::take(&mut module.items) {
        let Item::Function(f) = item else {
            new_items.push(item);
            continue;
        };
        if f.decorators
            .iter()
            .all(|d| !behavioral.contains_key(&d.name) && !is_cache_deco(d))
        {
            new_items.push(Item::Function(f));
            continue;
        }
        expand_function(f, &behavioral, &mut new_items, &mut diags);
    }
    module.items = new_items;

    diags
}

/// The built-in memoization decorator name (#666, D-0115).
const CACHE_DECO: &str = "cache";

/// Whether a decorator application is the built-in `@cache`.
fn is_cache_deco(d: &Decorator) -> bool {
    d.name == CACHE_DECO
}

/// Report `@cache` applied to a struct method or a parameter (ZAP5056): v1
/// `@cache` decorates free top-level functions only. A struct/field/enum
/// application is caught by sema's `require_target` instead (the function-only
/// expansion below never consumes it, so it reaches sema).
fn report_unsupported_cache_targets(module: &Module, diags: &mut Vec<TypeError>) {
    for it in &module.items {
        match it {
            Item::Struct(s) => {
                for m in &s.methods {
                    for d in &m.decorators {
                        if is_cache_deco(d) {
                            diags.push(err(
                                ErrorCode::ZAP5056,
                                "@cache applies to a free function, not a method (#666); \
                                 memoize method results by hand over a module-level dict"
                                    .to_string(),
                                d.span,
                            ));
                        }
                    }
                }
            }
            Item::Function(f) => {
                for p in &f.params {
                    for d in &p.decorators {
                        if is_cache_deco(d) {
                            diags.push(err(
                                ErrorCode::ZAP5056,
                                "@cache applies to a function, not a parameter (#666)".to_string(),
                                d.span,
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }
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

    // Split decorators into the wrapper-generating ones to apply (source order:
    // user `before`/`after`/`wrap`, and the built-in `@cache`) and the metadata
    // ones kept on the outermost wrapper. Both kinds go through the one fold
    // below so they stack in correct bottom-up order (#666 relies on this).
    let mut applied: Vec<AppliedDeco> = Vec::new();
    let mut kept = Vec::new();
    for d in &f.decorators {
        if let Some(def) = behavioral.get(&d.name) {
            applied.push(AppliedDeco::Behavioral(def, d.args.clone()));
        } else if is_cache_deco(d) {
            if !d.args.is_empty() {
                diags.push(err(
                    ErrorCode::ZAP5056,
                    "@cache takes no arguments in v1: the cache is unbounded for the \
                     process lifetime (#666, D-0115)"
                        .to_string(),
                    d.span,
                ));
            }
            applied.push(AppliedDeco::Cache(d.span));
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

    // Fold the wrapper-generating decorators bottom-up: the innermost (last
    // written) is nearest the declaration and wraps the original first.
    let mut inner_name = orig_name;
    for (idx, ad) in applied.iter().enumerate().rev() {
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
        let wrapper = match ad {
            AppliedDeco::Behavioral(def, call_args) => build_wrapper(
                &wrapper_name,
                &real_name,
                &inner_name,
                &f,
                def,
                call_args,
                decorators,
                diags,
            ),
            // The built-in `@cache` wrapper also pushes its generated store items
            // (key-struct, `@shareable` cache-struct, module global) onto `out`.
            AppliedDeco::Cache(span) => build_cache_wrapper(
                &wrapper_name,
                &inner_name,
                &f,
                *span,
                decorators,
                out,
                diags,
            ),
        };
        out.push(Item::Function(wrapper));
        inner_name = wrapper_name;
    }
}

/// One applied wrapper-generating decorator, in source order: a user behavioral
/// decorator (`before`/`after`/`wrap`) carrying its call arguments, or the
/// built-in `@cache` (#666) carrying its application span.
enum AppliedDeco<'a> {
    Behavioral(&'a DecoratorDef, Vec<Arg>),
    Cache(Span),
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
    let mut wrap: Option<&DecoratorMember> = None;
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
                wrap = Some(m);
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

    // The general `wrap(call)` form (#653) owns the whole wrapper body: it
    // forwards to the inner function itself, so it is mutually exclusive with
    // the `before`/`after` convenience form.
    if let Some(wrap) = wrap {
        if before.is_some() || after.is_some() {
            diags.push(err(
                ErrorCode::ZAP5052,
                format!(
                    "decorator `@{}` declares a `wrap(call)` hook together with \
                     `before`/`after`; a decorator uses one form or the other, not both",
                    def.name
                ),
                wrap.span,
            ));
        }
        build_wrap_body(
            &mut stmts, wrap, def, real_name, inner_name, sig, &renames, diags,
        );
        return Function {
            decorators,
            name: wrapper_name.to_string(),
            generics: sig.generics.clone(),
            params: sig.params.clone(),
            ret: sig.ret.clone(),
            body: Block { stmts, span: sp },
            span: sp,
        };
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
            wrap: None,
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
            wrap: None,
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

/// Build the body of a `wrap(call)` wrapper (#653) by inlining the decorator's
/// `inner` function into the wrapper (which carries the decorated function's
/// exact signature). `call(*args)` is rewritten to a typed call of the chained
/// inner function (`inner_name`) forwarding the decorated function's actual
/// parameters. `stmts` already holds the decorator-argument bindings.
///
/// Decision (D-0036 / D-0108): a `wrap` is a **typed forwarding template**.
/// `*args` is a compile-time forwarding token, never a runtime varargs value —
/// so `call(*args)` forwards exactly the decorated function's typed parameters,
/// and reading/indexing/iterating `*args` is rejected (ZAP5052). Where the
/// inlined body cannot be proven to return on every path (e.g. `retry`'s loop,
/// whose last iteration always returns or re-raises but which the type system
/// cannot see through), an implicit fall-through guard raises `RuntimeError`,
/// so the wrapper type-checks like hand-written code and has defined behavior if
/// control ever reaches the end.
#[allow(clippy::too_many_arguments)]
fn build_wrap_body(
    stmts: &mut Vec<Stmt>,
    member: &DecoratorMember,
    def: &DecoratorDef,
    real_name: &str,
    inner_name: &str,
    sig: &Function,
    renames: &HashMap<String, String>,
    diags: &mut Vec<TypeError>,
) {
    let sp = sig.span;
    let (Some(wrap), Some(inner_body)) = (&member.wrap, &member.body) else {
        diags.push(err(
            ErrorCode::ZAP5052,
            format!("decorator `@{}`'s `wrap(call)` hook is malformed", def.name),
            member.span,
        ));
        return;
    };

    // `wrap` must take exactly one macro parameter, `call`.
    let call_name = match member.params.as_slice() {
        [c] => c.clone(),
        _ => {
            diags.push(err(
                ErrorCode::ZAP5052,
                format!(
                    "decorator `@{}`'s `wrap` hook must take exactly one parameter, `call`",
                    def.name
                ),
                member.span,
            ));
            return;
        }
    };

    // The nested `inner` must be `inner()` or a single `*args` forwarder.
    let args_name = match wrap.inner_params.as_slice() {
        [] => None,
        [p] if p.variadic => Some(p.name.clone()),
        _ => {
            diags.push(err(
                ErrorCode::ZAP5052,
                format!(
                    "decorator `@{}`'s nested `inner` must take a single `*args` forwarder \
                     (or no parameters); keyword collectors and extra parameters are not supported",
                    def.name
                ),
                wrap.span,
            ));
            return;
        }
    };

    // The `wrap` body must end with exactly `return <inner>`.
    let trailer_ok = matches!(
        wrap.trailer.as_slice(),
        [Stmt::Return { value: Some(Expr::Name(n, _)), .. }] if *n == wrap.inner_name
    );
    if !trailer_ok {
        diags.push(err(
            ErrorCode::ZAP5054,
            format!(
                "decorator `@{}`'s `wrap` hook must end with `return {}` \
                 (returning the nested function it defines)",
                def.name, wrap.inner_name
            ),
            wrap.span,
        ));
    }

    // Inline the inner body, rewriting `call(*args)` into a forwarding call.
    let wrap_ctx = WrapCtx {
        call: call_name,
        args: args_name,
        target: inner_name.to_string(),
        params: sig.params.clone(),
    };
    let mut rw = Rewriter {
        ctx: String::new(),
        result_from: None,
        result_to: RESULT_LOCAL.to_string(),
        renames,
        fn_name: real_name,
        wrap: Some(wrap_ctx),
        diags,
    };
    let body_start = stmts.len();
    for s in &inner_body.stmts {
        stmts.push(rw.stmt(s.clone()));
    }

    // Fall-through guard (see the decision above): only emitted when the body is
    // not provably terminating, so it is always appended to a live block.
    if sig.ret.is_some() && !stmts_terminate(&stmts[body_start..]) {
        stmts.push(fallthrough_guard(real_name, &def.name, sp));
    }
}

/// `raise RuntimeError("…")` — the implicit fall-through guard of a `wrap`
/// wrapper that declares a return type but whose body may not return (#653).
fn fallthrough_guard(fn_name: &str, deco: &str, sp: Span) -> Stmt {
    Stmt::Raise {
        value: Some(Expr::Call {
            callee: Box::new(Expr::Name("RuntimeError".to_string(), sp)),
            args: vec![Arg {
                name: None,
                value: Expr::Str {
                    value: format!("decorator `@{deco}` wrapper for `{fn_name}` returned no value"),
                    raw: false,
                    bytes: false,
                    fstring: false,
                    span: sp,
                    fstring_fields: Vec::new(),
                },
                spread: ArgSpread::None,
                span: sp,
            }],
            span: sp,
        }),
        cause: None,
        span: sp,
    }
}

/// Whether a statement sequence provably returns/raises on every path — a
/// conservative check (any uncertainty returns `false`) used only to decide
/// whether a `wrap` wrapper needs a fall-through guard.
fn stmts_terminate(stmts: &[Stmt]) -> bool {
    stmts.last().is_some_and(stmt_terminates)
}

fn block_terminates(b: &Block) -> bool {
    stmts_terminate(&b.stmts)
}

fn stmt_terminates(s: &Stmt) -> bool {
    match s {
        Stmt::Return { .. } | Stmt::Raise { .. } => true,
        Stmt::If {
            branches,
            else_block: Some(eb),
            ..
        } => branches.iter().all(|(_, b)| block_terminates(b)) && block_terminates(eb),
        Stmt::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|a| block_terminates(&a.body))
        }
        _ => false,
    }
}

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

/// The synthetic locals the `@cache` wrapper uses (unforgeable `$` names).
const KEY_LOCAL: &str = "$k";
const HIT_LOCAL: &str = "$hit";

/// Build the `@cache` wrapper for `sig` (#666, D-0115) and push the generated
/// store items — the `@derive(Hash, Eq)` key-struct, the `@shareable`
/// cache-struct, and the module global — onto `out`. `wrapper_name` is the name
/// this wrapper takes (the decorated function's own name when outermost);
/// `inner_name` is the chained callable it memoizes. `deco_span` anchors any
/// ZAP5056 rejection. On an un-memoizable signature it reports ZAP5056 and
/// returns a plain forwarding wrapper (compilation aborts on the diagnostic, but
/// the rest of the pass still sees a well-formed AST).
fn build_cache_wrapper(
    wrapper_name: &str,
    inner_name: &str,
    sig: &Function,
    deco_span: Span,
    decorators: Vec<Decorator>,
    out: &mut Vec<Item>,
    diags: &mut Vec<TypeError>,
) -> Function {
    let sp = sig.span;
    // The receiver (if any) is not part of the key.
    let params: Vec<&Param> = sig.params.iter().filter(|p| p.name != "self").collect();

    // Validate the signature is memoizable; collect every problem as ZAP5056.
    let mut bad = false;
    if !sig.generics.is_empty() {
        diags.push(err(
            ErrorCode::ZAP5056,
            "@cache does not support a generic function in v1 (#666): each \
             instantiation would need its own store"
                .to_string(),
            deco_span,
        ));
        bad = true;
    }
    if sig.ret.is_none() {
        diags.push(err(
            ErrorCode::ZAP5056,
            "@cache needs a function that returns a value to memoize (#666)".to_string(),
            deco_span,
        ));
        bad = true;
    }
    if params.is_empty() {
        diags.push(err(
            ErrorCode::ZAP5056,
            "@cache needs at least one parameter to key on (#666)".to_string(),
            deco_span,
        ));
        bad = true;
    }
    for p in &params {
        if p.variadic || p.kwargs {
            diags.push(err(
                ErrorCode::ZAP5056,
                format!(
                    "@cache cannot key on the variadic/**kwargs parameter '{}': a \
                     collector is not a stable cache key (#666)",
                    p.name
                ),
                deco_span,
            ));
            bad = true;
        } else if let Some(ty) = &p.ty {
            if !type_is_hashable_key(ty) {
                diags.push(err(
                    ErrorCode::ZAP5056,
                    format!(
                        "@cache parameter '{}' of type `{}` is not a hashable cache key \
                         (#666): collections and bare tuples cannot be dict keys",
                        p.name,
                        type_display(ty)
                    ),
                    deco_span,
                ));
                bad = true;
            }
        }
    }
    if bad {
        return forwarding_wrapper(wrapper_name, inner_name, sig, decorators);
    }

    let ret_ty = sig.ret.clone().expect("return type checked present above");
    let key_struct = format!("{wrapper_name}$Key");
    let cache_struct = format!("{wrapper_name}$Cache");
    let global = format!("{wrapper_name}$cache");

    // @derive(Hash, Eq) struct <wrapper>$Key: <one field per key parameter>
    let key_fields: Vec<Field> = params
        .iter()
        .map(|p| Field {
            decorators: Vec::new(),
            ty: p.ty.clone().expect("key params are typed"),
            name: p.name.clone(),
            bits: None,
            default: None,
            span: sp,
        })
        .collect();
    out.push(Item::Struct(StructDef {
        decorators: vec![Decorator {
            name: "derive".to_string(),
            args: vec![name_arg("Hash", sp), name_arg("Eq", sp)],
            span: sp,
        }],
        name: key_struct.clone(),
        generics: Vec::new(),
        fields: key_fields,
        methods: Vec::new(),
        is_value: false,
        span: sp,
    }));

    // @shareable struct <wrapper>$Cache: bytes lock; dict[<Key>, R] data
    out.push(Item::Struct(StructDef {
        decorators: vec![Decorator {
            name: SHAREABLE_MARKER.to_string(),
            args: Vec::new(),
            span: sp,
        }],
        name: cache_struct.clone(),
        generics: Vec::new(),
        fields: vec![
            Field {
                decorators: Vec::new(),
                ty: named_ty("bytes", sp),
                name: "lock".to_string(),
                bits: None,
                default: None,
                span: sp,
            },
            Field {
                decorators: Vec::new(),
                ty: dict_ty(&key_struct, ret_ty, sp),
                name: "data".to_string(),
                bits: None,
                default: None,
                span: sp,
            },
        ],
        methods: Vec::new(),
        is_value: false,
        span: sp,
    }));

    // <Cache> <global> = <Cache>(sync.mutex(), {})   — an immortal module global.
    out.push(Item::Stmt(Stmt::Let {
        is_const: false,
        ty: Some(named_ty(&cache_struct, sp)),
        name: global.clone(),
        value: Expr::Call {
            callee: Box::new(name_expr(&cache_struct, sp)),
            args: vec![
                Arg {
                    name: None,
                    value: sync_call_expr("mutex", Vec::new(), sp),
                    spread: ArgSpread::None,
                    span: sp,
                },
                Arg {
                    name: None,
                    value: Expr::Dict(Vec::new(), sp),
                    spread: ArgSpread::None,
                    span: sp,
                },
            ],
            span: sp,
        },
        span: sp,
    }));

    // --- the wrapper body ---
    let lock_field = || attr_expr(&global, "lock", sp);
    let data_field = || attr_expr(&global, "data", sp);
    let mut stmts: Vec<Stmt> = Vec::new();

    // auto $k = <Key>(p0, p1, ...)
    stmts.push(Stmt::Let {
        is_const: false,
        ty: None,
        name: KEY_LOCAL.to_string(),
        value: Expr::Call {
            callee: Box::new(name_expr(&key_struct, sp)),
            args: params
                .iter()
                .map(|p| Arg {
                    name: None,
                    value: name_expr(&p.name, sp),
                    spread: ArgSpread::None,
                    span: sp,
                })
                .collect(),
            span: sp,
        },
        span: sp,
    });

    // sync.lock(<global>.lock)
    stmts.push(sync_stmt("lock", lock_field(), sp));

    // if $k in <global>.data: { auto $hit = <global>.data[$k]; sync.unlock(...); return $hit }
    let hit_block = Block {
        stmts: vec![
            Stmt::Let {
                is_const: false,
                ty: None,
                name: HIT_LOCAL.to_string(),
                value: Expr::Index {
                    base: Box::new(data_field()),
                    index: Box::new(name_expr(KEY_LOCAL, sp)),
                    span: sp,
                },
                span: sp,
            },
            sync_stmt("unlock", lock_field(), sp),
            Stmt::Return {
                value: Some(name_expr(HIT_LOCAL, sp)),
                span: sp,
            },
        ],
        span: sp,
    };
    stmts.push(Stmt::If {
        branches: vec![(
            Expr::Compare {
                first: Box::new(name_expr(KEY_LOCAL, sp)),
                rest: vec![(BinOp::In, data_field())],
                span: sp,
            },
            hit_block,
        )],
        else_block: None,
        span: sp,
    });

    // sync.unlock(...)  — release before computing, so recursion cannot deadlock.
    stmts.push(sync_stmt("unlock", lock_field(), sp));

    // auto $result = inner(args)   — computed OUTSIDE the lock.
    stmts.push(Stmt::Let {
        is_const: false,
        ty: sig.ret.clone(),
        name: RESULT_LOCAL.to_string(),
        value: Expr::Call {
            callee: Box::new(name_expr(inner_name, sp)),
            args: forward_args(&sig.params, sp),
            span: sp,
        },
        span: sp,
    });

    // sync.lock(...); <global>.data[$k] = $result; sync.unlock(...)
    stmts.push(sync_stmt("lock", lock_field(), sp));
    stmts.push(Stmt::Assign {
        target: Expr::Index {
            base: Box::new(data_field()),
            index: Box::new(name_expr(KEY_LOCAL, sp)),
            span: sp,
        },
        value: name_expr(RESULT_LOCAL, sp),
        span: sp,
    });
    stmts.push(sync_stmt("unlock", lock_field(), sp));

    // return $result
    stmts.push(Stmt::Return {
        value: Some(name_expr(RESULT_LOCAL, sp)),
        span: sp,
    });

    Function {
        decorators,
        name: wrapper_name.to_string(),
        generics: Vec::new(),
        params: sig.params.clone(),
        ret: sig.ret.clone(),
        body: Block { stmts, span: sp },
        span: sp,
    }
}

/// A plain wrapper that just forwards to `inner_name` — the fallback emitted when
/// a `@cache` signature is rejected (ZAP5056), so the AST stays well-formed.
fn forwarding_wrapper(
    wrapper_name: &str,
    inner_name: &str,
    sig: &Function,
    decorators: Vec<Decorator>,
) -> Function {
    let sp = sig.span;
    let call = Expr::Call {
        callee: Box::new(name_expr(inner_name, sp)),
        args: forward_args(&sig.params, sp),
        span: sp,
    };
    let stmts = if sig.ret.is_some() {
        vec![Stmt::Return {
            value: Some(call),
            span: sp,
        }]
    } else {
        vec![Stmt::Expr(call)]
    };
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

/// Whether a parameter type can serve as a dict key (hashable). A collection
/// (`list`/`dict`/`set`), a bare tuple, or a function value cannot; everything
/// else (scalars, `str`, `bytes`, enums, user structs) is accepted — a user
/// struct without a hash surfaces as the usual `@derive(Hash)` requirement on
/// the generated key-struct.
fn type_is_hashable_key(ty: &Type) -> bool {
    match ty {
        Type::Named { name, .. } => !matches!(name.as_str(), "list" | "dict" | "set"),
        Type::Tuple { .. } | Type::Func { .. } => false,
    }
}

/// A compact rendering of a parameter type for the ZAP5056 message.
fn type_display(ty: &Type) -> String {
    match ty {
        Type::Named { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                let inner: Vec<String> = args.iter().map(type_display).collect();
                format!("{name}[{}]", inner.join(", "))
            }
        }
        Type::Tuple { elems, .. } => {
            let inner: Vec<String> = elems.iter().map(type_display).collect();
            format!("({})", inner.join(", "))
        }
        Type::Func { .. } => "fn(...)".to_string(),
    }
}

// --- small AST builders shared by the `@cache` expansion ---

fn name_expr(n: &str, sp: Span) -> Expr {
    Expr::Name(n.to_string(), sp)
}

fn attr_expr(base: &str, field: &str, sp: Span) -> Expr {
    Expr::Attr {
        base: Box::new(name_expr(base, sp)),
        name: field.to_string(),
        span: sp,
    }
}

fn name_arg(n: &str, sp: Span) -> Arg {
    Arg {
        name: None,
        value: name_expr(n, sp),
        spread: ArgSpread::None,
        span: sp,
    }
}

fn named_ty(n: &str, sp: Span) -> Type {
    Type::Named {
        name: n.to_string(),
        args: Vec::new(),
        span: sp,
    }
}

fn dict_ty(key: &str, val: Type, sp: Span) -> Type {
    Type::Named {
        name: "dict".to_string(),
        args: vec![named_ty(key, sp), val],
        span: sp,
    }
}

/// `sync.<method>(<args>)` as an expression.
fn sync_call_expr(method: &str, args: Vec<Arg>, sp: Span) -> Expr {
    Expr::Call {
        callee: Box::new(Expr::Attr {
            base: Box::new(name_expr("sync", sp)),
            name: method.to_string(),
            span: sp,
        }),
        args,
        span: sp,
    }
}

/// `sync.<method>(<arg>)` as an expression statement.
fn sync_stmt(method: &str, arg: Expr, sp: Span) -> Stmt {
    Stmt::Expr(sync_call_expr(
        method,
        vec![Arg {
            name: None,
            value: arg,
            spread: ArgSpread::None,
            span: sp,
        }],
        sp,
    ))
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
    /// `Some` only when rewriting a `wrap(call)` inner body (#653): rewrites
    /// `call(*args)` into a typed forwarding call and rejects `call`/`*args`
    /// used as runtime values.
    wrap: Option<WrapCtx>,
    diags: &'a mut Vec<TypeError>,
}

/// Context for rewriting a `wrap(call)` inner body (#653).
struct WrapCtx {
    /// The `wrap(call)` parameter name — the callable being wrapped.
    call: String,
    /// The inner `*args` forwarder name, if any (`None` for `def inner()`).
    args: Option<String>,
    /// The chained inner function the forwarded call targets (`f$orig` or a
    /// `f$decoN` wrapper).
    target: String,
    /// The decorated function's parameters, forwarded by a `call(*args)`.
    params: Vec<Param>,
}

impl Rewriter<'_> {
    /// Map a bare identifier used as a *value* in a hook body.
    fn map_name(&mut self, n: String, span: Span) -> Expr {
        if let Some(w) = &self.wrap {
            if n == w.call {
                self.diags.push(err(
                    ErrorCode::ZAP5052,
                    "a `wrap`'s `call` may be used only as `call(*args)`; it cannot be \
                     passed around or stored as a value"
                        .to_string(),
                    span,
                ));
                return Expr::Name(n, span);
            }
            if w.args.as_deref() == Some(n.as_str()) {
                self.diags.push(err(
                    ErrorCode::ZAP5052,
                    "`*args` is a compile-time forwarding template, not a runtime value: it \
                     cannot be read, indexed, or iterated — only forwarded via `call(*args)`"
                        .to_string(),
                    span,
                ));
                return Expr::Name(n, span);
            }
        }
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

    /// Rewrite a `call(...)` inside a `wrap` body into the forwarding call
    /// `inner_target(<decorated params…>)`. The source invocation must be
    /// `call(*args)` (or `call()` for a no-arg forwarder); anything else is
    /// ZAP5052. The forwarded arguments always come from the decorated
    /// function's signature, never from the written argument list.
    fn rewrite_wrap_call(&mut self, args: Vec<Arg>, span: Span) -> Expr {
        let (target, params, args_name) = {
            let w = self.wrap.as_ref().expect("wrap context");
            (w.target.clone(), w.params.clone(), w.args.clone())
        };
        let shape_ok = match args.as_slice() {
            [] => true,
            [a] => {
                a.name.is_none()
                    && matches!(a.spread, ArgSpread::Iterable)
                    && matches!(&a.value, Expr::Name(n, _) if Some(n.as_str()) == args_name.as_deref())
            }
            _ => false,
        };
        if !shape_ok {
            self.diags.push(err(
                ErrorCode::ZAP5052,
                "the wrapped `call` may be invoked only as `call(*args)` (or `call()`), \
                 forwarding the decorated function's parameters unchanged"
                    .to_string(),
                span,
            ));
        }
        Expr::Call {
            callee: Box::new(Expr::Name(target, span)),
            args: forward_args(&params, span),
            span,
        }
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
            Expr::Call { callee, args, span } => {
                // In a `wrap` body, `call(*args)` is the forwarding point: rewrite
                // it to a typed call of the chained inner function with the
                // decorated function's actual parameters.
                if let (Some(w), Expr::Name(n, _)) = (&self.wrap, callee.as_ref()) {
                    if *n == w.call {
                        return self.rewrite_wrap_call(args, span);
                    }
                }
                Expr::Call {
                    callee: Box::new(self.expr(*callee)),
                    args: args.into_iter().map(|a| self.arg(a)).collect(),
                    span,
                }
            }
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
    fn wrap_expands_to_forwarding_wrapper() {
        // `retry` wraps `fetch`: the wrapper keeps `fetch`'s signature, binds the
        // decorator arg, inlines `inner`, rewrites `call(*args)` to the typed
        // forwarding call `fetch$orig(url)`, and appends a fall-through guard.
        let src = "decorator retry(int64 times):\n    def wrap(call):\n        def inner(*args):\n            for i in range(times):\n                try:\n                    return call(*args)\n                except Error:\n                    if i == times - 1:\n                        raise\n        return inner\n\
                   @retry(times=3)\ndef fetch(str url) -> str:\n    return url\n";
        let mut m = module(src);
        let diags = expand_behavioral_decorators(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");

        // Original renamed, keeps its signature.
        let orig = func(&m, "fetch$orig").expect("renamed original");
        assert_eq!(orig.params.len(), 1);

        // Wrapper keeps the real name + exact signature; decorator gone.
        let w = func(&m, "fetch").expect("wrapper");
        assert!(w.decorators.is_empty());
        assert_eq!(w.params.len(), 1);
        assert_eq!(w.params[0].name, "url");
        assert!(
            matches!(&w.ret, Some(zapc_syntax::ast::Type::Named { name, .. }) if name == "str")
        );

        // First stmt binds the decorator arg `int64 times$arg = 3`.
        assert!(
            matches!(&w.body.stmts[0], Stmt::Let { name, .. } if name == "times$arg"),
            "arg bind: {:?}",
            w.body.stmts[0]
        );
        // Then the inlined `for` loop, then the appended fall-through guard.
        assert!(
            matches!(&w.body.stmts[1], Stmt::For { .. }),
            "for loop: {:?}",
            w.body.stmts[1]
        );
        assert!(
            matches!(w.body.stmts.last(), Some(Stmt::Raise { .. })),
            "fall-through guard: {:?}",
            w.body.stmts.last()
        );

        // `call(*args)` inside the loop became `fetch$orig(url)`.
        let src_dump = format!("{:?}", w.body.stmts[1]);
        assert!(
            src_dump.contains("fetch$orig"),
            "forwarding call targets the original: {src_dump}"
        );
        assert!(
            !src_dump.contains("\"call\""),
            "the `call` macro name is fully rewritten: {src_dump}"
        );
    }

    #[test]
    fn wrap_without_return_type_needs_no_guard() {
        let src = "decorator around:\n    def wrap(call):\n        def inner(*args):\n            print(\"before\")\n            call(*args)\n            print(\"after\")\n        return inner\n\
                   @around\ndef act(int64 x):\n    print(x)\n";
        let mut m = module(src);
        let diags = expand_behavioral_decorators(&mut m);
        assert!(diags.is_empty(), "{diags:?}");
        let w = func(&m, "act").expect("wrapper");
        // Void function: no fall-through guard appended.
        assert!(
            !matches!(w.body.stmts.last(), Some(Stmt::Raise { .. })),
            "no guard for a void wrapper: {:?}",
            w.body.stmts
        );
        let dump = format!("{:?}", w.body.stmts);
        assert!(dump.contains("act$orig"), "forwarding call present: {dump}");
    }

    #[test]
    fn wrap_rejects_args_used_as_value() {
        // `*args` is a forwarding template, not a value — reading it is ZAP5052.
        let src = "decorator m:\n    def wrap(call):\n        def inner(*args):\n            print(args)\n            return call(*args)\n        return inner\n\
                   @m\ndef f(int64 x) -> int64:\n    return x\n";
        let mut md = module(src);
        let diags = expand_behavioral_decorators(&mut md);
        assert!(
            diags.iter().any(|d| d.code == ErrorCode::ZAP5052),
            "args-as-value should report ZAP5052: {diags:?}"
        );
    }

    #[test]
    fn wrap_rejects_call_as_value() {
        let src = "decorator m:\n    def wrap(call):\n        def inner(*args):\n            auto c = call\n            return call(*args)\n        return inner\n\
                   @m\ndef f(int64 x) -> int64:\n    return x\n";
        let mut md = module(src);
        let diags = expand_behavioral_decorators(&mut md);
        assert!(
            diags.iter().any(|d| d.code == ErrorCode::ZAP5052),
            "call-as-value should report ZAP5052: {diags:?}"
        );
    }

    #[test]
    fn wrap_requires_return_inner() {
        let src = "decorator m:\n    def wrap(call):\n        def inner(*args):\n            return call(*args)\n        return 0\n\
                   @m\ndef f(int64 x) -> int64:\n    return x\n";
        let mut md = module(src);
        let diags = expand_behavioral_decorators(&mut md);
        assert!(
            diags.iter().any(|d| d.code == ErrorCode::ZAP5054),
            "missing `return inner` should report ZAP5054: {diags:?}"
        );
    }

    #[test]
    fn wrap_rejects_mixing_with_before() {
        let src = "decorator m:\n    def before(ctx):\n        print(ctx.name)\n    def wrap(call):\n        def inner(*args):\n            return call(*args)\n        return inner\n\
                   @m\ndef f(int64 x) -> int64:\n    return x\n";
        let mut md = module(src);
        let diags = expand_behavioral_decorators(&mut md);
        assert!(
            diags.iter().any(|d| d.code == ErrorCode::ZAP5052),
            "mixing wrap with before should report ZAP5052: {diags:?}"
        );
    }

    #[test]
    fn wrap_stacks_over_before_after() {
        // `@trace` (before/after) over `@retry` (wrap): the outer wrapper is the
        // before/after form calling `f`, chained to `f$deco1` (the wrap), chained
        // to `f$orig`.
        let src = "decorator trace:\n    def before(ctx):\n        print(\"enter\")\n    def after(ctx, result):\n        print(\"exit\")\n\
                   decorator retry(int64 times):\n    def wrap(call):\n        def inner(*args):\n            for i in range(times):\n                return call(*args)\n        return inner\n\
                   @trace\n@retry(times=2)\ndef f(int64 x) -> int64:\n    return x\n";
        let mut md = module(src);
        let diags = expand_behavioral_decorators(&mut md);
        assert!(diags.is_empty(), "{diags:?}");
        assert!(func(&md, "f").is_some(), "outer wrapper");
        assert!(func(&md, "f$deco1").is_some(), "middle wrap wrapper");
        assert!(func(&md, "f$orig").is_some(), "renamed original");
        // The middle (wrap) wrapper forwards to the original.
        let mid = format!("{:?}", func(&md, "f$deco1").unwrap().body.stmts);
        assert!(mid.contains("f$orig"), "wrap forwards to original: {mid}");
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

    // ---- @cache (I#666, D-0115) -------------------------------------------

    fn strukt<'a>(m: &'a Module, name: &str) -> Option<&'a StructDef> {
        m.items.iter().find_map(|it| match it {
            Item::Struct(s) if s.name == name => Some(s),
            _ => None,
        })
    }

    fn global_let<'a>(m: &'a Module, name: &str) -> Option<&'a Stmt> {
        m.items.iter().find_map(|it| match it {
            Item::Stmt(s @ Stmt::Let { name: n, .. }) if n == name => Some(s),
            _ => None,
        })
    }

    fn deco_names(decos: &[Decorator]) -> Vec<&str> {
        decos.iter().map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn cache_generates_store_items_and_wrapper() {
        let mut m = module(
            "@cache\ndef fib(int64 n) -> int64:\n    return n\ndef main():\n    print(fib(1))\n",
        );
        let diags = expand_behavioral_decorators(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");

        // Original renamed, body + signature intact, no decorators.
        let orig = func(&m, "fib$orig").expect("renamed original");
        assert_eq!(orig.params.len(), 1);
        assert!(orig.decorators.is_empty());

        // Key-struct: @derive(Hash, Eq), one field per key parameter.
        let key = strukt(&m, "fib$Key").expect("key struct");
        assert_eq!(deco_names(&key.decorators), vec!["derive"]);
        assert_eq!(key.fields.len(), 1);
        assert_eq!(key.fields[0].name, "n");
        let derive_traits: Vec<&str> = key.decorators[0]
            .args
            .iter()
            .filter_map(|a| match &a.value {
                Expr::Name(t, _) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(derive_traits, vec!["Hash", "Eq"]);

        // Cache-struct: @shareable, `lock` + `data` fields; `data` is a dict
        // keyed by the key-struct.
        let cache = strukt(&m, "fib$Cache").expect("cache struct");
        assert_eq!(deco_names(&cache.decorators), vec![SHAREABLE_MARKER]);
        assert_eq!(cache.fields.len(), 2);
        assert_eq!(cache.fields[0].name, "lock");
        assert_eq!(cache.fields[1].name, "data");
        assert!(matches!(
            &cache.fields[1].ty,
            Type::Named { name, args, .. }
                if name == "dict"
                    && matches!(&args[0], Type::Named { name, .. } if name == "fib$Key")
        ));

        // Module global of the cache-struct type, initialized once.
        assert!(global_let(&m, "fib$cache").is_some(), "cache global");

        // Wrapper keeps the real name + signature; `@cache` is consumed.
        let wrapper = func(&m, "fib").expect("wrapper");
        assert!(wrapper.decorators.is_empty(), "@cache consumed");
        assert_eq!(wrapper.params.len(), 1);
        assert!(matches!(&wrapper.ret, Some(Type::Named { name, .. }) if name == "int64"));
    }

    #[test]
    fn cache_multi_arg_key_struct_has_field_per_param() {
        let mut m = module(
            "@cache\ndef g(int64 a, str b) -> int64:\n    return a\ndef main():\n    print(g(1, \"x\"))\n",
        );
        let diags = expand_behavioral_decorators(&mut m);
        assert!(diags.is_empty(), "{diags:?}");
        let key = strukt(&m, "g$Key").expect("key struct");
        let names: Vec<&str> = key.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn cache_stacks_bottom_up_with_behavioral() {
        // `@logged` over `@cache`: Python `logged(cache(f))` — cache is innermost
        // (wraps the original), logged is the outer wrapper keeping the name.
        let mut m = module(&format!(
            "{LOGGED}@logged\n@cache\ndef sq(int64 n) -> int64:\n    return n * n\ndef main():\n    print(sq(2))\n"
        ));
        let diags = expand_behavioral_decorators(&mut m);
        assert!(diags.is_empty(), "{diags:?}");
        assert!(func(&m, "sq$orig").is_some(), "original");
        // The inner (cache) wrapper is a non-outer position → `$deco1`, and its
        // store items are named after it.
        assert!(func(&m, "sq$deco1").is_some(), "cache wrapper (inner)");
        assert!(strukt(&m, "sq$deco1$Key").is_some(), "cache key struct");
        assert!(global_let(&m, "sq$deco1$cache").is_some(), "cache global");
        let outer = func(&m, "sq").expect("outer wrapper");
        assert!(outer.decorators.is_empty(), "both decorators consumed");
    }

    /// The single ZAP5056 code for every un-memoizable `@cache` signature.
    fn cache_rejects(src: &str) -> Vec<TypeError> {
        let mut m = module(src);
        let diags = expand_behavioral_decorators(&mut m);
        assert!(
            diags.iter().all(|d| d.code == ErrorCode::ZAP5056),
            "expected only ZAP5056, got {diags:?}"
        );
        assert!(!diags.is_empty(), "expected a rejection for {src:?}");
        diags
    }

    #[test]
    fn cache_rejects_unmemoizable_signatures() {
        // Non-hashable (collection) parameter.
        cache_rejects("@cache\ndef f(list[int64] xs) -> int64:\n    return 0\n");
        // No return value to store.
        cache_rejects("@cache\ndef f(int64 n):\n    print(n)\n");
        // No parameter to key on.
        cache_rejects("@cache\ndef f() -> int64:\n    return 0\n");
        // Variadic collector is not a stable key.
        cache_rejects("@cache\ndef f(int64... ns) -> int64:\n    return 0\n");
        // Generic function would need a store per instantiation.
        cache_rejects("@cache\ndef f[T](T x) -> int64:\n    return 0\n");
        // `@cache` takes no arguments in v1.
        cache_rejects("@cache(10)\ndef f(int64 n) -> int64:\n    return n\n");
    }

    #[test]
    fn cache_on_method_is_rejected() {
        let diags = cache_rejects(
            "struct S:\n    int64 x\n    @cache\n    def m(self, int64 n) -> int64:\n        return n\n",
        );
        assert!(diags[0].message.contains("free function"));
    }

    #[test]
    fn cache_without_any_application_is_untouched() {
        // A file with no `@cache` and no behavioral decorator is left as-is.
        let mut m = module("def f(int64 n) -> int64:\n    return n\n");
        let before = m.items.len();
        let diags = expand_behavioral_decorators(&mut m);
        assert!(diags.is_empty());
        assert_eq!(m.items.len(), before, "nothing generated");
    }
}
