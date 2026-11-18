//! Module-qualified construction of seam-exposed prelude *generic* collections
//! (I#996, D-0202).
//!
//! A handful of prelude collections are real generics (`struct Queue[T]`, …) but
//! are *built* through a bare prelude free function (`new_queue[T]()`), because a
//! module-qualified constructor call carries no type argument and the
//! monomorphizer keys on **bare** names (D-0078). This module lets the natural
//! dotted spelling
//!
//! ```text
//!     concurrent.Queue[int64] q = concurrent.Queue()     # T from the annotation
//!     concurrent.Queue[str]   q = concurrent.Queue[str]()  # explicit
//! ```
//!
//! work as pure **front-end sugar**: the dotted type head `concurrent.Queue` is
//! normalized to the bare prelude type `Queue`, and the dotted constructor callee
//! `concurrent.Queue[T](...)` / `concurrent.Queue(...)` is rewritten to the bare
//! `new_queue[T](...)` / `new_queue(...)`. Everything downstream (the sema
//! generic-call path, the monomorphizer, lowering) then runs unchanged on the
//! bare names it already understands — this adds **no** new keying dimension to
//! the monomorphizer. The bare `new_queue[T]()` form keeps working verbatim
//! (back-compat, D-0202).
//!
//! The table is the single source of truth, shared by the sema facet
//! (`zapc_sema`: type-annotation resolution + the constructor-call type-check)
//! and the monomorphizer (`zapc_mono`: the same two rewrites on the lowering
//! path), mirroring the existing twin-facet idiom (e.g. the `concurrent` module
//! check/lower facets). Adding another seam generic — once its natural dotted
//! spelling is decided (a maintainer design call; the `ConcurrentSet`/
//! `ConcurrentStack` siblings are a documented follow-up, D-0202) — is a single
//! row here.

use crate::ast::Expr;

/// The seam-exposed prelude generic collections that admit module-qualified
/// construction `module.Type[T](...)`, each mapped to the bare prelude free
/// constructor the monomorphizer keys on.
///
/// `(module, Type, bare free constructor)`.
pub const SEAM_GENERIC_CTORS: &[(&str, &str, &str)] = &[
    // The lock-free MPSC `Queue[T]` (I#184, D-0078) — built by `new_queue[T]()`.
    ("concurrent", "Queue", "new_queue"),
    // The keep-alive HTTP client `http.Client(...)` (#990) — a non-generic
    // prelude struct built by the prelude factory `__http_client(...)`, which
    // creates the connection pool. Same callee-rewrite sugar: `http.Client(...)`
    // → `__http_client(...)`, args (the config keywords) carried through. Used
    // via `auto client = http.Client(...)`, so the dotted type head is not spelled
    // as an annotation (the bare prelude type is `HttpClient`, not `Client`).
    ("http", "Client", "__http_client"),
];

/// If the dotted type head `dotted` (`"concurrent.Queue"`) names a seam generic,
/// the bare prelude type name it stands for (`"Queue"`). A non-dotted or
/// unrecognized name yields `None`, so callers fall through to ordinary
/// resolution unchanged.
pub fn seam_generic_bare_type(dotted: &str) -> Option<&'static str> {
    let (module, ty) = dotted.split_once('.')?;
    SEAM_GENERIC_CTORS
        .iter()
        .find(|(m, t, _)| *m == module && *t == ty)
        .map(|(_, t, _)| *t)
}

/// If `module.ty` names a seam generic, its bare prelude free constructor
/// (`("concurrent", "Queue") -> "new_queue"`).
pub fn seam_generic_ctor(module: &str, ty: &str) -> Option<&'static str> {
    SEAM_GENERIC_CTORS
        .iter()
        .find(|(m, t, _)| *m == module && *t == ty)
        .map(|(_, _, ctor)| *ctor)
}

/// Rewrite a seam module-qualified generic *constructor callee* to its bare
/// prelude free constructor, or `None` if `callee` is not such a form:
///
/// - `concurrent.Queue`      → `new_queue`        (T inferred from the sink)
/// - `concurrent.Queue[T]`   → `new_queue[T]`     (explicit type argument)
///
/// Only the callee shape is matched here; the value arguments and the explicit
/// type-argument subscript are carried through verbatim, so argument type-checks
/// and the existing generic-call machinery apply unchanged.
pub fn desugar_seam_ctor_callee(callee: &Expr) -> Option<Expr> {
    match callee {
        // `concurrent.Queue(...)` — the module-qualified constructor, no explicit
        // type argument (T is inferred from the binding's annotation).
        Expr::Attr { base, name, span } => {
            if let Expr::Name(module, _) = base.as_ref() {
                let ctor = seam_generic_ctor(module, name)?;
                return Some(Expr::Name(ctor.to_string(), *span));
            }
            None
        }
        // `concurrent.Queue[T](...)` — explicit type argument on the dotted
        // constructor; preserve the `[T]` subscript onto the bare constructor.
        Expr::Index { base, index, span } => {
            if let Expr::Attr {
                base: mbase,
                name,
                span: aspan,
            } = base.as_ref()
            {
                if let Expr::Name(module, _) = mbase.as_ref() {
                    let ctor = seam_generic_ctor(module, name)?;
                    return Some(Expr::Index {
                        base: Box::new(Expr::Name(ctor.to_string(), *aspan)),
                        index: index.clone(),
                        span: *span,
                    });
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn name(s: &str) -> Expr {
        Expr::Name(s.to_string(), Span::empty_at(0))
    }

    #[test]
    fn bare_type_lookup() {
        assert_eq!(seam_generic_bare_type("concurrent.Queue"), Some("Queue"));
        assert_eq!(seam_generic_bare_type("concurrent.Counter"), None);
        assert_eq!(seam_generic_bare_type("Queue"), None);
        assert_eq!(seam_generic_bare_type("other.Queue"), None);
        assert_eq!(seam_generic_bare_type("a.b.Queue"), None);
    }

    #[test]
    fn ctor_lookup() {
        assert_eq!(seam_generic_ctor("concurrent", "Queue"), Some("new_queue"));
        assert_eq!(seam_generic_ctor("concurrent", "Counter"), None);
        assert_eq!(seam_generic_ctor("other", "Queue"), None);
    }

    #[test]
    fn desugar_inferred_form() {
        // concurrent.Queue  ->  new_queue
        let callee = Expr::Attr {
            base: Box::new(name("concurrent")),
            name: "Queue".to_string(),
            span: Span::empty_at(0),
        };
        match desugar_seam_ctor_callee(&callee) {
            Some(Expr::Name(n, _)) => assert_eq!(n, "new_queue"),
            other => panic!("expected bare new_queue, got {other:?}"),
        }
    }

    #[test]
    fn desugar_explicit_form() {
        // concurrent.Queue[str]  ->  new_queue[str]
        let callee = Expr::Index {
            base: Box::new(Expr::Attr {
                base: Box::new(name("concurrent")),
                name: "Queue".to_string(),
                span: Span::empty_at(0),
            }),
            index: Box::new(name("str")),
            span: Span::empty_at(0),
        };
        match desugar_seam_ctor_callee(&callee) {
            Some(Expr::Index { base, index, .. }) => {
                assert!(matches!(base.as_ref(), Expr::Name(n, _) if n == "new_queue"));
                assert!(matches!(index.as_ref(), Expr::Name(n, _) if n == "str"));
            }
            other => panic!("expected new_queue[str], got {other:?}"),
        }
    }

    #[test]
    fn non_seam_callees_untouched() {
        // A non-seam module attribute, and a bare name, are left alone.
        let counter = Expr::Attr {
            base: Box::new(name("concurrent")),
            name: "Counter".to_string(),
            span: Span::empty_at(0),
        };
        assert!(desugar_seam_ctor_callee(&counter).is_none());
        assert!(desugar_seam_ctor_callee(&name("new_queue")).is_none());
    }
}
