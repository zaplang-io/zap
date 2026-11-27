//! Registering function-decorator desugaring + reflection-driven handler
//! adapters (I#457/D-0087, I#466/D-0092, I#959/D-0198).
//!
//! The adapter also injects **request-derived** parameters (I#959, D-0198): a
//! handler parameter whose default calls a `@request_source`-annotated resolver
//! (e.g. `str id = Path()`) is resolved by synthesizing `str id = Path(req,
//! "id")` and dropping the parameter from the seam, composing with `Depends`.
//! The core recognizes only the annotation, never the resolver names, so the
//! source vocabulary stays framework-defined. See `classify_param` /
//! `synth_adapter` and the `ParamKind::Source` arm.
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
use zapc_syntax::ast::{Arg, ArgSpread, Block, Expr, Function, Item, Module, Param, Stmt, Type};
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
pub fn desugar_registering_decorators(
    module: &mut Module,
    import_prefixes: &HashMap<String, String>,
) -> Vec<TypeError> {
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

    // Each top-level function's generic type-parameter names, keyed like `fn_sigs`
    // (#1647). A decorator receiver whose handler parameter is typed as one of its
    // own generics is a *uniform generic seam* — the innermost use adapts the
    // handler into it, an outer use threads the previous layer's return value, so
    // any layer may be innermost.
    let fn_generics: HashMap<String, Vec<String>> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Function(f) => Some((
                f.name.clone(),
                f.generics.iter().map(|g| g.name.clone()).collect(),
            )),
            _ => None,
        })
        .collect();

    // Snapshot the request-source resolver functions (#959, D-0198): top-level
    // `def`s carrying `@request_source`. A handler parameter whose default calls
    // one of these is a request-sourced parameter the adapter injects, rather
    // than a seam pass-through. Taken before any decorator stripping; the marker
    // is a bare (non-registering) decorator, so the resolver is never rewritten.
    let source_fns: HashSet<String> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Function(f) if f.decorators.iter().any(|d| d.name == "request_source") => {
                Some(f.name.clone())
            }
            _ => None,
        })
        .collect();

    // Adapters synthesized this pass, appended to the module after the strip
    // loop (we cannot push while iterating items mutably). Keyed by handler name
    // so a handler with several registering decorators (e.g. get + post) reuses
    // one adapter rather than defining it twice.
    let mut adapters: Vec<Function> = Vec::new();
    let mut adapter_name: HashMap<String, String> = HashMap::new();

    // Object-binding decorators (#1509): whether a dotted decorator *rebinds* the
    // decorated name to its receiver method's returned object (Click command
    // objects) — as opposed to registering-and-discarding — is decided by that
    // method's declared return type (mechanism A, return-type-driven; recorded in
    // decisions/lang/1509-decorator-binds-object.md). We read the return type
    // from the already-linked AST before the mutable loop: a map of every
    // struct's method return types, and a map of each resolvable receiver
    // variable to the struct type it holds. A void method, or a receiver whose
    // type we cannot resolve here, keeps the registering behavior.
    let struct_ret = collect_struct_method_rets(module);
    let struct_names: HashSet<String> = struct_ret.keys().cloned().collect();
    let recv_struct = collect_receiver_struct_types(module, &struct_names);
    // The declared type of each struct method's TRAILING parameter (#1544): the
    // handler parameter a returning decorator receives. When it is the uniform
    // argparse seam `fn(Namespace) -> R`, the decorated handler (of any signature)
    // is adapted into that seam rather than passed raw. Imported-module receivers
    // read the same from `fn_sigs` below.
    let struct_method_params = collect_struct_method_params(module);
    let struct_method_generics = collect_struct_method_generics(module);

    // Imported-module receivers (#1519, the cli#4 surface): a dotted decorator
    // whose root is an IMPORTED module (`import factory; @factory.command`)
    // resolves to the package's exported FREE FUNCTION, not a struct method. The
    // merge already rewrote every ordinary `factory.command(x)` call to the
    // mangled symbol (`__zm1_factory__command`), but a decorator name is a plain
    // string the merge leaves untouched, so we resolve it here from the entry
    // module's import alias → mangle-prefix map (`import_prefixes`) plus a
    // snapshot of every top-level function's declared return type (keyed by its
    // already-mangled name). The same return-type-driven rule (mechanism A) then
    // decides rebind (non-void) vs register (void), exactly as for a local
    // struct receiver. Taken before the mutable loop renames the decorated `def`.
    let fn_rets: HashMap<String, Option<Type>> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Function(f) => Some((f.name.clone(), f.ret.clone())),
            _ => None,
        })
        .collect();

    // Pass 1: strip dotted decorators off free functions, building a registration
    // per decorator in source order (so routes register in declaration order).
    for item in &mut module.items {
        let Item::Function(f) = item else { continue };
        if f.decorators.iter().all(|d| !is_registering(&d.name)) {
            continue;
        }

        // Object-binding path (#1509): if the OUTERMOST dotted decorator targets
        // a receiver method with a non-void return type, rebind the decorated
        // name to the (possibly stacked) decorator chain's returned object,
        // renaming the original function to `<name>$impl` (the typed callable the
        // decorator receives). A void or unresolvable method falls through to the
        // registering logic below, unchanged.
        if let Some((reg, adapter)) = try_build_rebind(
            f,
            &struct_ret,
            &recv_struct,
            import_prefixes,
            &fn_rets,
            &struct_method_params,
            &struct_method_generics,
            &fn_sigs,
            &fn_generics,
            &source_fns,
            &mut diags,
        ) {
            if let Some(a) = adapter {
                adapters.push(a);
            }
            regs.push(reg);
            continue;
        }

        let fn_name = f.name.clone();

        // Does this handler declare injected parameters — a `Depends(...)`
        // provider, or a `@request_source` request-derived source (#959)? If so,
        // the thing we register is the adapter, not the handler. Synthesize once.
        let needs_adapter = f.params.iter().any(|p| {
            matches!(
                classify_param(p, &source_fns),
                ParamKind::Depends { .. } | ParamKind::Source { .. } | ParamKind::BadSource { .. }
            )
        });
        let registered_name = if needs_adapter {
            adapter_name
                .entry(fn_name.clone())
                .or_insert_with(|| {
                    let (adapter, aname) = synth_adapter(f, &fn_sigs, &source_fns, &mut diags);
                    adapters.push(adapter);
                    aname
                })
                .clone()
        } else {
            fn_name.clone()
        };

        let mut kept = Vec::with_capacity(f.decorators.len());
        for d in std::mem::take(&mut f.decorators) {
            // A bare (non-dotted) decorator is a declared annotation/decorator —
            // left for the metadata path, untouched.
            if !is_registering(&d.name) {
                kept.push(d);
                continue;
            }
            // Resolve the dotted receiver to its call form: a local struct
            // method (`recv.method`, an `Attr` callee) or an imported-module free
            // function (`__zmN_pkg__method`, a bare `Name` callee, #1519). An
            // unresolvable receiver (truly unknown) falls back to the literal
            // `recv.method` form so sema reports a precise "unknown name" at the
            // synthesized call rather than this pass guessing.
            let (callee, recv_root) = match resolve_dotted(
                &d.name,
                d.span,
                &recv_struct,
                &struct_ret,
                import_prefixes,
                &fn_rets,
                &struct_method_params,
                &struct_method_generics,
                &fn_sigs,
                &fn_generics,
            ) {
                Some(r) => (r.callee, r.root),
                None => {
                    let (recv_expr, method, root) =
                        split_registering(&d.name, d.span).expect("a dotted decorator name splits");
                    (
                        Expr::Attr {
                            base: Box::new(recv_expr),
                            name: method,
                            span: d.span,
                        },
                        root,
                    )
                }
            };
            // recv.method(<decorator args>, <registered fn value>)
            let mut args = d.args.clone();
            args.push(Arg {
                name: None,
                value: Expr::Name(registered_name.clone(), d.span),
                spread: ArgSpread::None,
                span: d.span,
            });
            let call = Expr::Call {
                callee: Box::new(callee),
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

/// Map each struct's method name to its declared return type (`None` = void),
/// read from the already-linked module AST. Used by the object-binding
/// decorator path (#1509) to decide, before sema, whether a dotted decorator
/// rebinds the decorated name to its returned object (non-void) or registers
/// and discards (void). A later method with the same name on the same struct
/// (not legal, but harmless here) simply overwrites — sema is the authority.
fn collect_struct_method_rets(module: &Module) -> HashMap<String, HashMap<String, Option<Type>>> {
    let mut out: HashMap<String, HashMap<String, Option<Type>>> = HashMap::new();
    for it in &module.items {
        if let Item::Struct(s) = it {
            let methods = out.entry(s.name.clone()).or_default();
            for m in &s.methods {
                methods.insert(m.name.clone(), m.ret.clone());
            }
        }
    }
    out
}

/// The declared type of each struct method's TRAILING parameter (#1544), keyed
/// `struct -> method -> type`. A returning decorator hands the decorated handler
/// to its receiver method as that method's last argument, so the last
/// parameter's type is what decides whether an argparse-backed adapter is
/// synthesized (`fn(Namespace) -> R`) or the raw handler is passed. A method with
/// no explicit trailing parameter type maps to `None`.
/// A snapshot of struct methods' parameters, keyed `struct -> method ->
/// [(param name, param type)]`, with the implicit `self` dropped (#1626).
type MethodParams = HashMap<String, HashMap<String, Vec<(String, Option<Type>)>>>;

fn collect_struct_method_params(module: &Module) -> MethodParams {
    let mut out: MethodParams = HashMap::new();
    for it in &module.items {
        if let Item::Struct(s) = it {
            let methods = out.entry(s.name.clone()).or_default();
            for m in &s.methods {
                // Drop the implicit `self` receiver: a decorator call supplies no
                // argument for it, so the stored parameters align with the call's
                // arguments (and with the handler the desugar passes).
                let params = m
                    .params
                    .iter()
                    .filter(|p| p.name != "self")
                    .map(|p| (p.name.clone(), p.ty.clone()))
                    .collect();
                methods.insert(m.name.clone(), params);
            }
        }
    }
    out
}

/// Each struct method's generic type-parameter names (#1647), keyed as
/// `struct -> method -> [generic names]`, mirroring [`collect_struct_method_params`].
/// Used to recognize a *uniform generic seam* parameter (one typed as the method's
/// own generic).
type MethodGenerics = HashMap<String, HashMap<String, Vec<String>>>;

fn collect_struct_method_generics(module: &Module) -> MethodGenerics {
    let mut out: MethodGenerics = HashMap::new();
    for it in &module.items {
        if let Item::Struct(s) = it {
            let methods = out.entry(s.name.clone()).or_default();
            for m in &s.methods {
                methods.insert(
                    m.name.clone(),
                    m.generics.iter().map(|g| g.name.clone()).collect(),
                );
            }
        }
    }
    out
}

/// Map a receiver variable name to the struct type it holds, for the forms we
/// can resolve syntactically before sema: a typed parameter or `let`
/// (`Cli cli`), or an `auto cli = Cli()` whose initializer calls a known
/// struct's constructor. First binding found wins; a receiver we cannot resolve
/// this way (e.g. an imported module value — the cli#4 extension) is simply
/// absent, and its decorator keeps the registering behavior. Only top-level
/// function statements are scanned, matching where [`splice_after_bindings`]
/// can anchor the synthesized binding.
fn collect_receiver_struct_types(
    module: &Module,
    structs: &HashSet<String>,
) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for it in &module.items {
        let Item::Function(f) = it else { continue };
        for p in &f.params {
            if let Some(Type::Named { name, .. }) = &p.ty {
                if structs.contains(name) {
                    out.entry(p.name.clone()).or_insert_with(|| name.clone());
                }
            }
        }
        for stmt in &f.body.stmts {
            let Stmt::Let {
                name, ty, value, ..
            } = stmt
            else {
                continue;
            };
            if let Some(Type::Named { name: tn, .. }) = ty {
                if structs.contains(tn) {
                    out.entry(name.clone()).or_insert_with(|| tn.clone());
                }
            } else if let Expr::Call { callee, .. } = value {
                if let Expr::Name(ctor, _) = callee.as_ref() {
                    if structs.contains(ctor) {
                        out.entry(name.clone()).or_insert_with(|| ctor.clone());
                    }
                }
            }
        }
    }
    out
}

/// Object-binding desugar (#1509). If `f`'s OUTERMOST dotted decorator targets a
/// receiver method with a non-void declared return type, this is a *replacing*
/// decorator: rebind the decorated name to the returned object. All of `f`'s
/// dotted decorators compose, bottom-up, into one nested call chain whose
/// innermost argument is the original function (renamed `<name>$impl`, kept as
/// the typed callable the decorator receives); stacked `@option`/`@argument`
/// arguments ride along as the library methods' arguments, so their metadata is
/// retained for a later dispatcher. Returns the `auto <name> = <chain>` splice
/// (anchored on the outermost receiver's root, reusing [`splice_after_bindings`])
/// and mutates `f` in place (rename + strip dotted decorators). Returns `None`
/// — leaving `f` untouched for the registering logic — when the outermost method
/// is void or its receiver/return type cannot be resolved here.
#[allow(clippy::too_many_arguments)]
fn try_build_rebind(
    f: &mut Function,
    struct_ret: &HashMap<String, HashMap<String, Option<Type>>>,
    recv_struct: &HashMap<String, String>,
    import_prefixes: &HashMap<String, String>,
    fn_rets: &HashMap<String, Option<Type>>,
    struct_method_params: &MethodParams,
    struct_method_generics: &MethodGenerics,
    fn_sigs: &HashMap<String, Vec<Param>>,
    fn_generics: &HashMap<String, Vec<String>>,
    source_fns: &HashSet<String>,
    diags: &mut Vec<TypeError>,
) -> Option<(Registration, Option<Function>)> {
    // The outermost dotted decorator (first in source order) decides the form
    // and produces the value bound to the name.
    let d0 = f.decorators.iter().find(|d| is_registering(&d.name))?;
    let r0 = resolve_dotted(
        &d0.name,
        d0.span,
        recv_struct,
        struct_ret,
        import_prefixes,
        fn_rets,
        struct_method_params,
        struct_method_generics,
        fn_sigs,
        fn_generics,
    )?;
    // Non-void return → rebind; void or unknown/unresolvable receiver →
    // registering (mechanism A, return-type-driven).
    match r0.ret {
        Some(Some(_)) => {}
        _ => return None,
    }

    let orig_name = f.name.clone();
    let impl_name = format!("{orig_name}$impl");

    // Dotted decorators are consumed into the binding chain; any bare decorator
    // stays on the renamed implementation function (the behavioral-decorator
    // pass, which runs next, still sees it there).
    let mut dotted = Vec::new();
    let mut kept = Vec::new();
    for d in std::mem::take(&mut f.decorators) {
        if is_registering(&d.name) {
            dotted.push(d);
        } else {
            kept.push(d);
        }
    }
    f.decorators = kept;

    // #1544: the INNERMOST dotted decorator (closest to `def`, last listed) is the
    // one handed the decorated handler. When its receiver method declares that
    // trailing parameter as the uniform argparse seam `fn(Namespace) -> R`, the
    // handler — of ANY signature — is adapted into that seam: a synthesized
    // `<name>$adapter(Namespace) -> R` reads each handler parameter from the
    // namespace by name (type-matched getter) and calls the handler in signature
    // order. The chain then threads the adapter, not the raw handler. Any other
    // trailing type (e.g. the handler's own fixed `fn(str) -> str`, #1527) passes
    // the raw implementation, unchanged.
    //
    // The seam parameter may appear ANYWHERE among the receiver method's
    // parameters, not only last (#1626) — e.g. `command(str name="",
    // fn(Namespace) handler)` or `command(fn(Namespace) handler, str name="")`.
    // So the synthesized adapter is passed to the handler parameter *by name*,
    // leaving the decorator's optional named metadata (`name=`, `help=`) to bind
    // by default or keyword as usual.
    let mut adapter: Option<Function> = None;
    let mut base_name = impl_name.clone();
    let mut handler_param: Option<String> = None;
    if let Some(innermost) = dotted.last() {
        let ri = resolve_dotted(
            &innermost.name,
            innermost.span,
            recv_struct,
            struct_ret,
            import_prefixes,
            fn_rets,
            struct_method_params,
            struct_method_generics,
            fn_sigs,
            fn_generics,
        );
        if let Some(seam) = ri.as_ref().and_then(|r| seam_param(&r.params, &r.generics)) {
            let (pname, context_ty) = match seam.kind {
                // A literal `fn([C,] Namespace)` seam carries its context type.
                SeamKind::Literal(ctx) => (seam.name, ctx),
                // A uniform generic seam (#1647) carries no context type; infer it
                // from the handler's own first non-scalar parameter (as #1621 does).
                SeamKind::Generic => (seam.name, infer_handler_context(&f.params)),
            };
            let (a, aname) = synth_namespace_adapter(
                &orig_name,
                &impl_name,
                &f.params,
                &f.ret,
                context_ty.as_ref(),
                fn_sigs,
                source_fns,
                f.span,
                diags,
            );
            base_name = aname;
            adapter = Some(a);
            handler_param = Some(pname);
        }
    }

    f.name = impl_name;

    // Build the chain innermost-first: start from the renamed implementation (or
    // the synthesized adapter), then wrap with each dotted decorator from the one
    // closest to `def` (last listed) outward to the outermost (first listed). Each
    // wrap is the receiver's call form — `recv.method(<args...>, <inner>)` for a
    // local struct method, or `<mangled free fn>(<args...>, <inner>)` for an
    // imported module function (#1519) — with the decorator's own arguments
    // (stacked `@option`/`@argument` metadata) kept as that call's leading
    // arguments.
    //
    // The INNERMOST wrap (the one handed the handler, processed first) passes the
    // synthesized adapter to the handler parameter **by name** when the seam named
    // it (#1626), so optional metadata parameters keep their defaults regardless
    // of declaration order. When that decorator carries POSITIONAL metadata around
    // a NON-LAST handler parameter, those positionals are routed onto the
    // receiver's parameter names *skipping the handler's slot* (#1645), so a
    // positional never lands on — or double-binds — the handler parameter. A raw
    // handler (no seam) and the outer wraps (which thread the composed object) stay
    // the trailing positional argument.
    let mut chain = Expr::Name(base_name, f.span);
    let n_dotted = dotted.len();
    for (i, d) in dotted.iter().rev().enumerate() {
        let Some(rd) = resolve_dotted(
            &d.name,
            d.span,
            recv_struct,
            struct_ret,
            import_prefixes,
            fn_rets,
            struct_method_params,
            struct_method_generics,
            fn_sigs,
            fn_generics,
        ) else {
            continue;
        };
        let mut args = match (i, handler_param.as_deref()) {
            // Innermost decorator with a named handler seam: pass the adapter by
            // the handler parameter's name and route any positional metadata around
            // its slot (#1626/#1645).
            (0, Some(hp)) => innermost_wrap_args(&d.args, &rd.params, hp, chain, d.span),
            // A raw handler (no seam) or an outer wrap: the inner value is the
            // trailing positional argument, unchanged.
            _ => {
                let mut args = d.args.clone();
                args.push(Arg {
                    name: None,
                    value: chain,
                    spread: ArgSpread::None,
                    span: d.span,
                });
                args
            }
        };
        // #1665: the OUTERMOST decorator (processed last) defaults an omitted
        // `str name` parameter to the decorated declaration's name (Click-style
        // `@command` naming), so a package can recover the decl name when `name=`
        // is not given.
        if i + 1 == n_dotted {
            fill_decl_name(&mut args, &rd.params, &orig_name, d.span);
        }
        chain = Expr::Call {
            callee: Box::new(rd.callee),
            args,
            span: d.span,
        };
    }

    Some((
        Registration {
            recv_root: r0.root,
            stmt: Stmt::Let {
                is_const: false,
                ty: None,
                name: orig_name,
                value: chain,
                span: f.span,
            },
        },
        adapter,
    ))
}

/// Build the innermost decorator's call arguments when the synthesized handler
/// reaches its parameter *by name* (#1626), routing positional metadata around a
/// non-last handler slot (#1645).
///
/// The synthesized `adapter` is always passed to the handler parameter
/// `handler_param` by keyword, so optional named metadata (`name=`/`help=`) keeps
/// its defaults regardless of the handler's position. The decorator's own
/// arguments (`decorator_args`) are kept; keyword metadata binds by its own name.
///
/// Positional metadata needs care only when a positional would reach the handler's
/// slot — i.e. there are more leading positionals than the handler's parameter
/// index. Then every positional from the handler's index onward is bound by the
/// receiver parameter's *name*, shifted one past the handler (`params[pos + 1]`),
/// so the handler's slot is skipped and no positional lands on (or double-binds)
/// it. Positionals before the handler's index keep their position, and when no
/// positional reaches the handler the arguments are kept verbatim — so the
/// handler-last and keyword-only-metadata cases (#1544/#1626) are byte-for-byte
/// unchanged.
fn innermost_wrap_args(
    decorator_args: &[Arg],
    params: &[(String, Option<Type>)],
    handler_param: &str,
    adapter: Expr,
    span: Span,
) -> Vec<Arg> {
    let handler_kw = Arg {
        name: Some(handler_param.to_string()),
        value: adapter,
        spread: ArgSpread::None,
        span,
    };
    let handler_idx = params.iter().position(|(n, _)| n == handler_param);
    // Leading positional metadata arguments (Zap requires positionals before
    // keywords, so the leading run is the full positional count).
    let positional = decorator_args
        .iter()
        .take_while(|a| a.name.is_none())
        .count();
    // Only route around the handler when a positional would reach or pass its slot.
    let needs_remap = handler_idx.is_some_and(|idx| positional > idx);
    if !needs_remap {
        let mut args = decorator_args.to_vec();
        args.push(handler_kw);
        return args;
    }
    let handler_idx = handler_idx.expect("needs_remap implies a handler index");
    let mut args: Vec<Arg> = Vec::with_capacity(decorator_args.len() + 1);
    let mut pos = 0usize; // ordinal among the decorator's positional arguments
    for a in decorator_args {
        if a.name.is_some() {
            // Keyword metadata binds by its own name — unchanged.
            args.push(a.clone());
            continue;
        }
        if pos < handler_idx {
            // Before the handler's slot: keep the positional in place.
            args.push(a.clone());
        } else {
            // At or beyond the handler's slot: bind by the receiver parameter's
            // name, shifted one past the handler so its slot is skipped. An
            // overflow (more positionals than parameters) is a user arity error
            // left to sema; keep it positional so the argument count is preserved.
            match params.get(pos + 1) {
                Some((pname, _)) => args.push(Arg {
                    name: Some(pname.clone()),
                    value: a.value.clone(),
                    spread: a.spread,
                    span: a.span,
                }),
                None => args.push(a.clone()),
            }
        }
        pos += 1;
    }
    args.push(handler_kw);
    args
}

/// Default an OMITTED `str name` parameter of the outermost returning decorator to
/// the decorated declaration's name `decl_name` (#1665, Click-style `@command`
/// naming). Fires only when the receiver declares a `str name` parameter and the
/// call binds it neither by the `name=` keyword nor by a leading positional in its
/// slot — an explicit name always wins, and a decorator with no `str name`
/// parameter is left unchanged. Appended as a keyword argument so it never
/// collides with the by-name handler or the threaded composed object.
fn fill_decl_name(
    args: &mut Vec<Arg>,
    params: &[(String, Option<Type>)],
    decl_name: &str,
    span: Span,
) {
    let Some(name_idx) = params.iter().position(|(n, t)| {
        n == "name"
            && matches!(t, Some(Type::Named { name, args, .. }) if name == "str" && args.is_empty())
    }) else {
        return;
    };
    // Bound by keyword, or by a leading positional reaching the `name` slot? Then
    // the caller supplied it — leave it.
    if args.iter().any(|a| a.name.as_deref() == Some("name")) {
        return;
    }
    let leading_positional = args.iter().take_while(|a| a.name.is_none()).count();
    if leading_positional > name_idx {
        return;
    }
    args.push(Arg {
        name: Some("name".to_string()),
        value: str_lit(decl_name, span),
        spread: ArgSpread::None,
        span,
    });
}

/// Classify the trailing-parameter type `t` of a returning decorator's receiver
/// method as the uniform argparse handler seam (#1544/#1621), the signal that the
/// decorated handler should be adapted rather than passed verbatim. The seam's
/// LAST function parameter is `Namespace`:
///
/// - `fn(Namespace) -> R`  ⇒ `Some(None)` — no context (#1544).
/// - `fn(C, Namespace) -> R` ⇒ `Some(Some(C))` — an explicit typed context `C`
///   supplied by the dispatcher as the handler's first parameter (#1621, cli#7).
///
/// Any other shape (not a function type, last parameter not `Namespace`, or more
/// than one leading parameter) is `None` — not a seam, so the raw handler is
/// passed, unchanged.
fn namespace_seam(t: &Option<Type>) -> Option<Option<Type>> {
    let Some(Type::Func { params, .. }) = t else {
        return None;
    };
    let last = params.last()?;
    let is_ns = matches!(last, Type::Named { name, args, .. }
        if name == "Namespace" && args.is_empty());
    if !is_ns {
        return None;
    }
    match params.len() {
        1 => Some(None),
        2 => Some(Some(params[0].clone())),
        _ => None,
    }
}

/// How a handler parameter's declared type binds from the argparse `Namespace`
/// (#1544/#1625/#1655): the typed getter to call, and an optional conversion
/// builtin to wrap its result in when the parameter is narrower than the getter's
/// type. The `Namespace` exposes `get_int` (`int64`), `get_float` (`float64`),
/// `get_flag` (`bool`), `get_str` (`str`), and the non-numeric `get_char` (`char`),
/// `get_bytes` (`bytes`, the argument's UTF-8 encoding), `get_bigint` (`bigint`)
/// and `get_complex` (`complex`) — the last two parse the argument string (#1657).
/// A narrower numeric scalar reads the wide getter and converts, e.g. `int32 n` →
/// `int32(ns.get_int("n"))`, `float32 r` → `float32(ns.get_float("r"))`; a
/// non-numeric scalar reads its dedicated getter directly (no wrap).
///
/// `None` for any other type — a parameter that cannot be bound from the command
/// line in v1 (no `Context`/struct binding, and `int8`/`int16`, which are not
/// scalar value types in Zap, epic #1644/#1654), reported at synthesis.
fn namespace_getter_for(t: &Option<Type>) -> Option<(&'static str, Option<&'static str>)> {
    let Some(Type::Named { name, args, .. }) = t else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    match name.as_str() {
        "int64" | "int" | "i64" => Some(("get_int", None)),
        "int32" | "i32" => Some(("get_int", Some("int32"))),
        "uint8" | "u8" => Some(("get_int", Some("uint8"))),
        "uint16" | "u16" => Some(("get_int", Some("uint16"))),
        "uint32" | "u32" => Some(("get_int", Some("uint32"))),
        "uint64" | "u64" => Some(("get_int", Some("uint64"))),
        "usize" => Some(("get_int", Some("usize"))),
        "float64" | "float" | "f64" => Some(("get_float", None)),
        "float32" | "f32" => Some(("get_float", Some("float32"))),
        "bool" => Some(("get_flag", None)),
        "str" => Some(("get_str", None)),
        // Non-numeric scalars read a dedicated typed getter that returns the final
        // type directly, so no conversion wrapper is needed (#1655). `char` is a
        // single-codepoint parse, `bytes` the argument's UTF-8 encoding.
        "char" => Some(("get_char", None)),
        "bytes" => Some(("get_bytes", None)),
        // `bigint`/`complex` parse the argument string (#1657); the getter returns
        // the final type directly, like `char`/`bytes`.
        "bigint" => Some(("get_bigint", None)),
        "complex" => Some(("get_complex", None)),
        _ => None,
    }
}

/// Synthesize the argparse-backed handler adapter (#1544, #1621). Given a handler
/// kept as `impl_name` with the declared `params`/`ret`, build
/// `def <orig>$adapter([C $ctx, ]Namespace $ns) -> R:` whose body reads each
/// handler parameter from the namespace by name with the type-matched getter and
/// calls the handler in signature order (`return <impl>(...)`, or a bare call for
/// a void handler).
///
/// When `context_ty` is `Some(C)` the seam is `fn(C, Namespace)`: the adapter
/// takes a leading context parameter of type `C` that the dispatcher supplies
/// directly, and the handler's **first** parameter, when it is a non-argv type
/// (not a namespace scalar), receives that context value rather than a namespace
/// read (#1621, cli#7 — "a handler may declare `Context ctx` as its first
/// parameter"). The context is optional on the handler: a handler whose first
/// parameter is a scalar binds every parameter from the namespace and the context
/// argument is unused.
///
/// A parameter that is neither the context nor an argv-sourced scalar is a
/// decoration-site error (ZAP5048); the synthesized call keeps its arity so the
/// module stays well-formed and the error is the reported failure.
#[allow(clippy::too_many_arguments)]
fn synth_namespace_adapter(
    orig_name: &str,
    impl_name: &str,
    params: &[Param],
    ret: &Option<Type>,
    context_ty: Option<&Type>,
    fn_sigs: &HashMap<String, Vec<Param>>,
    source_fns: &HashSet<String>,
    span: Span,
    diags: &mut Vec<TypeError>,
) -> (Function, String) {
    let adapter_name = format!("{orig_name}$adapter");
    let ns = "$ns";
    let ctx = "$ctx";
    // Reuse the `Depends(...)` resolver (D-0092/#466): an injected parameter is
    // resolved deps-first, once each, into a `<provider>$dep` local emitted before
    // the handler call (#1646). The builder owns the diagnostics for its lifetime.
    let mut builder = DepBuilder {
        fn_sigs,
        source_fns,
        diags,
        resolved: HashMap::new(),
        lets: Vec::new(),
    };
    let mut call_args: Vec<Arg> = Vec::with_capacity(params.len());
    for (i, p) in params.iter().enumerate() {
        match classify_param(p, source_fns) {
            // An injected dependency `T x = Depends(provider)`: resolved from its
            // provider in the adapter body, never read from the namespace (#1646).
            ParamKind::Depends { provider } => {
                let mut stack = HashSet::new();
                let local = builder.resolve(&provider, p.ty.clone(), p.span, &mut stack);
                call_args.push(arg(Expr::Name(local, p.span)));
            }
            // `@request_source` injection resolves against a request value, which a
            // command-line handler does not have — a clear error, not a miscompile.
            ParamKind::Source { .. } => {
                builder.diags.push(err(
                    format!(
                        "`@request_source` parameter `{}` is not available in an \
                         argparse-backed handler: there is no request value to resolve it \
                         against; use `Depends(...)` or a command-line scalar instead",
                        p.name
                    ),
                    p.span,
                ));
                call_args.push(arg(Expr::Name(p.name.clone(), span)));
            }
            ParamKind::BadDepends { message, span: s }
            | ParamKind::BadSource { message, span: s } => {
                builder.diags.push(err(message, s));
                // Keep the arity right so the synthesized call stays well-formed.
                call_args.push(arg(Expr::Name(p.name.clone(), span)));
            }
            // An ordinary parameter (no injection marker): bind it from the
            // namespace (a scalar), supply the dispatcher context (the first,
            // non-scalar parameter under a context seam, #1621), or reject it.
            ParamKind::PassThrough => {
                let is_context =
                    i == 0 && context_ty.is_some() && namespace_getter_for(&p.ty).is_none();
                if is_context {
                    call_args.push(arg(Expr::Name(ctx.to_string(), span)));
                    continue;
                }
                match namespace_getter_for(&p.ty) {
                    Some((getter, convert)) => {
                        // `ns.get_<kind>("<param>")`, wrapped in a conversion to the
                        // declared type when it is narrower than the getter's (#1625).
                        let read = Expr::Call {
                            callee: Box::new(Expr::Attr {
                                base: Box::new(Expr::Name(ns.to_string(), span)),
                                name: getter.to_string(),
                                span,
                            }),
                            args: vec![arg(str_lit(&p.name, span))],
                            span,
                        };
                        let value = match convert {
                            Some(conv) => Expr::Call {
                                callee: Box::new(Expr::Name(conv.to_string(), span)),
                                args: vec![arg(read)],
                                span,
                            },
                            None => read,
                        };
                        call_args.push(arg(value));
                    }
                    None => {
                        builder.diags.push(err(
                            format!(
                                "handler parameter `{}` has a type that cannot be bound from the \
                                 command-line namespace; an argparse-backed handler takes scalar \
                                 parameters (bool, str, char, bytes, bigint, complex, the integer \
                                 types int/int32/uint8..uint64/usize, and float/float32), an \
                                 optional typed context as its first parameter, and injected \
                                 `Depends(...)` parameters",
                                p.name
                            ),
                            p.span,
                        ));
                        // Keep the arity right so the synthesized call is well-formed;
                        // the diagnostic above is the reported failure.
                        call_args.push(arg(Expr::Name(p.name.clone(), span)));
                    }
                }
            }
        }
    }
    // Body: the dependency `let`s (deps-first), then the handler call.
    let mut stmts = builder.lets;
    let call = Expr::Call {
        callee: Box::new(Expr::Name(impl_name.to_string(), span)),
        args: call_args,
        span,
    };
    stmts.push(match ret {
        Some(_) => Stmt::Return {
            value: Some(call),
            span,
        },
        None => Stmt::Expr(call),
    });
    let ns_param = Param {
        decorators: Vec::new(),
        ty: Some(Type::Named {
            name: "Namespace".to_string(),
            args: Vec::new(),
            span,
        }),
        name: ns.to_string(),
        default: None,
        variadic: false,
        kwargs: false,
        keyword_only: false,
        positional_only: false,
        span,
    };
    // A context seam prepends the typed context parameter, carrying the seam's
    // declared context type verbatim so sema checks the handler's context
    // parameter against it.
    let mut adapter_params = Vec::with_capacity(2);
    if let Some(cty) = context_ty {
        adapter_params.push(Param {
            decorators: Vec::new(),
            ty: Some(cty.clone()),
            name: ctx.to_string(),
            default: None,
            variadic: false,
            kwargs: false,
            keyword_only: false,
            positional_only: false,
            span,
        });
    }
    adapter_params.push(ns_param);
    let adapter = Function {
        decorators: Vec::new(),
        name: adapter_name.clone(),
        generics: Vec::new(),
        params: adapter_params,
        ret: ret.clone(),
        body: Block { stmts, span },
        span,
    };
    (adapter, adapter_name)
}

/// A dotted decorator's resolved receiver: the call form it desugars to and the
/// declared return type that decides rebind vs register (mechanism A).
struct ResolvedRecv {
    /// The callee expression for the synthesized call: `recv.method` (an `Attr`,
    /// for a local struct receiver) or a bare, already-mangled free-function
    /// `Name` (for an imported-module receiver, #1519).
    callee: Expr,
    /// The receiver root used to anchor the splice. For a local struct receiver
    /// this is the variable name (the splice lands right after its `let`); for an
    /// imported module it is the import alias, which is never `let`-bound, so the
    /// splice lands at the top of `main` (the fallback) — exactly where the
    /// decorated name is used.
    root: String,
    /// The target's declared return type: `None` = the symbol could not be
    /// resolved (unknown receiver); `Some(None)` = void; `Some(Some(T))` =
    /// non-void. Only a non-void target rebinds.
    ret: Option<Option<Type>>,
    /// The receiver method's parameters `(name, type)` in declaration order, with
    /// the implicit `self` dropped (#1544/#1626). The handler parameter — the one
    /// typed as the uniform argparse seam `fn([C,] Namespace) -> R` — may appear
    /// anywhere among them (e.g. before optional `name=`/`help=` metadata), so the
    /// decorated handler is adapted and passed to it *by name*, not positionally.
    params: Vec<(String, Option<Type>)>,
    /// The receiver method's own generic type-parameter names (#1647). A parameter
    /// typed as one of these is a *uniform generic seam*.
    generics: Vec<String>,
}

/// Resolve a dotted decorator name `root.method` to the call it desugars to.
///
/// A **local struct receiver** (`cli` bound to a `Cli` value) resolves to the
/// struct method `recv.method` and reads the method's return type from the
/// struct's declared methods. An **imported-module receiver** (`factory`, an
/// import alias in `import_prefixes`) resolves to the package's exported free
/// function under its merged, mangled name `{prefix}{method}`, reading that
/// function's return type from `fn_rets` (#1519). A struct receiver is tried
/// first, so a local value shadows an import of the same name. Returns `None`
/// when the root is neither — the receiver is unresolvable here, and the caller
/// keeps the registering behavior (no guess, no crash), unchanged by #1519.
#[allow(clippy::too_many_arguments)]
fn resolve_dotted(
    name: &str,
    span: Span,
    recv_struct: &HashMap<String, String>,
    struct_ret: &HashMap<String, HashMap<String, Option<Type>>>,
    import_prefixes: &HashMap<String, String>,
    fn_rets: &HashMap<String, Option<Type>>,
    struct_method_params: &MethodParams,
    struct_method_generics: &MethodGenerics,
    fn_sigs: &HashMap<String, Vec<Param>>,
    fn_generics: &HashMap<String, Vec<String>>,
) -> Option<ResolvedRecv> {
    let (recv_expr, method, root) = split_registering(name, span)?;
    if let Some(struct_name) = recv_struct.get(&root) {
        let ret = struct_ret
            .get(struct_name)
            .and_then(|mm| mm.get(&method))
            .cloned();
        let params = struct_method_params
            .get(struct_name)
            .and_then(|mm| mm.get(&method))
            .cloned()
            .unwrap_or_default();
        let generics = struct_method_generics
            .get(struct_name)
            .and_then(|mm| mm.get(&method))
            .cloned()
            .unwrap_or_default();
        return Some(ResolvedRecv {
            callee: Expr::Attr {
                base: Box::new(recv_expr),
                name: method,
                span,
            },
            root,
            ret,
            params,
            generics,
        });
    }
    if let Some(prefix) = import_prefixes.get(&root) {
        let fn_name = format!("{prefix}{method}");
        let ret = fn_rets.get(&fn_name).cloned();
        // An imported-module receiver is a merged free function — no `self` — so
        // its parameters map directly.
        let params = fn_sigs
            .get(&fn_name)
            .map(|ps| {
                ps.iter()
                    .filter(|p| p.name != "self")
                    .map(|p| (p.name.clone(), p.ty.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let generics = fn_generics.get(&fn_name).cloned().unwrap_or_default();
        return Some(ResolvedRecv {
            callee: Expr::Name(fn_name, span),
            root,
            ret,
            generics,
            params,
        });
    }
    None
}

/// What kind of handler seam a receiver method's parameter is.
enum SeamKind {
    /// A literal `fn([C,] Namespace) -> R` seam (#1544/#1621), carrying the
    /// dispatcher-supplied context type `C`, if any.
    Literal(Option<Type>),
    /// A **uniform generic seam** (#1647): the parameter is typed as one of the
    /// receiver method's own generic type parameters, so one signature serves every
    /// stack position — the innermost use adapts the handler into it, an outer use
    /// threads the previous layer's return value. The context is inferred from the
    /// handler (the seam carries none).
    Generic,
}

/// The receiver method's handler seam — the parameter name and its kind.
struct SeamInfo {
    name: String,
    kind: SeamKind,
}

/// Find the receiver method's handler seam parameter among `params` (`self` already
/// dropped). A parameter is a seam when its type is the literal argparse seam
/// `fn([C,] Namespace) -> R` (#1544/#1621), **or** a bare reference to one of the
/// method's own generic type parameters `generics` (a uniform generic seam, #1647).
/// The adapter is passed to it by keyword, independent of surrounding optional
/// metadata (#1626). Returns `None` when no parameter is a seam — the decorator
/// receives the raw handler, unchanged.
fn seam_param(params: &[(String, Option<Type>)], generics: &[String]) -> Option<SeamInfo> {
    // A literal `fn([C,] Namespace)` seam always wins.
    if let Some(info) = params.iter().find_map(|(n, t)| {
        namespace_seam(t).map(|ctx| SeamInfo {
            name: n.clone(),
            kind: SeamKind::Literal(ctx),
        })
    }) {
        return Some(info);
    }
    // A uniform generic seam applies ONLY when the decorator has no explicit
    // function-typed parameter: a raw-handler decorator (#1527) already owns the
    // handler slot with its own `fn(...)` parameter, so a bare-generic parameter
    // there is an ordinary one (e.g. a `T kind` type-value argument, #1527), not a
    // seam. Without any `fn(...)` parameter, the first bare-generic parameter is
    // the handler seam.
    if params
        .iter()
        .any(|(_, t)| matches!(t, Some(Type::Func { .. })))
    {
        return None;
    }
    params.iter().find_map(|(n, t)| {
        is_generic_param_type(t, generics).then(|| SeamInfo {
            name: n.clone(),
            kind: SeamKind::Generic,
        })
    })
}

/// Whether `t` is a bare reference to one of `generics`: a `Type::Named` with no
/// type arguments whose name is a declared generic parameter (#1647). This is the
/// uniform-generic-seam marker — `Type[T]` (a type *value*) and concrete types are
/// not seams.
fn is_generic_param_type(t: &Option<Type>, generics: &[String]) -> bool {
    matches!(t, Some(Type::Named { name, args, .. })
        if args.is_empty() && generics.iter().any(|g| g == name))
}

/// Infer a generic-seam handler's context type (#1647): the type of the handler's
/// first parameter when it is a non-argv (non-scalar) type with no default — the
/// same "first parameter is the context" rule #1621 uses, excluding an injected
/// `Depends(...)`/source parameter (which carries a default). `None` when the
/// handler has no such leading context parameter (every parameter then binds from
/// the namespace or is injected).
fn infer_handler_context(params: &[Param]) -> Option<Type> {
    params
        .first()
        .filter(|p| p.default.is_none() && namespace_getter_for(&p.ty).is_none())
        .and_then(|p| p.ty.clone())
}

/// How a handler/provider parameter is bound by the adapter.
enum ParamKind {
    /// A plain parameter, passed through positionally (the Request, etc.).
    PassThrough,
    /// `T x = Depends(provider)` — injected by resolving `provider`.
    Depends { provider: String },
    /// `Depends(...)` in a shape we cannot resolve; carries the diagnostic.
    BadDepends { message: String, span: Span },
    /// `T x = Resolver(extra...)` where `Resolver` is a `@request_source` function
    /// (#959, D-0198): injected by calling `Resolver(request, "x", extra...)`.
    Source {
        resolver: String,
        /// The caller-supplied marker arguments (e.g. the fallback in
        /// `Query("")`), forwarded after `(request, name)`.
        extra_args: Vec<Arg>,
    },
    /// A `@request_source` marker in a shape we cannot resolve; the diagnostic.
    BadSource { message: String, span: Span },
}

/// Classify a parameter by its default. A `Depends(provider)` default marks a
/// dependency-injected parameter (`Depends` is a compiler-recognized marker,
/// I#466/D-0092); a default that *calls a `@request_source` function* marks a
/// request-sourced parameter (`source_fns`, #959/D-0198). Both markers are
/// recognized here and consumed by the adapter — never runtime values. Any other
/// default (or none) is an ordinary pass-through.
fn classify_param(p: &Param, source_fns: &HashSet<String>) -> ParamKind {
    let Some(default) = &p.default else {
        return ParamKind::PassThrough;
    };
    let Expr::Call { callee, args, span } = default else {
        return ParamKind::PassThrough;
    };
    let Expr::Name(name, _) = callee.as_ref() else {
        return ParamKind::PassThrough;
    };
    // A request-source marker: the default calls a `@request_source` function.
    // The marker's own arguments are forwarded after the synthesized
    // `(request, "<param>")` prefix; a keyword marker argument is rejected (the
    // forwarded call is positional), a clear error rather than a silent drop.
    if source_fns.contains(name) {
        if let Some(bad) = args.iter().find(|a| a.name.is_some()) {
            return ParamKind::BadSource {
                message: format!(
                    "`@request_source` marker `{name}(...)` on parameter `{}` takes only \
                     positional arguments (forwarded after the request and the parameter name)",
                    p.name
                ),
                span: bad.span,
            };
        }
        return ParamKind::Source {
            resolver: name.clone(),
            extra_args: args.clone(),
        };
    }
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
    source_fns: &HashSet<String>,
    diags: &mut Vec<TypeError>,
) -> (Function, String) {
    let sp = handler.span;
    let adapter_name = format!("{}$adapter", handler.name);

    // The framework value a request source resolves against — the handler's first
    // pass-through parameter (the Request). `@request_source` resolvers take it as
    // their first argument. A handler that declares a source parameter but no
    // pass-through parameter has nothing to resolve against (ZAP5048 below).
    let request: Option<(String, Span)> = handler
        .params
        .iter()
        .find(|p| matches!(classify_param(p, source_fns), ParamKind::PassThrough))
        .map(|p| (p.name.clone(), p.span));

    let mut builder = DepBuilder {
        fn_sigs,
        source_fns,
        diags,
        resolved: HashMap::new(),
        lets: Vec::new(),
    };

    // Resolve each injected parameter and record the call argument to pass for
    // every handler parameter, in declaration order.
    let mut call_args: Vec<Arg> = Vec::with_capacity(handler.params.len());
    let mut pass_through: Vec<Param> = Vec::new();
    for p in &handler.params {
        match classify_param(p, source_fns) {
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
            ParamKind::Source {
                resolver,
                extra_args,
            } => {
                // Inject `<ty> <name>$src = resolver(request, "<name>", extra...)`
                // and pass that local. The source parameter is NOT a seam
                // parameter — that is the whole point (it does not widen the
                // `fn(Request) -> Response` seam). It carries the param's declared
                // type so the resolver (often generic) is instantiated and sema
                // coerces/validates the result, exactly like a `Depends` local.
                match &request {
                    Some((req, req_span)) => {
                        let local = format!("{}$src", p.name);
                        let mut rargs = vec![
                            arg(Expr::Name(req.clone(), *req_span)),
                            arg(str_lit(&p.name, p.span)),
                        ];
                        rargs.extend(extra_args);
                        builder.lets.push(Stmt::Let {
                            is_const: false,
                            ty: p.ty.clone(),
                            name: local.clone(),
                            value: Expr::Call {
                                callee: Box::new(Expr::Name(resolver, p.span)),
                                args: rargs,
                                span: p.span,
                            },
                            span: p.span,
                        });
                        call_args.push(arg(Expr::Name(local, p.span)));
                    }
                    None => {
                        builder.diags.push(err(
                            format!(
                                "`@request_source` parameter `{}` needs the request value, but \
                                 handler `{}` declares no plain (framework-supplied) parameter; \
                                 add one, e.g. `Request req`",
                                p.name, handler.name
                            ),
                            p.span,
                        ));
                        // Keep the arg count right; the ZAP5048 above is the failure.
                        call_args.push(arg(Expr::Name(p.name.clone(), p.span)));
                    }
                }
            }
            ParamKind::BadSource { message, span } => {
                builder.diags.push(err(message, span));
                call_args.push(arg(Expr::Name(p.name.clone(), p.span)));
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
    /// The `@request_source` resolver names (#959), to classify provider params.
    source_fns: &'a HashSet<String>,
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
            match classify_param(pp, self.source_fns) {
                ParamKind::Depends { provider: inner } => {
                    let inner_local = self.resolve(&inner, pp.ty.clone(), pp.span, stack);
                    provider_args.push(arg(Expr::Name(inner_local, pp.span)));
                }
                ParamKind::Source { .. } => {
                    // Request-source injection is a handler-adapter capability
                    // (#959, D-0198): the per-request scope is the adapter body,
                    // not a provider. A `@request_source` parameter inside a
                    // provider has no request to resolve against here — a clear
                    // error, not a silent miscompile.
                    self.diags.push(err(
                        format!(
                            "`@request_source` parameter `{}` of `Depends(...)` provider \
                             `{provider}` is not injected: request sources are resolved only in \
                             the handler in v1 — make it `Depends(...)` or read it in the handler",
                            pp.name
                        ),
                        pp.span,
                    ));
                }
                ParamKind::BadSource { message, span } => {
                    self.diags.push(err(message, span));
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

/// A plain (non-f, non-raw, non-bytes) string-literal expression — the source
/// key (`"<param>"`) forwarded to a `@request_source` resolver (#959).
fn str_lit(value: &str, span: Span) -> Expr {
    Expr::Str {
        value: value.to_string(),
        raw: false,
        bytes: false,
        fstring: false,
        span,
        fstring_fields: Vec::new(),
    }
}

/// A positional call argument around an expression.
fn arg(value: Expr) -> Arg {
    let span = expr_span(&value);
    Arg {
        name: None,
        value,
        spread: ArgSpread::None,
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
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
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
        desugar_registering_decorators(&mut m, &HashMap::new());
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
        desugar_registering_decorators(&mut m, &HashMap::new());
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
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
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
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
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
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
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
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
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
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
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
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
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
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
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

    // ---- request-source parameter injection (I#959, D-0198) ----------------

    // The call args of a `let <name> = <callee>(args...)`, if the stmt is one.
    fn let_call<'a>(stmt: &'a Stmt, name: &str) -> Option<(&'a Expr, &'a [Arg])> {
        let Stmt::Let {
            name: n,
            value: Expr::Call { callee, args, .. },
            ..
        } = stmt
        else {
            return None;
        };
        if n != name {
            return None;
        }
        Some((callee.as_ref(), args.as_slice()))
    }

    fn is_name(e: &Expr, want: &str) -> bool {
        matches!(e, Expr::Name(n, _) if n == want)
    }

    fn is_str(e: &Expr, want: &str) -> bool {
        matches!(e, Expr::Str { value, .. } if value == want)
    }

    #[test]
    fn request_source_injects_and_is_dropped_from_the_seam() {
        // A `@request_source` resolver used as a parameter default makes that
        // parameter request-sourced: the adapter injects it by calling the
        // resolver with `(request, "<param>", <marker args...>)`, and the
        // parameter is NOT part of the `fn(Req)->Resp` seam.
        let mut m = module(
            "struct Req:\n    str path\n\
             struct Resp:\n    int64 status\n\
             @request_source\ndef Query(Req r, str name, str fallback) -> str:\n    return fallback\n\
             struct App:\n    def get(self, str p, fn(Req)->Resp h):\n        print(p)\n\
             @app.get(\"/x\")\ndef hx(Req req, str tag = Query(\"def\")) -> Resp:\n    return Resp(0)\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");

        // Registration binds the adapter.
        let stmts = main_stmts(&m);
        let trailing = match &stmts[1] {
            Stmt::Expr(Expr::Call { args, .. }) => &args.last().unwrap().value,
            _ => panic!("expected registration"),
        };
        assert!(matches!(trailing, Expr::Name(n, _) if n == "hx$adapter"));

        let adapter = func(&m, "hx$adapter").expect("adapter synthesized");
        // Only the Request is a seam parameter; the source param is dropped.
        assert_eq!(
            adapter.params.len(),
            1,
            "source param must not widen the seam"
        );
        assert_eq!(adapter.params[0].name, "req");

        let b = body(adapter);
        // `str tag$src = Query(req, "tag", "def")` — request + key + forwarded arg.
        let (callee, cargs) = let_call(&b[0], "tag$src").expect("source injection let");
        assert!(is_name(callee, "Query"), "calls the resolver");
        assert_eq!(cargs.len(), 3, "req + key + the marker's own arg");
        assert!(is_name(&cargs[0].value, "req"), "first arg is the request");
        assert!(
            is_str(&cargs[1].value, "tag"),
            "second arg is the param name"
        );
        assert!(
            is_str(&cargs[2].value, "def"),
            "marker args forwarded verbatim"
        );

        // Tail call: hx(req, tag$src).
        let Stmt::Return {
            value: Some(Expr::Call { callee, args, .. }),
            ..
        } = b.last().unwrap()
        else {
            panic!("expected tail return");
        };
        assert!(is_name(callee, "hx"));
        assert_eq!(args.len(), 2);
        assert!(is_name(&args[0].value, "req"));
        assert!(is_name(&args[1].value, "tag$src"));
    }

    #[test]
    fn request_source_composes_with_depends() {
        let mut m = module(
            "struct Req:\n    str path\n\
             struct DB:\n    int64 conn\n\
             struct Resp:\n    int64 status\n\
             def get_db() -> DB:\n    return DB(1)\n\
             @request_source\ndef Path(Req r, str name) -> str:\n    return r.path\n\
             struct App:\n    def get(self, str p, fn(Req)->Resp h):\n        print(p)\n\
             @app.get(\"/x\")\ndef hx(Req req, str id = Path(), DB db = Depends(get_db)) -> Resp:\n    return Resp(0)\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "hx$adapter").expect("adapter");
        assert_eq!(adapter.params.len(), 1, "only the Request is a seam param");
        let b = body(adapter);
        // Both a source injection and a Depends resolution are present.
        assert!(
            b.iter().any(|s| let_call(s, "id$src").is_some()),
            "source let present"
        );
        assert!(
            b.iter().any(|s| let_call(s, "get_db$dep").is_some()),
            "depends let present"
        );
        // Tail call passes req, the source local, and the dep local, in order.
        let Stmt::Return {
            value: Some(Expr::Call { args, .. }),
            ..
        } = b.last().unwrap()
        else {
            panic!("expected tail return");
        };
        assert_eq!(args.len(), 3);
        assert!(is_name(&args[0].value, "req"));
        assert!(is_name(&args[1].value, "id$src"));
        assert!(is_name(&args[2].value, "get_db$dep"));
    }

    #[test]
    fn request_source_without_a_request_param_is_an_error() {
        // No pass-through (framework) parameter: the resolver has no request to
        // resolve against — a clear ZAP5048, not a silent miscompile.
        let mut m = module(
            "struct Req:\n    str path\n\
             struct Resp:\n    int64 status\n\
             @request_source\ndef Path(Req r, str name) -> str:\n    return r.path\n\
             struct App:\n    def get(self, str p, fn(Req)->Resp h):\n        print(p)\n\
             @app.get(\"/x\")\ndef hx(str id = Path()) -> Resp:\n    return Resp(0)\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert_eq!(diags.len(), 1, "one diagnostic: {diags:?}");
        assert_eq!(diags[0].code, ErrorCode::ZAP5048);
        assert!(
            diags[0].message.contains("needs the request value"),
            "message: {}",
            diags[0].message
        );
    }

    #[test]
    fn request_source_inside_a_provider_is_rejected() {
        // A `@request_source` parameter inside a `Depends` provider has no request
        // to resolve against (the per-request scope is the handler adapter, not a
        // provider): rejected with a clear ZAP5048 in v1.
        let mut m = module(
            "struct Req:\n    str path\n\
             struct DB:\n    int64 conn\n\
             struct Resp:\n    int64 status\n\
             @request_source\ndef Path(Req r, str name) -> str:\n    return r.path\n\
             def get_db(str who = Path()) -> DB:\n    return DB(0)\n\
             struct App:\n    def get(self, str p, fn(Req)->Resp h):\n        print(p)\n\
             @app.get(\"/x\")\ndef hx(Req req, DB db = Depends(get_db)) -> Resp:\n    return Resp(0)\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(
            diags.iter().any(|d| d.code == ErrorCode::ZAP5048
                && d.message.contains("resolved only in the handler")),
            "expected a provider-source rejection: {diags:?}"
        );
    }

    #[test]
    fn request_source_keyword_marker_argument_is_rejected() {
        // The forwarded resolver call is positional; a keyword marker argument is
        // a malformed source annotation — a clear ZAP5048, not a silent drop.
        let mut m = module(
            "struct Req:\n    str path\n\
             struct Resp:\n    int64 status\n\
             @request_source\ndef Query(Req r, str name, str fallback) -> str:\n    return fallback\n\
             struct App:\n    def get(self, str p, fn(Req)->Resp h):\n        print(p)\n\
             @app.get(\"/x\")\ndef hx(Req req, str tag = Query(fallback=\"d\")) -> Resp:\n    return Resp(0)\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert_eq!(diags.len(), 1, "one diagnostic: {diags:?}");
        assert_eq!(diags[0].code, ErrorCode::ZAP5048);
        assert!(
            diags[0].message.contains("positional"),
            "message: {}",
            diags[0].message
        );
    }

    #[test]
    fn ordinary_default_calling_a_non_source_fn_stays_pass_through() {
        // Regression: a parameter default that calls a function which is NOT
        // `@request_source` (and is not `Depends`) is an ordinary default — the
        // parameter stays a pass-through seam parameter, no adapter, no error.
        let mut m = module(
            "struct Req:\n    str path\n\
             struct Resp:\n    int64 status\n\
             def mk() -> str:\n    return \"\"\n\
             struct App:\n    def get(self, str p, fn(Req, str)->Resp h):\n        print(p)\n\
             @app.get(\"/x\")\ndef hx(Req req, str tag = mk()) -> Resp:\n    return Resp(0)\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        assert!(
            func(&m, "hx$adapter").is_none(),
            "a non-source default needs no adapter"
        );
        // The handler itself is registered directly.
        let stmts = main_stmts(&m);
        let trailing = match &stmts[1] {
            Stmt::Expr(Expr::Call { args, .. }) => &args.last().unwrap().value,
            _ => panic!(),
        };
        assert!(matches!(trailing, Expr::Name(n, _) if n == "hx"));
    }

    // ---- object-binding (replacing) decorators (#1509) ---------------------
    //
    // A dotted decorator whose receiver method RETURNS a value (non-void) binds
    // the decorated name to that returned object (Click-style command objects),
    // instead of registering-and-discarding. The decision is return-type-driven
    // (chosen mechanism A): a void method keeps the registering behavior. The
    // desugar pass runs before sema, so these tests only assert the AST
    // transform; the synthesized `auto name = recv.method(..., name$impl)` is
    // type-checked by sema like any other binding.

    // An `auto <name> = <callee>(args...)` let whose value is a call: returns the
    // bound name, the call's trailing-argument expression, and the receiver
    // root + method of the (outermost) call, if the stmt is that shape.
    fn rebind_let<'a>(stmt: &'a Stmt, name: &str) -> Option<(&'a Expr, String, String)> {
        let Stmt::Let {
            name: n,
            value: Expr::Call { callee, args, .. },
            is_const: false,
            ..
        } = stmt
        else {
            return None;
        };
        if n != name {
            return None;
        }
        let Expr::Attr {
            base, name: method, ..
        } = callee.as_ref()
        else {
            return None;
        };
        let Expr::Name(root, _) = base.as_ref() else {
            return None;
        };
        Some((&args.last().unwrap().value, root.clone(), method.clone()))
    }

    #[test]
    fn returning_dotted_decorator_rebinds_name_to_the_object() {
        // The issue's minimal reproduction: `@cli.command` on `greet`, where
        // `Cli.command(...) -> Command` returns a value. After desugar the name
        // `greet` must be bound to `cli.command(greet$impl)` (the Command), and
        // the original function kept, renamed, as the typed callable `greet$impl`.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran command\")\n\
             struct Cli:\n    def command(self, fn(str) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(str name) -> str:\n    return \"Hello \" + name\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");

        // The original `greet` is renamed to the typed callable `greet$impl`,
        // keeping its parameters and return type; there is no top-level `greet`.
        assert!(
            func(&m, "greet").is_none(),
            "greet must be rebound, not a fn"
        );
        let impl_fn = func(&m, "greet$impl").expect("original kept as greet$impl");
        assert_eq!(impl_fn.params.len(), 1, "impl keeps the handler params");
        assert_eq!(impl_fn.params[0].name, "name");
        assert!(
            impl_fn.decorators.is_empty(),
            "the dotted decorator is consumed into the binding"
        );

        // main: `auto cli = Cli()`, then `auto greet = cli.command(greet$impl)`,
        // then `greet.run()` — the binding is spliced after the receiver and
        // before the use.
        let stmts = main_stmts(&m);
        assert!(matches!(&stmts[0], Stmt::Let { name, .. } if name == "cli"));
        let (trailing, root, method) =
            rebind_let(&stmts[1], "greet").expect("greet rebound to a command object");
        assert_eq!((root.as_str(), method.as_str()), ("cli", "command"));
        assert!(
            matches!(trailing, Expr::Name(n, _) if n == "greet$impl"),
            "the decorator receives the original function as a typed callable"
        );
        // The use site is preserved, after the binding.
        assert!(matches!(
            &stmts[2],
            Stmt::Expr(Expr::Call { callee, .. })
                if matches!(callee.as_ref(), Expr::Attr { name, .. } if name == "run")
        ));
    }

    #[test]
    fn stacked_returning_decorators_compose_bottom_up() {
        // Stacked `@cli.command` / `@cli.option` / `@cli.argument` compose into a
        // nested call chain, innermost = the decorator closest to `def`. The
        // decorator arguments (the option/argument metadata) are retained
        // verbatim as call arguments, so a later dispatcher can read them.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Spec:\n    int64 n\n\
             struct Cli:\n\
             \x20   def argument(self, str name, fn(str) -> str handler) -> Spec:\n        return Spec(0)\n\
             \x20   def option(self, str flag, Spec s) -> Spec:\n        return s\n\
             \x20   def command(self, Spec s) -> Command:\n        return Command()\n\
             @cli.command\n@cli.option(\"--count\")\n@cli.argument(\"name\")\n\
             def greet(str name) -> str:\n    return name\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");

        assert!(func(&m, "greet").is_none());
        assert!(func(&m, "greet$impl").is_some());

        // Outermost call is `cli.command(...)`.
        let stmts = main_stmts(&m);
        let (command_inner, root, method) = rebind_let(&stmts[1], "greet").expect("greet rebound");
        assert_eq!((root.as_str(), method.as_str()), ("cli", "command"));

        // Its trailing arg is `cli.option("--count", <inner>)`.
        let Expr::Call { callee, args, .. } = command_inner else {
            panic!("command's arg must be the option call: {command_inner:?}");
        };
        assert!(
            matches!(callee.as_ref(), Expr::Attr { name, .. } if name == "option"),
            "second layer is option"
        );
        assert!(
            matches!(&args[0].value, Expr::Str { value, .. } if value == "--count"),
            "option metadata retained"
        );

        // Its trailing arg is `cli.argument("name", greet$impl)` — innermost.
        let option_inner = &args.last().unwrap().value;
        let Expr::Call { callee, args, .. } = option_inner else {
            panic!("option's arg must be the argument call: {option_inner:?}");
        };
        assert!(
            matches!(callee.as_ref(), Expr::Attr { name, .. } if name == "argument"),
            "innermost layer is argument"
        );
        assert!(
            matches!(&args[0].value, Expr::Str { value, .. } if value == "name"),
            "argument metadata retained"
        );
        assert!(
            matches!(&args.last().unwrap().value, Expr::Name(n, _) if n == "greet$impl"),
            "the original function is the innermost argument"
        );
    }

    #[test]
    fn void_dotted_decorator_still_registers() {
        // A dotted decorator whose method returns VOID keeps the registering
        // behavior (fastzap `@app.get`): the name is NOT rebound, the return is
        // discarded, and a `app.get(...)` statement is spliced after the binding.
        let mut m = module(
            "struct App:\n    def get(self, str path, fn() -> int64 h):\n        print(path)\n\
             @app.get(\"/x\")\ndef h() -> int64:\n    return 0\n\
             def main():\n    auto app = App()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        // The handler stays a top-level function; it is not rebound.
        assert!(
            func(&m, "h").is_some(),
            "void method must not rebind the name"
        );
        assert!(
            func(&m, "h$impl").is_none(),
            "no impl rename for a void method"
        );
        let stmts = main_stmts(&m);
        assert_eq!(
            registration(&stmts[1]),
            Some(("app".to_string(), "get".to_string())),
            "a void dotted decorator registers as before"
        );
        assert!(
            rebind_let(&stmts[1], "h").is_none(),
            "a registration is a bare expr statement, not a rebinding let"
        );
    }

    #[test]
    fn unresolvable_receiver_falls_back_to_registering() {
        // When the receiver's type cannot be resolved (never bound by a `let`,
        // or an imported module — the latter is the cli#4 extension), the pass
        // cannot read the method's return type, so it falls back to the
        // registering behavior rather than guessing a rebind. No crash, no
        // rename; sema later reports the unbound receiver precisely.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn() -> int64 h) -> Command:\n        return Command()\n\
             @cli.command\ndef h() -> int64:\n    return 0\n\
             def main():\n    print(\"cli never bound here\")\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        assert!(
            func(&m, "h").is_some(),
            "unresolved receiver must not rebind"
        );
        assert!(
            func(&m, "h$impl").is_none(),
            "no rename on the fallback path"
        );
        // The registration landed (at the top of main, the unbound-receiver
        // fallback) as a bare `cli.command(h)` statement.
        assert!(
            main_stmts(&m)
                .iter()
                .any(|s| registration(s) == Some(("cli".into(), "command".into()))),
            "falls back to a registration statement"
        );
    }

    // ---- #1544: argparse-backed handler adapter synthesis -------------------

    // The getter method name and the parameter name read by a `ns.get_X("p")`
    // call expression, if that is its shape.
    fn getter_call(e: &Expr) -> Option<(String, String)> {
        let Expr::Call { callee, args, .. } = e else {
            return None;
        };
        let Expr::Attr { name: getter, .. } = callee.as_ref() else {
            return None;
        };
        let Expr::Str { value, .. } = &args.first()?.value else {
            return None;
        };
        Some((getter.clone(), value.clone()))
    }

    #[test]
    fn namespace_adapter_synthesized_for_fn_namespace_handler() {
        // #1544: a returning decorator whose receiver method declares its handler
        // parameter as `fn(Namespace) -> R` receives a compiler-synthesized
        // adapter, NOT the raw handler — so a package can adapt a handler of ANY
        // signature into one uniform `fn(Namespace) -> R` dispatcher. The adapter
        // reads each handler parameter from the argparse `Namespace` by name with
        // the type-matched getter and calls the handler in signature order.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(int64 count, bool shout, str name) -> str:\n    return name\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");

        // The original handler is kept as the renamed implementation.
        let impl_fn = func(&m, "greet$impl").expect("original kept as greet$impl");
        assert_eq!(impl_fn.params.len(), 3);

        // An adapter `greet$adapter(Namespace ns) -> str` is synthesized.
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        assert_eq!(
            adapter.params.len(),
            1,
            "adapter takes exactly the Namespace"
        );
        assert!(
            matches!(&adapter.params[0].ty, Some(Type::Named { name, .. }) if name == "Namespace"),
            "adapter parameter is the argparse Namespace"
        );
        assert!(
            matches!(&adapter.ret, Some(Type::Named { name, .. }) if name == "str"),
            "adapter return type mirrors the handler's"
        );

        // Its body is `return greet$impl(ns.get_int("count"), ns.get_flag("shout"),
        // ns.get_str("name"))` — by name, in the handler's signature order.
        let b = body(adapter);
        assert_eq!(b.len(), 1);
        let Stmt::Return {
            value: Some(Expr::Call { callee, args, .. }),
            ..
        } = &b[0]
        else {
            panic!("adapter body must be `return greet$impl(...)`: {:?}", b[0]);
        };
        assert!(
            matches!(callee.as_ref(), Expr::Name(n, _) if n == "greet$impl"),
            "adapter calls the renamed implementation"
        );
        assert_eq!(args.len(), 3, "one argument per handler parameter");
        assert_eq!(
            getter_call(&args[0].value),
            Some(("get_int".into(), "count".into()))
        );
        assert_eq!(
            getter_call(&args[1].value),
            Some(("get_flag".into(), "shout".into()))
        );
        assert_eq!(
            getter_call(&args[2].value),
            Some(("get_str".into(), "name".into()))
        );

        // The decorator receives the ADAPTER, not the raw handler.
        let stmts = main_stmts(&m);
        let (trailing, root, method) = rebind_let(&stmts[1], "greet").expect("greet rebound");
        assert_eq!((root.as_str(), method.as_str()), ("cli", "command"));
        assert!(
            matches!(trailing, Expr::Name(n, _) if n == "greet$adapter"),
            "the decorator receives the synthesized adapter"
        );
    }

    #[test]
    fn namespace_adapter_void_handler_has_no_return() {
        // A void handler (the Click shape: it prints and returns nothing). The
        // adapter's body is a bare call statement and it has no return type.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Namespace) handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(int64 count):\n    print(count)\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        assert!(adapter.ret.is_none(), "void handler -> void adapter");
        let b = body(adapter);
        assert_eq!(b.len(), 1);
        let Stmt::Expr(Expr::Call { callee, args, .. }) = &b[0] else {
            panic!("void adapter body must be a bare call: {:?}", b[0]);
        };
        assert!(matches!(callee.as_ref(), Expr::Name(n, _) if n == "greet$impl"));
        assert_eq!(
            getter_call(&args[0].value),
            Some(("get_int".into(), "count".into()))
        );
    }

    #[test]
    fn namespace_adapter_zero_params() {
        // A zero-parameter handler: the adapter ignores the Namespace and calls
        // the handler with no arguments.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet() -> str:\n    return \"hi\"\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        let b = body(adapter);
        let Stmt::Return {
            value: Some(Expr::Call { callee, args, .. }),
            ..
        } = &b[0]
        else {
            panic!("expected `return greet$impl()`");
        };
        assert!(matches!(callee.as_ref(), Expr::Name(n, _) if n == "greet$impl"));
        assert!(args.is_empty(), "no handler parameters, no getter calls");
    }

    #[test]
    fn namespace_adapter_float_and_other_int_getters() {
        // float64 -> get_float; the Python-facing `int`/`float` spellings map to
        // the same int/float getters.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(float64 ratio, int n, str s) -> str:\n    return s\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        let Stmt::Return {
            value: Some(Expr::Call { args, .. }),
            ..
        } = &body(adapter)[0]
        else {
            panic!("expected return call");
        };
        assert_eq!(
            getter_call(&args[0].value),
            Some(("get_float".into(), "ratio".into()))
        );
        assert_eq!(
            getter_call(&args[1].value),
            Some(("get_int".into(), "n".into()))
        );
        assert_eq!(
            getter_call(&args[2].value),
            Some(("get_str".into(), "s".into()))
        );
    }

    #[test]
    fn fixed_fn_handler_keeps_passing_raw_impl() {
        // Back-compat (#1527): a receiver method that declares the handler's own
        // fixed signature (`fn(str) -> str`), NOT `fn(Namespace) -> R`, still
        // receives the raw handler — no adapter is synthesized.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(str) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(str name) -> str:\n    return name\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        assert!(
            func(&m, "greet$adapter").is_none(),
            "no adapter for a fixed-signature handler"
        );
        let (trailing, _, _) = rebind_let(&main_stmts(&m)[1], "greet").expect("greet rebound");
        assert!(
            matches!(trailing, Expr::Name(n, _) if n == "greet$impl"),
            "fixed-signature handler receives the raw implementation, unchanged"
        );
    }

    #[test]
    fn namespace_adapter_unsupported_param_type_errors() {
        // A handler parameter whose type is not an argv-sourced scalar cannot be
        // bound from the Namespace in v1 (no Context / struct binding): a precise
        // decoration-site error, not a silent miscompile.
        let mut m = module(
            "struct Ctx:\n    int64 n\n\
             struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(Ctx ctx, int64 count) -> str:\n    return \"x\"\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("Ctx") || d.message.contains("ctx")),
            "expected an unsupported-parameter diagnostic, got: {diags:?}"
        );
    }

    #[test]
    fn imported_receiver_namespace_adapter() {
        // The cli#4 surface: the decorator's receiver is an IMPORTED module
        // (`import cli; @cli.command`), resolved to the package's merged, mangled
        // free function (#1519). Its trailing parameter type is read from the
        // snapshot of top-level function signatures, so the same adapter synthesis
        // applies across the package boundary.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             def __zm1_cli__command(fn(Namespace) -> str handler) -> Command:\n    return Command()\n\
             @cli.command\ndef greet(int64 count, str name) -> str:\n    return name\n\
             def main():\n    greet.run()\n",
        );
        let mut prefixes = HashMap::new();
        prefixes.insert("cli".to_string(), "__zm1_cli__".to_string());
        let diags = desugar_registering_decorators(&mut m, &prefixes);
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");

        let adapter = func(&m, "greet$adapter").expect("adapter synthesized across the boundary");
        let Stmt::Return {
            value: Some(Expr::Call { callee, args, .. }),
            ..
        } = &body(adapter)[0]
        else {
            panic!("expected `return greet$impl(...)`");
        };
        assert!(matches!(callee.as_ref(), Expr::Name(n, _) if n == "greet$impl"));
        assert_eq!(
            getter_call(&args[0].value),
            Some(("get_int".into(), "count".into()))
        );
        assert_eq!(
            getter_call(&args[1].value),
            Some(("get_str".into(), "name".into()))
        );

        // The merged, mangled package function receives the adapter as its
        // trailing argument.
        let stmts = main_stmts(&m);
        let binding = stmts.iter().find_map(|s| match s {
            Stmt::Let {
                name,
                value: Expr::Call { callee, args, .. },
                ..
            } if name == "greet" => Some((callee.as_ref(), args)),
            _ => None,
        });
        let (callee, args) = binding.expect("greet rebound to the imported command call");
        assert!(matches!(callee, Expr::Name(n, _) if n == "__zm1_cli__command"));
        assert!(
            matches!(&args.last().unwrap().value, Expr::Name(n, _) if n == "greet$adapter"),
            "the imported receiver gets the synthesized adapter"
        );
    }

    // ---- #1621: explicit typed context through the adapter --------------------

    #[test]
    fn context_seam_passes_context_and_binds_scalars() {
        // #1621 / cli PR #7: a handler may declare `Context ctx` as its first
        // parameter; the dispatcher supplies it directly. The receiver method
        // opts in with the seam `fn(Context, Namespace) -> R`. The synthesized
        // adapter takes `(Context ctx, Namespace ns)`, passes `ctx` to the
        // handler's context parameter, and reads the remaining scalars from the
        // namespace by name — in the handler's signature order.
        let mut m = module(
            "struct Context:\n    str command_name\n\
             struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Context, Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(Context ctx, int64 count, str name) -> str:\n    return name\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");

        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        assert_eq!(
            adapter.params.len(),
            2,
            "adapter takes (Context, Namespace)"
        );
        assert!(
            matches!(&adapter.params[0].ty, Some(Type::Named { name, .. }) if name == "Context"),
            "first adapter parameter is the context type from the seam"
        );
        assert!(
            matches!(&adapter.params[1].ty, Some(Type::Named { name, .. }) if name == "Namespace"),
            "second adapter parameter is the Namespace"
        );
        let ctx_name = adapter.params[0].name.clone();
        let Stmt::Return {
            value: Some(Expr::Call { callee, args, .. }),
            ..
        } = &body(adapter)[0]
        else {
            panic!("expected `return greet$impl(...)`");
        };
        assert!(matches!(callee.as_ref(), Expr::Name(n, _) if n == "greet$impl"));
        assert_eq!(args.len(), 3);
        // ctx is passed through; count/name come from the namespace.
        assert!(
            matches!(&args[0].value, Expr::Name(n, _) if n == &ctx_name),
            "the context parameter receives the adapter's context argument"
        );
        assert_eq!(
            getter_call(&args[1].value),
            Some(("get_int".into(), "count".into()))
        );
        assert_eq!(
            getter_call(&args[2].value),
            Some(("get_str".into(), "name".into()))
        );
    }

    #[test]
    fn context_seam_without_context_param_binds_all_from_namespace() {
        // The context parameter is OPTIONAL on the handler. Under the
        // `fn(Context, Namespace)` seam, a handler that declares no context still
        // works: the adapter still takes `(Context, Namespace)` (uniform seam) but
        // binds every parameter from the namespace and ignores the context.
        let mut m = module(
            "struct Context:\n    str command_name\n\
             struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Context, Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(str name) -> str:\n    return name\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        assert_eq!(adapter.params.len(), 2, "seam shape is preserved");
        let Stmt::Return {
            value: Some(Expr::Call { args, .. }),
            ..
        } = &body(adapter)[0]
        else {
            panic!("expected return call");
        };
        assert_eq!(args.len(), 1);
        assert_eq!(
            getter_call(&args[0].value),
            Some(("get_str".into(), "name".into()))
        );
    }

    #[test]
    fn plain_namespace_seam_has_no_context_parameter() {
        // Back-compat: a `fn(Namespace)` seam (no context) is unchanged — the
        // adapter takes only the Namespace.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(str name) -> str:\n    return name\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        assert_eq!(
            adapter.params.len(),
            1,
            "no context parameter for a plain Namespace seam"
        );
        assert!(
            matches!(&adapter.params[0].ty, Some(Type::Named { name, .. }) if name == "Namespace"),
        );
    }

    #[test]
    fn context_seam_second_nonscalar_param_errors() {
        // Only the FIRST parameter may be the context. A second non-scalar
        // parameter cannot be bound from the namespace — a precise error.
        let mut m = module(
            "struct Context:\n    str command_name\n\
             struct Widget:\n    int64 n\n\
             struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Context, Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(Context ctx, Widget w) -> str:\n    return \"x\"\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(
            diags.iter().any(|d| d.message.contains("`w`")),
            "expected an unsupported-parameter diagnostic for `w`, got: {diags:?}"
        );
    }

    #[test]
    fn stacked_namespace_adapter_innermost_receives_adapter() {
        // Stacked decorators: the innermost (closest to `def`) is the one that
        // receives the handler. When it declares `fn(Namespace) -> R`, the adapter
        // is threaded as the innermost argument and the metadata decorators
        // compose around it.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Spec:\n    int64 n\n\
             struct Cli:\n\
             \x20   def argument(self, str name, fn(Namespace) -> str handler) -> Spec:\n        return Spec(0)\n\
             \x20   def option(self, str flag, Spec s) -> Spec:\n        return s\n\
             \x20   def command(self, Spec s) -> Command:\n        return Command()\n\
             @cli.command\n@cli.option(\"--count\")\n@cli.argument(\"name\")\n\
             def greet(int64 count, str name) -> str:\n    return name\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        assert!(func(&m, "greet$adapter").is_some(), "adapter synthesized");

        // Drill to the innermost `cli.argument("name", greet$adapter)`.
        let (command_inner, _, _) = rebind_let(&main_stmts(&m)[1], "greet").expect("rebound");
        let Expr::Call { args, .. } = command_inner else {
            panic!("command arg is the option call");
        };
        let Expr::Call { callee, args, .. } = &args.last().unwrap().value else {
            panic!("option arg is the argument call");
        };
        assert!(matches!(callee.as_ref(), Expr::Attr { name, .. } if name == "argument"));
        assert!(
            matches!(&args.last().unwrap().value, Expr::Name(n, _) if n == "greet$adapter"),
            "innermost argument is the synthesized adapter, not the raw handler"
        );
    }

    // ---- #1625: wider scalar parameter types --------------------------------

    // A `conv(ns.get_X("name"))` call: the conversion builtin, the getter, and the
    // parameter name read.
    fn conv_getter_call(e: &Expr) -> Option<(String, String, String)> {
        let Expr::Call { callee, args, .. } = e else {
            return None;
        };
        let Expr::Name(conv, _) = callee.as_ref() else {
            return None;
        };
        let (getter, name) = getter_call(&args.first()?.value)?;
        Some((conv.clone(), getter, name))
    }

    #[test]
    fn wider_scalar_params_bind_via_conversion() {
        // #1625: `int32`/`uint8`/`float32` read the wide getter and convert.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(int32 count, uint8 level, float32 ratio) -> str:\n    return \"x\"\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        let Stmt::Return {
            value: Some(Expr::Call { args, .. }),
            ..
        } = &body(adapter)[0]
        else {
            panic!("expected return call");
        };
        assert_eq!(
            conv_getter_call(&args[0].value),
            Some(("int32".into(), "get_int".into(), "count".into()))
        );
        assert_eq!(
            conv_getter_call(&args[1].value),
            Some(("uint8".into(), "get_int".into(), "level".into()))
        );
        assert_eq!(
            conv_getter_call(&args[2].value),
            Some(("float32".into(), "get_float".into(), "ratio".into()))
        );
    }

    #[test]
    fn wide_scalar_params_bind_without_conversion() {
        // The getter-native types are read directly, with no conversion wrapper.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(int64 a, float64 b) -> str:\n    return \"x\"\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        let Stmt::Return {
            value: Some(Expr::Call { args, .. }),
            ..
        } = &body(adapter)[0]
        else {
            panic!("expected return call");
        };
        assert_eq!(
            getter_call(&args[0].value),
            Some(("get_int".into(), "a".into()))
        );
        assert_eq!(
            getter_call(&args[1].value),
            Some(("get_float".into(), "b".into()))
        );
    }

    #[test]
    fn non_conversion_scalar_param_errors() {
        // `int8`/`int16` are not scalar value types in Zap (array dtypes only;
        // epic #1644/#1654), so they cannot be bound — a precise decoration-site
        // error. (`bigint`/`complex` now bind via #1657; see the #1657 test below.)
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(int8 n) -> str:\n    return \"x\"\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(
            diags.iter().any(|d| d.message.contains("`n`")),
            "expected an unsupported-parameter diagnostic for `n`, got: {diags:?}"
        );
    }

    // ---- #1626: handler reaches its parameter past optional metadata ---------

    // The arguments of the `root.method(...)` rebind call for `name`, if it is one.
    fn rebind_call_args<'a>(stmt: &'a Stmt, name: &str) -> Option<&'a [Arg]> {
        let Stmt::Let {
            name: n,
            value: Expr::Call { args, .. },
            ..
        } = stmt
        else {
            return None;
        };
        (n == name).then_some(args.as_slice())
    }

    #[test]
    fn handler_passed_by_name_before_optional_metadata() {
        // #1626: optional metadata precedes the handler parameter. The synthesized
        // adapter must reach the `handler` parameter by name, leaving `name=` to
        // its default — not fill the `name` slot positionally.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, str name, fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(str who) -> str:\n    return who\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let args = rebind_call_args(&main_stmts(&m)[1], "greet").expect("greet rebound");
        let handler_arg = args
            .iter()
            .find(|a| a.name.as_deref() == Some("handler"))
            .expect("adapter passed by the `handler` keyword");
        assert!(
            matches!(&handler_arg.value, Expr::Name(n, _) if n == "greet$adapter"),
            "the `handler` keyword carries the synthesized adapter"
        );
    }

    #[test]
    fn handler_seam_detected_before_trailing_metadata() {
        // #1626: the handler parameter comes FIRST, with trailing optional
        // metadata. The seam is detected regardless of position and the adapter is
        // still passed by name.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Namespace) -> str handler, str name) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(str who) -> str:\n    return who\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        assert!(
            func(&m, "greet$adapter").is_some(),
            "seam detected on a non-trailing parameter"
        );
        let args = rebind_call_args(&main_stmts(&m)[1], "greet").expect("greet rebound");
        assert!(
            args.iter().any(|a| a.name.as_deref() == Some("handler")
                && matches!(&a.value, Expr::Name(n, _) if n == "greet$adapter")),
            "adapter passed by the `handler` keyword even though it is the first parameter"
        );
    }

    #[test]
    fn handler_by_name_coexists_with_decorator_metadata() {
        // `@cli.command(name="alias")`: the decorator's own keyword metadata and
        // the by-name handler coexist on the one call.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, str name, fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command(name=\"alias\")\ndef greet(str who) -> str:\n    return who\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let args = rebind_call_args(&main_stmts(&m)[1], "greet").expect("greet rebound");
        assert!(
            args.iter().any(|a| a.name.as_deref() == Some("name")
                && matches!(&a.value, Expr::Str { value, .. } if value == "alias")),
            "the decorator's `name=` metadata is retained"
        );
        assert!(
            args.iter().any(|a| a.name.as_deref() == Some("handler")
                && matches!(&a.value, Expr::Name(n, _) if n == "greet$adapter")),
            "the adapter is passed by the `handler` keyword alongside the metadata"
        );
    }

    #[test]
    fn raw_handler_without_seam_stays_positional() {
        // No namespace seam (#1527 fixed signature): the raw handler is passed as
        // the trailing positional argument, unchanged — not by keyword.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(str) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(str name) -> str:\n    return name\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        assert!(
            func(&m, "greet$adapter").is_none(),
            "no adapter for a fixed-signature handler"
        );
        let args = rebind_call_args(&main_stmts(&m)[1], "greet").expect("greet rebound");
        let last = args.last().unwrap();
        assert!(
            last.name.is_none(),
            "raw handler stays a positional argument"
        );
        assert!(matches!(&last.value, Expr::Name(n, _) if n == "greet$impl"));
    }

    // ---- #1645: positional metadata around a non-last handler parameter ------

    #[test]
    fn positional_metadata_skips_nonlast_handler_slot() {
        // #1645: positional decorator metadata with a NON-LAST handler parameter.
        // `argument(str name, fn(Namespace) handler, str metavar = "")` with two
        // positional metadata arguments — the handler is at index 1, so the second
        // positional must be routed PAST it to `metavar` by name. It must not land
        // on the handler slot (which the adapter fills by keyword), nor double-bind
        // it.
        let mut m = module(
            "struct Arg:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def argument(self, str name, fn(Namespace) -> str handler, str metavar = \"\") -> Arg:\n        return Arg()\n\
             @cli.argument(\"pos-name\", \"META\")\ndef greet(str who) -> str:\n    return who\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let args = rebind_call_args(&main_stmts(&m)[1], "greet").expect("greet rebound");
        // Exactly one positional ("pos-name" → the pre-handler `name`); the handler
        // slot is never filled positionally.
        assert_eq!(
            args.iter().filter(|a| a.name.is_none()).count(),
            1,
            "only the pre-handler positional stays positional: {args:?}"
        );
        assert!(
            args.iter().any(|a| a.name.is_none()
                && matches!(&a.value, Expr::Str { value, .. } if value == "pos-name")),
            "the first positional binds the pre-handler `name` parameter: {args:?}"
        );
        // The post-handler positional is routed PAST the handler to `metavar`.
        assert!(
            args.iter().any(|a| a.name.as_deref() == Some("metavar")
                && matches!(&a.value, Expr::Str { value, .. } if value == "META")),
            "the post-handler positional binds `metavar` by name, skipping the handler: {args:?}"
        );
        // The adapter reaches the handler parameter by keyword.
        assert!(
            args.iter().any(|a| a.name.as_deref() == Some("handler")
                && matches!(&a.value, Expr::Name(n, _) if n == "greet$adapter")),
            "the adapter is passed to `handler` by keyword: {args:?}"
        );
    }

    #[test]
    fn positional_metadata_skips_first_handler_slot() {
        // #1645: the handler is the FIRST parameter, with positional metadata after
        // it. Both positionals route past the leading handler slot to `name`/
        // `metavar` by name, leaving the handler to the by-keyword adapter.
        let mut m = module(
            "struct Arg:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def argument(self, fn(Namespace) -> str handler, str name, str metavar = \"\") -> Arg:\n        return Arg()\n\
             @cli.argument(\"pos-name\", \"META\")\ndef greet(str who) -> str:\n    return who\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let args = rebind_call_args(&main_stmts(&m)[1], "greet").expect("greet rebound");
        // No positional survives — all route past the leading handler slot by name.
        assert_eq!(
            args.iter().filter(|a| a.name.is_none()).count(),
            0,
            "no positional lands on the leading handler slot: {args:?}"
        );
        assert!(
            args.iter().any(|a| a.name.as_deref() == Some("name")
                && matches!(&a.value, Expr::Str { value, .. } if value == "pos-name")),
            "first positional binds `name` by name: {args:?}"
        );
        assert!(
            args.iter().any(|a| a.name.as_deref() == Some("metavar")
                && matches!(&a.value, Expr::Str { value, .. } if value == "META")),
            "second positional binds `metavar` by name: {args:?}"
        );
        assert!(
            args.iter().any(|a| a.name.as_deref() == Some("handler")
                && matches!(&a.value, Expr::Name(n, _) if n == "greet$adapter")),
            "the adapter is passed to `handler` by keyword: {args:?}"
        );
    }

    #[test]
    fn positional_before_handler_slot_stays_verbatim() {
        // #1645 guard: a single positional that does NOT reach the handler slot is
        // kept verbatim — no spurious keyword rewrite — matching #1626. With
        // `argument(str name, fn(Namespace) handler, str metavar = "")`, one
        // positional fills `name` positionally and `metavar` keeps its default.
        let mut m = module(
            "struct Arg:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def argument(self, str name, fn(Namespace) -> str handler, str metavar = \"\") -> Arg:\n        return Arg()\n\
             @cli.argument(\"pos-name\")\ndef greet(str who) -> str:\n    return who\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let args = rebind_call_args(&main_stmts(&m)[1], "greet").expect("greet rebound");
        assert!(
            args.iter().any(|a| a.name.is_none()
                && matches!(&a.value, Expr::Str { value, .. } if value == "pos-name")),
            "the lone positional stays positional (fills `name`): {args:?}"
        );
        assert!(
            !args.iter().any(|a| a.name.as_deref() == Some("metavar")),
            "no spurious `metavar` keyword when the positional does not reach the handler: {args:?}"
        );
        assert!(
            args.iter().any(|a| a.name.as_deref() == Some("handler")
                && matches!(&a.value, Expr::Name(n, _) if n == "greet$adapter")),
            "the adapter is passed to `handler` by keyword: {args:?}"
        );
    }

    // ---- #1646: injected Depends parameters in the argparse adapter ----------

    #[test]
    fn namespace_adapter_injects_depends_alongside_context_and_scalars() {
        // #1646: an injected `Depends(provider)` parameter composes with the #1621
        // context and the namespace scalars. The adapter resolves the provider into
        // a `<provider>$dep` local (deps-first) and passes it to the handler; the
        // context flows from the dispatcher, the scalar from the namespace. The seam
        // is NOT widened by the injected parameter.
        let mut m = module(
            "struct Context:\n    str command_name\n\
             struct DB:\n    int64 conn\n\
             def get_db() -> DB:\n    return DB(conn=1)\n\
             struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Context, Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(Context ctx, DB db = Depends(get_db), str name) -> str:\n    return name\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        assert_eq!(
            adapter.params.len(),
            2,
            "a Depends parameter does not widen the (Context, Namespace) seam"
        );
        let ctx_name = adapter.params[0].name.clone();
        let stmts = body(adapter);
        // Deps-first: `DB get_db$dep = get_db()` precedes the handler call.
        let (dep_callee, _) =
            let_call(&stmts[0], "get_db$dep").expect("Depends `let` emitted first");
        assert!(
            is_name(dep_callee, "get_db"),
            "the provider is called by name"
        );
        let Stmt::Return {
            value: Some(Expr::Call { callee, args, .. }),
            ..
        } = stmts.last().unwrap()
        else {
            panic!("expected `return greet$impl(...)`");
        };
        assert!(is_name(callee, "greet$impl"));
        assert_eq!(args.len(), 3);
        assert!(
            is_name(&args[0].value, &ctx_name),
            "ctx from the dispatcher"
        );
        assert!(
            is_name(&args[1].value, "get_db$dep"),
            "db from the resolved provider local"
        );
        assert_eq!(
            getter_call(&args[2].value),
            Some(("get_str".into(), "name".into())),
            "name from the namespace"
        );
    }

    #[test]
    fn namespace_adapter_depends_as_first_param_leaves_context_unused() {
        // #1646: a `Depends` first parameter is injected, not treated as the
        // context (the context is the first UNMARKED non-scalar). The scalar still
        // binds from the namespace.
        let mut m = module(
            "struct Context:\n    str command_name\n\
             struct DB:\n    int64 conn\n\
             def get_db() -> DB:\n    return DB(conn=1)\n\
             struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Context, Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(DB db = Depends(get_db), str name) -> str:\n    return name\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        let stmts = body(adapter);
        assert!(
            let_call(&stmts[0], "get_db$dep").is_some(),
            "the first parameter's Depends is injected"
        );
        let Stmt::Return {
            value: Some(Expr::Call { args, .. }),
            ..
        } = stmts.last().unwrap()
        else {
            panic!("expected tail return");
        };
        assert_eq!(args.len(), 2);
        assert!(is_name(&args[0].value, "get_db$dep"));
        assert_eq!(
            getter_call(&args[1].value),
            Some(("get_str".into(), "name".into()))
        );
    }

    #[test]
    fn namespace_adapter_rejects_request_source_param() {
        // #1646: `@request_source` injection resolves against a request value; an
        // argparse-backed handler has none, so such a parameter is a precise error
        // rather than a miscompile.
        let mut m = module(
            "struct Req:\n    str path\n\
             @request_source\ndef Query(Req r, str name) -> str:\n    return r.path\n\
             struct Context:\n    str command_name\n\
             struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Context, Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(Context ctx, str q = Query()) -> str:\n    return q\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("`q`") && d.message.contains("request")),
            "expected a request-source-not-available error for `q`, got: {diags:?}"
        );
    }

    // ---- #1655: non-numeric scalar getters (char / bytes) --------------------

    #[test]
    fn char_and_bytes_params_bind_via_dedicated_getters() {
        // #1655: `char`/`bytes` read a dedicated typed getter (`get_char`/
        // `get_bytes`) that returns the final type directly — no conversion
        // wrapper, unlike the narrowing numeric scalars (#1625).
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(char c, bytes b) -> str:\n    return \"x\"\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        let Stmt::Return {
            value: Some(Expr::Call { args, .. }),
            ..
        } = &body(adapter)[0]
        else {
            panic!("expected return call");
        };
        // Read directly with the dedicated getter — not wrapped in a conversion
        // builtin (so `getter_call`, which rejects a wrapper, matches).
        assert_eq!(
            getter_call(&args[0].value),
            Some(("get_char".into(), "c".into()))
        );
        assert_eq!(
            getter_call(&args[1].value),
            Some(("get_bytes".into(), "b".into()))
        );
    }

    #[test]
    fn bigint_and_complex_params_bind_via_dedicated_getters() {
        // #1657: `bigint`/`complex` now read a dedicated typed getter
        // (`get_bigint`/`get_complex`) that parses the argument string — like
        // `char`/`bytes`, read directly with no conversion wrapper.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(bigint n, complex z) -> str:\n    return \"x\"\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        let Stmt::Return {
            value: Some(Expr::Call { args, .. }),
            ..
        } = &body(adapter)[0]
        else {
            panic!("expected return call");
        };
        assert_eq!(
            getter_call(&args[0].value),
            Some(("get_bigint".into(), "n".into()))
        );
        assert_eq!(
            getter_call(&args[1].value),
            Some(("get_complex".into(), "z".into()))
        );
    }

    // ---- #1647: uniform generic seam (repeatable decorators) -----------------

    #[test]
    fn generic_seam_adapts_handler_with_inferred_context() {
        // #1647: a receiver method whose handler parameter is typed as its own
        // generic (`Inner previous`) is a uniform generic seam. The innermost use
        // adapts the handler into it (so scalars/context still work), the adapter
        // being passed to the generic parameter BY NAME. The context is inferred
        // from the handler's first non-scalar parameter (the seam carries none).
        let mut m = module(
            "struct Context:\n    str command_name\n\
             struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command[Inner](self, Inner previous, str name = \"\") -> Command:\n        return Command()\n\
             @cli.command\ndef greet(Context ctx, str who) -> str:\n    return who\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized for a generic seam");
        assert_eq!(
            adapter.params.len(),
            2,
            "context inferred from the handler => (Context, Namespace) adapter"
        );
        assert!(
            matches!(&adapter.params[0].ty, Some(Type::Named { name, .. }) if name == "Context"),
            "the inferred context type is the handler's first non-scalar parameter type"
        );
        // The adapter is passed to the generic seam parameter `previous` by name.
        let args = rebind_call_args(&main_stmts(&m)[1], "greet").expect("greet rebound");
        assert!(
            args.iter().any(|a| a.name.as_deref() == Some("previous")
                && matches!(&a.value, Expr::Name(n, _) if n == "greet$adapter")),
            "adapter bound to the generic seam `previous` by keyword: {args:?}"
        );
        // Body: `return greet$impl($ctx, $ns.get_str("who"))`.
        let ctx_name = adapter.params[0].name.clone();
        let Stmt::Return {
            value: Some(Expr::Call { args: cargs, .. }),
            ..
        } = &body(adapter)[0]
        else {
            panic!("expected a return call");
        };
        assert!(is_name(&cargs[0].value, &ctx_name), "ctx passed through");
        assert_eq!(
            getter_call(&cargs[1].value),
            Some(("get_str".into(), "who".into()))
        );
    }

    #[test]
    fn generic_type_value_param_is_not_a_seam() {
        // #1647 guard: a decorator with a raw `fn(...)` handler (#1527) and a bare
        // generic TYPE-VALUE parameter (`T kind`, which receives a type like
        // `int64`) must NOT treat the generic parameter as the handler seam — the
        // raw handler owns the slot, so no adapter is synthesized.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def option[T](self, str name, T kind, fn(str) -> str handler) -> Command:\n        return Command()\n\
             @cli.option(\"n\", int64)\ndef greet(str name) -> str:\n    return name\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        assert!(
            func(&m, "greet$adapter").is_none(),
            "a raw-handler decorator with a generic type-value parameter gets no adapter"
        );
        // The raw handler is threaded as the trailing positional argument, unchanged.
        let args = rebind_call_args(&main_stmts(&m)[1], "greet").expect("greet rebound");
        assert!(
            args.last()
                .is_some_and(|a| a.name.is_none()
                    && matches!(&a.value, Expr::Name(n, _) if n == "greet$impl")),
            "raw handler stays the trailing positional: {args:?}"
        );
    }

    #[test]
    fn generic_seam_context_free_handler() {
        // #1647: a generic-seam handler with only scalar parameters infers NO
        // context, so the adapter is the plain `(Namespace)` shape and every
        // parameter binds from the namespace.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command[Inner](self, Inner previous) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(str who) -> str:\n    return who\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let adapter = func(&m, "greet$adapter").expect("adapter synthesized");
        assert_eq!(
            adapter.params.len(),
            1,
            "no context inferred => a plain (Namespace) adapter"
        );
        assert!(
            matches!(&adapter.params[0].ty, Some(Type::Named { name, .. }) if name == "Namespace")
        );
    }

    #[test]
    fn generic_seam_stacks_bottom_up() {
        // #1647: two uniform-generic-seam decorators compose. The innermost adapts
        // the handler; the outer receives the inner decorator's returned object
        // (threaded as the trailing argument into its own generic seam).
        let mut m = module(
            "struct Context:\n    str command_name\n\
             struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command[Inner](self, Inner previous, str name = \"\") -> Command:\n        return Command()\n\
             struct Cli2:\n    def argument[Inner](self, str name, Inner previous) -> Command:\n        return Command()\n\
             @cli.command\n@arg.argument(\"first\")\ndef greet(Context ctx, str who) -> str:\n    return who\n\
             def main():\n    auto cli = Cli()\n    auto arg = Cli2()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        // The innermost (`arg.argument`) adapts the handler.
        assert!(
            func(&m, "greet$adapter").is_some(),
            "innermost generic seam synthesizes the adapter"
        );
        // The rebind is the OUTER `cli.command(...)` call, threading the inner
        // `arg.argument(...)` call as its trailing argument.
        let (callee, args) = main_stmts(&m)
            .iter()
            .find_map(|s| match s {
                Stmt::Let {
                    name,
                    value: Expr::Call { callee, args, .. },
                    ..
                } if name == "greet" => Some((callee, args)),
                _ => None,
            })
            .expect("greet rebound to a call");
        assert!(
            matches!(callee.as_ref(), Expr::Attr { name, .. } if name == "command"),
            "outermost wrap is cli.command"
        );
        assert!(
            args.iter()
                .any(|a| matches!(&a.value, Expr::Call { callee, .. }
                if matches!(callee.as_ref(), Expr::Attr { name, .. } if name == "argument"))),
            "the composed inner argument(...) object is threaded into command(...): {args:?}"
        );
    }

    // ---- #1665: decl name defaults an omitted `name` on the outermost decorator --

    // The string value of a keyword argument `kw` in `args`, if present.
    fn kw_str<'a>(args: &'a [Arg], kw: &str) -> Option<&'a str> {
        args.iter().find_map(|a| match (&a.name, &a.value) {
            (Some(n), Expr::Str { value, .. }) if n == kw => Some(value.as_str()),
            _ => None,
        })
    }

    #[test]
    fn omitted_name_defaults_to_decl_name() {
        // #1665: `@cli.command` with no `name=` defaults the receiver's `str name`
        // parameter to the decorated declaration's name (`"greet"`), Click-style.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, str name = \"\", fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(str who) -> str:\n    return who\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let args = rebind_call_args(&main_stmts(&m)[1], "greet").expect("greet rebound");
        assert_eq!(
            kw_str(args, "name"),
            Some("greet"),
            "the omitted `name` defaults to the decl name: {args:?}"
        );
        // The handler is still passed by name alongside it.
        assert!(args.iter().any(|a| a.name.as_deref() == Some("handler")
            && matches!(&a.value, Expr::Name(n, _) if n == "greet$adapter")));
    }

    #[test]
    fn explicit_name_is_not_overwritten() {
        // An explicit `name=` wins; the decl name does not override it, and no
        // duplicate `name` argument is added.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, str name = \"\", fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command(name=\"alias\")\ndef greet(str who) -> str:\n    return who\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let args = rebind_call_args(&main_stmts(&m)[1], "greet").expect("greet rebound");
        assert_eq!(kw_str(args, "name"), Some("alias"), "explicit name kept");
        assert_eq!(
            args.iter()
                .filter(|a| a.name.as_deref() == Some("name"))
                .count(),
            1,
            "no duplicate `name` argument: {args:?}"
        );
    }

    #[test]
    fn no_name_param_is_unaffected() {
        // A receiver with no `str name` parameter gets no synthesized `name`.
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             @cli.command\ndef greet(str who) -> str:\n    return who\n\
             def main():\n    auto cli = Cli()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        let args = rebind_call_args(&main_stmts(&m)[1], "greet").expect("greet rebound");
        assert!(
            !args.iter().any(|a| a.name.as_deref() == Some("name")),
            "no `name` argument synthesized when the receiver has no `str name`: {args:?}"
        );
    }

    #[test]
    fn decl_name_fills_only_the_outermost_decorator() {
        // #1665: in a stack, only the OUTERMOST decorator's omitted `name` defaults
        // to the decl name; an inner decorator's omitted `name` keeps its own
        // default (its command/argument name is its own concern).
        let mut m = module(
            "struct Command:\n    def run(self):\n        print(\"ran\")\n\
             struct Cli:\n    def command(self, str name = \"\", fn(Namespace) -> str handler) -> Command:\n        return Command()\n\
             struct Wrap:\n    def wrap(self, Command inner, str name = \"\") -> Command:\n        return inner\n\
             @w.wrap\n@cli.command\ndef greet(str who) -> str:\n    return who\n\
             def main():\n    auto cli = Cli()\n    auto w = Wrap()\n    greet.run()\n",
        );
        let diags = desugar_registering_decorators(&mut m, &HashMap::new());
        assert!(diags.is_empty(), "no diagnostics expected: {diags:?}");
        // Outer `w.wrap(...)` call (greet's rebind) carries name="greet".
        let outer = main_stmts(&m)
            .iter()
            .find_map(|s| match s {
                Stmt::Let {
                    name,
                    value: Expr::Call { callee, args, .. },
                    ..
                } if name == "greet" => Some((callee, args)),
                _ => None,
            })
            .expect("greet rebound");
        assert!(
            matches!(outer.0.as_ref(), Expr::Attr { name, .. } if name == "wrap"),
            "outermost is w.wrap"
        );
        assert_eq!(
            kw_str(outer.1, "name"),
            Some("greet"),
            "outermost gets the decl name"
        );
        // The inner `cli.command(...)` object threaded into wrap has NO `name`.
        let inner_args = outer
            .1
            .iter()
            .find_map(|a| match &a.value {
                Expr::Call { callee, args, .. }
                    if matches!(callee.as_ref(), Expr::Attr { name, .. } if name == "command") =>
                {
                    Some(args)
                }
                _ => None,
            })
            .expect("inner command(...) threaded");
        assert!(
            !inner_args.iter().any(|a| a.name.as_deref() == Some("name")),
            "the inner decorator's `name` is NOT auto-filled: {inner_args:?}"
        );
    }
}
