//! Registering function-decorator desugaring + reflection-driven handler
//! adapters (I#457/D-0087, I#466/D-0092).
//!
//! A **dotted** decorator name on a free function — `@app.get("/x")` — is a
//! *registering callable target*: the decorated function is handed to a method
//! on an in-scope value. We desugar it, as a whole-program AST pass run **before
//! sema**, into the registration call the receiver's method already accepts:
//!
//! ```text
//! @app.get("/x")                       auto app = App()
//! def h(Request r) -> Response:   ==>  app.get("/x", h)   # inserted after the
//!     ...                              def h(Request r) -> Response: ...
//! ```
//!
//! This is exactly the method-call registration surface fastzap v1 already
//! compiles and runs (`app.get(path, handler)`, zap#312), so `@app.get` becomes
//! pure sugar over it with no engine change. Because the pass runs before sema,
//! the synthesized call is type-checked like any other: an unknown receiver, a
//! missing method, or a wrong argument shape produces a precise error at the
//! call site rather than a blanket "unknown decorator" (ZAP5032).
//!
//! Every built-in / declared `annotation` / `decorator` is a **bare** name, so a
//! dotted decorator name is unambiguously a registering target; bare names are
//! left untouched for the declared-decorator metadata path (D-0036/D-0087 (1),
//! handled in lowering). This pass only rewrites free top-level functions;
//! methods, fields and structs keep their decorators.
//!
//! ## Reflection-driven adapters — `Depends(...)` injection (I#466, D-0092)
//!
//! When a decorated handler declares extra parameters with a `Depends(provider)`
//! default — the FastAPI dependency-injection form —
//!
//! ```text
//! @app.get("/items")
//! def list_items(Request req, DB db = Depends(get_db)) -> Response:
//!     return Response(db.conn)
//! ```
//!
//! its type is `fn(Request, DB) -> Response`, which does **not** match the
//! uniform `fn(Request) -> Response` seam (and is now correctly rejected, #465).
//! Instead of registering the handler directly we synthesize, at the decoration
//! site, a **statically-typed adapter** `fn(Request) -> Response`:
//!
//! ```text
//! def list_items$adapter(Request req) -> Response:
//!     DB get_db$dep = get_db()          # resolve Depends, deps-first, once each
//!     return list_items(req, get_db$dep)
//! ```
//!
//! and register *that*. Because this pass has the handler's full signature AST
//! and every provider is a top-level `def` in the same module, the provider
//! (wall 2) and the typed invocation (wall 3) are both recovered **at compile
//! time** — no value-reflection, no runtime annotation recovery, no dynamic
//! invoke. The adapter body is the per-request scope: each distinct provider is
//! resolved into exactly one local and reused, giving FastAPI's resolve-once-
//! per-request semantics for free; nested `Depends` in a provider's own
//! signature resolve recursively. Synthetic `$`-suffixed names cannot collide
//! with source identifiers (the lexer admits no `$`). A handler with no
//! `Depends` parameter is registered directly, unchanged.

use std::collections::{HashMap, HashSet};

use zapc_diag::ErrorCode;
use zapc_sema::TypeError;
use zapc_syntax::ast::{Arg, Block, Expr, Function, Item, Module, Param, Stmt, Type};
use zapc_syntax::span::Span;

/// One pending registration: the root receiver name whose binding we anchor the
/// call after (`app` in `app.get`), and the synthesized `recv.method(args, h)`
/// statement.
struct Registration {
    recv_root: String,
    stmt: Stmt,
}

/// Rewrite every registering (dotted) function decorator in `module` into its
/// registration call, synthesizing a `Depends`-resolving adapter for any
/// handler that declares injected parameters. In place; a no-op when there are
/// no registering decorators, so an ordinary program is unchanged. Returns any
/// diagnostics raised while building adapters (invalid `Depends`, ZAP5048).
pub fn desugar_registering_decorators(module: &mut Module) -> Vec<TypeError> {
    let mut regs: Vec<Registration> = Vec::new();
    let mut diags: Vec<TypeError> = Vec::new();

    // Snapshot every top-level function's signature up front, so adapter
    // synthesis can read provider signatures while we mutate items in place.
    let fn_sigs: HashMap<String, Vec<Param>> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Function(f) => Some((f.name.clone(), f.params.clone())),
            _ => None,
        })
        .collect();

    // Adapters synthesized this pass, appended to the module after the strip
    // loop (we cannot push while iterating items mutably). Keyed by handler name
    // so a handler with several registering decorators (e.g. get + post) reuses
    // one adapter rather than defining it twice.
    let mut adapters: Vec<Function> = Vec::new();
    let mut adapter_name: HashMap<String, String> = HashMap::new();

    // Pass 1: strip dotted decorators off free functions, building a registration
    // per decorator in source order (so routes register in declaration order).
    for item in &mut module.items {
        let Item::Function(f) = item else { continue };
        if f.decorators.iter().all(|d| !is_registering(&d.name)) {
            continue;
        }
        let fn_name = f.name.clone();

        // Does this handler declare injected (`Depends`) parameters? If so, the
        // thing we register is the adapter, not the handler. Synthesize it once.
        let has_depends = f
            .params
            .iter()
            .any(|p| matches!(classify_param(p), ParamKind::Depends { .. }));
        let registered_name = if has_depends {
            adapter_name
                .entry(fn_name.clone())
                .or_insert_with(|| {
                    let (adapter, aname) = synth_adapter(f, &fn_sigs, &mut diags);
                    adapters.push(adapter);
                    aname
                })
                .clone()
        } else {
            fn_name.clone()
        };

        let mut kept = Vec::with_capacity(f.decorators.len());
        for d in std::mem::take(&mut f.decorators) {
            let Some((recv_expr, method, recv_root)) = split_registering(&d.name, d.span) else {
                kept.push(d);
                continue;
            };
            // recv.method(<decorator args>, <registered fn value>)
            let mut args = d.args.clone();
            args.push(Arg {
                name: None,
                value: Expr::Name(registered_name.clone(), d.span),
                span: d.span,
            });
            let call = Expr::Call {
                callee: Box::new(Expr::Attr {
                    base: Box::new(recv_expr),
                    name: method,
                    span: d.span,
                }),
                args,
                span: d.span,
            };
            regs.push(Registration {
                recv_root,
                stmt: Stmt::Expr(call),
            });
        }
        f.decorators = kept;
    }

    // Splice synthesized adapters in as ordinary top-level functions. They are
    // referenced by name from the registration calls and type-checked by sema.
    module
        .items
        .extend(adapters.into_iter().map(Item::Function));

    if regs.is_empty() {
        return diags;
    }

    // Pass 2: splice each registration in right after its receiver's binding.
    // `main` is tried first (the common case — the receiver is a `main` local),
    // then every other function, so a receiver bound elsewhere still anchors.
    let order = fn_order_main_first(module);
    for fi in order {
        if regs.is_empty() {
            break;
        }
        if let Item::Function(f) = &mut module.items[fi] {
            splice_after_bindings(&mut f.body.stmts, &mut regs);
        }
    }

    // Any receiver never bound by a `let` lands at the top of `main`; sema then
    // reports a precise "unknown name" at the registration call rather than the
    // pass guessing. If there is no `main`, drop them at module start similarly.
    if !regs.is_empty() {
        let anchor = module
            .items
            .iter()
            .position(|it| matches!(it, Item::Function(f) if f.name == "main"))
            .or_else(|| {
                module
                    .items
                    .iter()
                    .position(|it| matches!(it, Item::Function(_)))
            });
        if let Some(fi) = anchor {
            if let Item::Function(f) = &mut module.items[fi] {
                for (k, reg) in regs.drain(..).enumerate() {
                    f.body.stmts.insert(k, reg.stmt);
                }
            }
        }
    }

    diags
}

/// How a handler/provider parameter is bound by the adapter.
enum ParamKind {
    /// A plain parameter, passed through positionally (the Request, etc.).
    PassThrough,
    /// `T x = Depends(provider)` — injected by resolving `provider`.
    Depends { provider: String },
    /// `Depends(...)` in a shape we cannot resolve; carries the diagnostic.
    BadDepends { message: String, span: Span },
}

/// Classify a parameter by its default. A `Depends(provider)` default marks an
/// injected parameter; the marker `Depends` is recognized here and consumed —
/// it is never a runtime value (I#466, D-0092).
fn classify_param(p: &Param) -> ParamKind {
    let Some(default) = &p.default else {
        return ParamKind::PassThrough;
    };
    let Expr::Call { callee, args, span } = default else {
        return ParamKind::PassThrough;
    };
    let Expr::Name(name, _) = callee.as_ref() else {
        return ParamKind::PassThrough;
    };
    if name != "Depends" {
        return ParamKind::PassThrough;
    }
    // From here on it is a `Depends(...)` marker: any malformed shape is an error
    // (ZAP5048), not a silent pass-through.
    if args.len() != 1 || args[0].name.is_some() {
        return ParamKind::BadDepends {
            message: format!(
                "`Depends(...)` on parameter `{}` takes exactly one positional \
                 provider function",
                p.name
            ),
            span: *span,
        };
    }
    match &args[0].value {
        Expr::Name(provider, _) => ParamKind::Depends {
            provider: provider.clone(),
        },
        _ => ParamKind::BadDepends {
            message: format!(
                "`Depends(...)` on parameter `{}` must name a provider function \
                 directly, e.g. `Depends(get_db)`",
                p.name
            ),
            span: *span,
        },
    }
}

/// Build the per-request adapter for a handler that declares injected
/// parameters. Returns the adapter `Function` and its name. The adapter's
/// parameters are the handler's pass-through parameters (so its type is the
/// seam's `fn(Request) -> Response`); its body resolves each `Depends` provider
/// into a local (deps-first, each provider once) and tail-calls the handler.
fn synth_adapter(
    handler: &Function,
    fn_sigs: &HashMap<String, Vec<Param>>,
    diags: &mut Vec<TypeError>,
) -> (Function, String) {
    let sp = handler.span;
    let adapter_name = format!("{}$adapter", handler.name);

    let mut builder = DepBuilder {
        fn_sigs,
        diags,
        resolved: HashMap::new(),
        lets: Vec::new(),
    };

    // Resolve each injected parameter and record the call argument to pass for
    // every handler parameter, in declaration order.
    let mut call_args: Vec<Arg> = Vec::with_capacity(handler.params.len());
    let mut pass_through: Vec<Param> = Vec::new();
    for p in &handler.params {
        match classify_param(p) {
            ParamKind::PassThrough => {
                // Flows straight through to the handler; also a parameter of the
                // adapter (its contribution to the seam signature).
                call_args.push(arg(Expr::Name(p.name.clone(), p.span)));
                pass_through.push(strip_default(p));
            }
            ParamKind::Depends { provider } => {
                let mut stack = HashSet::new();
                let local = builder.resolve(&provider, p.ty.clone(), p.span, &mut stack);
                call_args.push(arg(Expr::Name(local, p.span)));
            }
            ParamKind::BadDepends { message, span } => {
                builder.diags.push(err(message, span));
                // Keep the arg count right so the synthesized call is still
                // well-formed; the ZAP5048 above is the reported failure.
                call_args.push(arg(Expr::Name(p.name.clone(), p.span)));
                pass_through.push(strip_default(p));
            }
        }
    }

    // Body: the dependency `let`s (deps-first), then `return handler(args...)`.
    let mut stmts = builder.lets;
    let call = Expr::Call {
        callee: Box::new(Expr::Name(handler.name.clone(), sp)),
        args: call_args,
        span: sp,
    };
    stmts.push(Stmt::Return {
        value: Some(call),
        span: sp,
    });

    let adapter = Function {
        decorators: Vec::new(),
        name: adapter_name.clone(),
        generics: Vec::new(),
        params: pass_through,
        ret: handler.ret.clone(),
        body: Block { stmts, span: sp },
        span: sp,
    };
    (adapter, adapter_name)
}

/// Resolves `Depends` providers into dependency `let`s, one per distinct
/// provider (resolve-once-per-request), deps-first, with cycle detection.
struct DepBuilder<'a> {
    fn_sigs: &'a HashMap<String, Vec<Param>>,
    diags: &'a mut Vec<TypeError>,
    /// provider name -> the local it was resolved into (`<provider>$dep`).
    resolved: HashMap<String, String>,
    /// Emitted dependency `let`s, in resolution (deps-first) order.
    lets: Vec<Stmt>,
}

impl DepBuilder<'_> {
    /// Resolve `provider` into a local of type `ty` (the declared type of the
    /// parameter it injects), emitting its `let` once. `stack` holds the
    /// providers currently being resolved, for cycle detection. Returns the
    /// local's name.
    fn resolve(
        &mut self,
        provider: &str,
        ty: Option<Type>,
        span: Span,
        stack: &mut HashSet<String>,
    ) -> String {
        let local = format!("{provider}$dep");
        if self.resolved.contains_key(provider) {
            return local; // already resolved once this request — reuse.
        }
        if stack.contains(provider) {
            self.diags.push(err(
                format!("`Depends` dependency cycle through provider `{provider}`"),
                span,
            ));
            // Record it as resolved to break the recursion; the cycle diagnostic
            // is the reported failure.
            self.resolved.insert(provider.to_string(), local.clone());
            return local;
        }

        let Some(pparams) = self.fn_sigs.get(provider) else {
            self.diags.push(err(
                format!("`Depends(...)` provider `{provider}` is not a top-level function"),
                span,
            ));
            self.resolved.insert(provider.to_string(), local.clone());
            return local;
        };

        // Resolve the provider's own parameters first (deps-first). Clone the
        // small list we need so we don't hold a borrow of `self.fn_sigs` across
        // the recursive &mut self calls.
        let pparams: Vec<Param> = pparams.clone();
        stack.insert(provider.to_string());
        let mut provider_args: Vec<Arg> = Vec::with_capacity(pparams.len());
        for pp in &pparams {
            match classify_param(pp) {
                ParamKind::Depends { provider: inner } => {
                    let inner_local = self.resolve(&inner, pp.ty.clone(), pp.span, stack);
                    provider_args.push(arg(Expr::Name(inner_local, pp.span)));
                }
                ParamKind::PassThrough => {
                    // A provider parameter that is not itself `Depends(...)` would
                    // be a request-derived dependency, which v1 does not inject.
                    self.diags.push(err(
                        format!(
                            "`Depends(...)` provider `{provider}` parameter `{}` is not \
                             itself `Depends(...)`; request-derived dependencies are not \
                             injected in v1 — read them from the request in the handler",
                            pp.name
                        ),
                        pp.span,
                    ));
                }
                ParamKind::BadDepends { message, span } => {
                    self.diags.push(err(message, span));
                }
            }
        }
        stack.remove(provider);

        // Emit `<ty> <provider>$dep = provider(<resolved deps>)`.
        self.lets.push(Stmt::Let {
            is_const: false,
            ty,
            name: local.clone(),
            value: Expr::Call {
                callee: Box::new(Expr::Name(provider.to_string(), span)),
                args: provider_args,
                span,
            },
            span,
        });
        self.resolved.insert(provider.to_string(), local.clone());
        local
    }
}

/// A positional call argument around an expression.
fn arg(value: Expr) -> Arg {
    let span = expr_span(&value);
    Arg {
        name: None,
        value,
        span,
    }
}

/// A pass-through parameter, with any ordinary default dropped: the adapter is
/// only ever called through the seam with every argument supplied, and a default
/// does not change the function-value type, so keeping it would be noise.
fn strip_default(p: &Param) -> Param {
    Param {
        default: None,
        ..p.clone()
    }
}

fn err(message: String, span: Span) -> TypeError {
    TypeError {
        code: ErrorCode::ZAP5048,
        message,
        span,
    }
}

fn expr_span(e: &Expr) -> Span {
    match e {
        Expr::Name(_, s) => *s,
        Expr::Call { span, .. } => *span,
        _ => Span { start: 0, end: 0 },
    }
}

/// A decorator name is a registering target iff it is dotted (`recv.method`).
fn is_registering(name: &str) -> bool {
    name.contains('.')
}

/// Split `recv.method` into the receiver expression, the method name, and the
/// root receiver name used to find its binding. `a.b.get` → (`a.b`, `get`, `a`).
fn split_registering(name: &str, span: Span) -> Option<(Expr, String, String)> {
    let (recv_path, method) = name.rsplit_once('.')?;
    let mut segs = recv_path.split('.');
    let root = segs.next()?.to_string();
    let mut recv = Expr::Name(root.clone(), span);
    for seg in segs {
        recv = Expr::Attr {
            base: Box::new(recv),
            name: seg.to_string(),
            span,
        };
    }
    Some((recv, method.to_string(), root))
}

/// Item indices of top-level functions, `main` first.
fn fn_order_main_first(module: &Module) -> Vec<usize> {
    let mut main_idx = None;
    let mut rest = Vec::new();
    for (i, it) in module.items.iter().enumerate() {
        if let Item::Function(f) = it {
            if f.name == "main" {
                main_idx = Some(i);
            } else {
                rest.push(i);
            }
        }
    }
    let mut order = Vec::with_capacity(rest.len() + 1);
    order.extend(main_idx);
    order.extend(rest);
    order
}

/// Walk a function's top-level statements; after each `let`/`auto` binding whose
/// name is a pending registration's receiver, splice that receiver's calls in,
/// in source order, and remove them from `regs`.
fn splice_after_bindings(stmts: &mut Vec<Stmt>, regs: &mut Vec<Registration>) {
    let mut i = 0;
    while i < stmts.len() {
        if let Some(bound) = let_binding_name(&stmts[i]) {
            let mut taken = Vec::new();
            let mut remaining = Vec::with_capacity(regs.len());
            for reg in regs.drain(..) {
                if reg.recv_root == bound {
                    taken.push(reg.stmt);
                } else {
                    remaining.push(reg);
                }
            }
            *regs = remaining;
            let n = taken.len();
            for (k, stmt) in taken.into_iter().enumerate() {
                stmts.insert(i + 1 + k, stmt);
            }
            i += n;
        }
        i += 1;
    }
}

/// The name bound by a single-name `let`/`auto` declaration, if this statement
/// is one. Tuple destructuring and assignments are not treated as receiver
/// bindings (a registering receiver is a plain `auto app = …`).
fn let_binding_name(stmt: &Stmt) -> Option<String> {
    match stmt {
        Stmt::Let { name, .. } => Some(name.clone()),
        _ => None,
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

    fn main_stmts(m: &Module) -> &[Stmt] {
        for it in &m.items {
            if let Item::Function(f) = it {
                if f.name == "main" {
                    return &f.body.stmts;
                }
            }
        }
        panic!("no main");
    }

    fn func<'a>(m: &'a Module, name: &str) -> Option<&'a Function> {
        m.items.iter().find_map(|it| match it {
            Item::Function(f) if f.name == name => Some(f),
            _ => None,
        })
    }

    // The receiver + method of a registration-call statement, if it is one.
    fn registration(stmt: &Stmt) -> Option<(String, String)> {
        let Stmt::Expr(Expr::Call { callee, .. }) = stmt else {
            return None;
        };
        let Expr::Attr { base, name, .. } = callee.as_ref() else {
            return None;
        };
        let Expr::Name(recv, _) = base.as_ref() else {
            return None;
        };
        Some((recv.clone(), name.clone()))
    }

    #[test]
    fn dotted_decorator_desugars_to_registration_after_binding() {
        let mut m = module(
            "struct App:\n    def get(self, str path, fn()->int64 h):\n        print(path)\n\
             @app.get(\"/x\")\ndef h() -> int64:\n    return 0\n\
             def main():\n    auto app = App()\n    print(\"ready\")\n",
        );
        let diags = desugar_registering_decorators(&mut m);
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");

        // The dotted decorator is stripped off `h`.
        for it in &m.items {
            if let Item::Function(f) = it {
                assert!(f.decorators.is_empty(), "decorator should be removed");
            }
        }

        // A `app.get(...)` registration is spliced right after `auto app = ...`.
        let stmts = main_stmts(&m);
        assert!(matches!(&stmts[0], Stmt::Let { name, .. } if name == "app"));
        assert_eq!(
            registration(&stmts[1]),
            Some(("app".to_string(), "get".to_string())),
            "registration call must follow the receiver binding"
        );
        // The handler `h` is appended as the trailing argument.
        if let Stmt::Expr(Expr::Call { args, .. }) = &stmts[1] {
            assert_eq!(args.len(), 2, "path + handler");
            assert!(matches!(&args[1].value, Expr::Name(n, _) if n == "h"));
        } else {
            panic!("expected a registration call");
        }
    }

    #[test]
    fn multiple_routes_keep_declaration_order() {
        let mut m = module(
            "struct App:\n    def get(self, str p, fn()->int64 h):\n        print(p)\n\
             @app.get(\"/a\")\ndef ha() -> int64:\n    return 1\n\
             @app.get(\"/b\")\ndef hb() -> int64:\n    return 2\n\
             def main():\n    auto app = App()\n",
        );
        desugar_registering_decorators(&mut m);
        let stmts = main_stmts(&m);
        // app binding, then /a registration, then /b registration.
        assert_eq!(registration(&stmts[1]), Some(("app".into(), "get".into())));
        assert_eq!(registration(&stmts[2]), Some(("app".into(), "get".into())));
        let first = match &stmts[1] {
            Stmt::Expr(Expr::Call { args, .. }) => &args[0].value,
            _ => panic!(),
        };
        assert!(matches!(first, Expr::Str { value, .. } if value == "/a"));
    }

    #[test]
    fn bare_decorator_is_left_untouched() {
        let mut m = module(
            "decorator audited:\n    pass\n\
             @audited\ndef h() -> int64:\n    return 0\n\
             def main():\n    print(\"x\")\n",
        );
        desugar_registering_decorators(&mut m);
        // The bare `@audited` is not a registering target; it stays on `h` for
        // the declared-decorator metadata path, and main is unchanged.
        let kept = m
            .items
            .iter()
            .any(|it| matches!(it, Item::Function(f) if f.name == "h" && f.decorators.len() == 1));
        assert!(kept, "bare decorator must be preserved");
        assert_eq!(main_stmts(&m).len(), 1);
    }

    // ---- Depends adapters (I#466, D-0092) ----------------------------------

    // Extract the `let`s and the final `return <callee>(args)` of a function.
    fn body(f: &Function) -> &[Stmt] {
        &f.body.stmts
    }

    #[test]
    fn depends_handler_registers_a_synthesized_adapter() {
        let mut m = module(
            "struct Req:\n    str path\n\
             struct DB:\n    int64 conn\n\
             struct Resp:\n    int64 status\n\
             def get_db() -> DB:\n    return DB(1)\n\
             struct App:\n    def get(self, str p, fn(Req)->Resp h):\n        print(p)\n\
             @app.get(\"/items\")\ndef list_items(Req req, DB db = Depends(get_db)) -> Resp:\n    return Resp(db.conn)\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m);
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");

        // The registration passes the ADAPTER, not the handler.
        let stmts = main_stmts(&m);
        let trailing = match &stmts[1] {
            Stmt::Expr(Expr::Call { args, .. }) => &args.last().unwrap().value,
            _ => panic!("expected registration"),
        };
        assert!(
            matches!(trailing, Expr::Name(n, _) if n == "list_items$adapter"),
            "registration must bind the adapter"
        );

        // The adapter exists, takes only the pass-through param, returns Resp.
        let adapter = func(&m, "list_items$adapter").expect("adapter synthesized");
        assert_eq!(adapter.params.len(), 1, "only the Request passes through");
        assert_eq!(adapter.params[0].name, "req");
        assert!(matches!(&adapter.ret, Some(Type::Named { name, .. }) if name == "Resp"));

        // Body: `DB get_db$dep = get_db()` then `return list_items(req, get_db$dep)`.
        let b = body(adapter);
        assert!(
            matches!(&b[0], Stmt::Let { name, ty: Some(Type::Named{name: tn, ..}), .. } if name == "get_db$dep" && tn == "DB"),
            "first stmt resolves the dependency: {:?}",
            b[0]
        );
        let Stmt::Return {
            value: Some(Expr::Call { callee, args, .. }),
            ..
        } = &b[1]
        else {
            panic!("expected tail return call: {:?}", b[1]);
        };
        assert!(matches!(callee.as_ref(), Expr::Name(n, _) if n == "list_items"));
        assert_eq!(args.len(), 2);
        assert!(matches!(&args[0].value, Expr::Name(n, _) if n == "req"));
        assert!(matches!(&args[1].value, Expr::Name(n, _) if n == "get_db$dep"));
    }

    #[test]
    fn nested_depends_resolves_deps_first() {
        let mut m = module(
            "struct Req:\n    str path\n\
             struct Settings:\n    int64 n\n\
             struct DB:\n    int64 conn\n\
             struct Resp:\n    int64 status\n\
             def get_settings() -> Settings:\n    return Settings(7)\n\
             def get_db(Settings s = Depends(get_settings)) -> DB:\n    return DB(s.n)\n\
             struct App:\n    def get(self, str p, fn(Req)->Resp h):\n        print(p)\n\
             @app.get(\"/x\")\ndef hx(Req req, DB db = Depends(get_db)) -> Resp:\n    return Resp(db.conn)\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m);
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "hx$adapter").expect("adapter");
        let b = body(adapter);
        // deps-first: settings resolved before db.
        assert!(matches!(&b[0], Stmt::Let { name, .. } if name == "get_settings$dep"));
        assert!(matches!(&b[1], Stmt::Let { name, .. } if name == "get_db$dep"));
        // get_db is called with the resolved settings local.
        let Stmt::Let {
            value: Expr::Call { args, .. },
            ..
        } = &b[1]
        else {
            panic!("expected db let call");
        };
        assert_eq!(args.len(), 1);
        assert!(matches!(&args[0].value, Expr::Name(n, _) if n == "get_settings$dep"));
    }

    #[test]
    fn shared_provider_resolved_once_per_request() {
        // Two parameters depend on the same provider: it is resolved into one
        // local and reused (resolve-once-per-request).
        let mut m = module(
            "struct Req:\n    str path\n\
             struct DB:\n    int64 conn\n\
             struct Resp:\n    int64 status\n\
             def get_db() -> DB:\n    return DB(1)\n\
             struct App:\n    def get(self, str p, fn(Req)->Resp h):\n        print(p)\n\
             @app.get(\"/x\")\ndef hx(Req req, DB a = Depends(get_db), DB b = Depends(get_db)) -> Resp:\n    return Resp(a.conn)\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m);
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "hx$adapter").expect("adapter");
        let b = body(adapter);
        let lets = b.iter().filter(|s| matches!(s, Stmt::Let { .. })).count();
        assert_eq!(lets, 1, "shared provider resolved exactly once");
        // Both handler args reference the same local.
        let Stmt::Return {
            value: Some(Expr::Call { args, .. }),
            ..
        } = b.last().unwrap()
        else {
            panic!("expected tail return");
        };
        assert!(matches!(&args[1].value, Expr::Name(n, _) if n == "get_db$dep"));
        assert!(matches!(&args[2].value, Expr::Name(n, _) if n == "get_db$dep"));
    }

    #[test]
    fn unknown_provider_is_a_precise_error() {
        let mut m = module(
            "struct Req:\n    str path\n\
             struct DB:\n    int64 conn\n\
             struct Resp:\n    int64 status\n\
             struct App:\n    def get(self, str p, fn(Req)->Resp h):\n        print(p)\n\
             @app.get(\"/x\")\ndef hx(Req req, DB db = Depends(missing)) -> Resp:\n    return Resp(db.conn)\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m);
        assert_eq!(diags.len(), 1, "one diagnostic");
        assert_eq!(diags[0].code, ErrorCode::ZAP5048);
        assert!(diags[0].message.contains("missing"));
    }

    #[test]
    fn provider_with_non_depends_param_is_rejected() {
        let mut m = module(
            "struct Req:\n    str path\n\
             struct DB:\n    int64 conn\n\
             struct Resp:\n    int64 status\n\
             def get_db(int64 n) -> DB:\n    return DB(n)\n\
             struct App:\n    def get(self, str p, fn(Req)->Resp h):\n        print(p)\n\
             @app.get(\"/x\")\ndef hx(Req req, DB db = Depends(get_db)) -> Resp:\n    return Resp(db.conn)\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, ErrorCode::ZAP5048);
        assert!(diags[0].message.contains("request-derived"));
    }

    #[test]
    fn depends_cycle_is_detected() {
        let mut m = module(
            "struct Req:\n    str path\n\
             struct A:\n    int64 n\n\
             struct Resp:\n    int64 status\n\
             def pa(A x = Depends(pb)) -> A:\n    return x\n\
             def pb(A x = Depends(pa)) -> A:\n    return x\n\
             struct App:\n    def get(self, str p, fn(Req)->Resp h):\n        print(p)\n\
             @app.get(\"/x\")\ndef hx(Req req, A a = Depends(pa)) -> Resp:\n    return Resp(a.n)\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m);
        assert!(
            diags
                .iter()
                .any(|d| d.code == ErrorCode::ZAP5048 && d.message.contains("cycle")),
            "expected a cycle diagnostic: {diags:?}"
        );
    }

    #[test]
    fn handler_without_depends_registers_directly() {
        let mut m = module(
            "struct Req:\n    str path\n\
             struct Resp:\n    int64 status\n\
             struct App:\n    def get(self, str p, fn(Req)->Resp h):\n        print(p)\n\
             @app.get(\"/x\")\ndef hx(Req req) -> Resp:\n    return Resp(0)\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m);
        assert!(diags.is_empty());
        assert!(
            func(&m, "hx$adapter").is_none(),
            "no adapter for a plain handler"
        );
        let stmts = main_stmts(&m);
        let trailing = match &stmts[1] {
            Stmt::Expr(Expr::Call { args, .. }) => &args.last().unwrap().value,
            _ => panic!(),
        };
        assert!(matches!(trailing, Expr::Name(n, _) if n == "hx"));
    }
}
