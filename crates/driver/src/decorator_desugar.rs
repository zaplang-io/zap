//! Registering function-decorator desugaring (I#457, D-0087).
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

use zapc_syntax::ast::{Arg, Expr, Item, Module, Stmt};
use zapc_syntax::span::Span;

/// One pending registration: the root receiver name whose binding we anchor the
/// call after (`app` in `app.get`), and the synthesized `recv.method(args, h)`
/// statement.
struct Registration {
    recv_root: String,
    stmt: Stmt,
}

/// Rewrite every registering (dotted) function decorator in `module` into its
/// registration call, in place. A no-op when there are none, so the ordinary
/// decorator-free program is unchanged.
pub fn desugar_registering_decorators(module: &mut Module) {
    let mut regs: Vec<Registration> = Vec::new();

    // Pass 1: strip dotted decorators off free functions, building a registration
    // per decorator in source order (so routes register in declaration order).
    for item in &mut module.items {
        let Item::Function(f) = item else { continue };
        if f.decorators.iter().all(|d| !is_registering(&d.name)) {
            continue;
        }
        let fn_name = f.name.clone();
        let mut kept = Vec::with_capacity(f.decorators.len());
        for d in std::mem::take(&mut f.decorators) {
            let Some((recv_expr, method, recv_root)) = split_registering(&d.name, d.span) else {
                kept.push(d);
                continue;
            };
            // recv.method(<decorator args>, <fn h>)
            let mut args = d.args.clone();
            args.push(Arg {
                name: None,
                value: Expr::Name(fn_name.clone(), d.span),
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

    if regs.is_empty() {
        return;
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
        desugar_registering_decorators(&mut m);

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
}
