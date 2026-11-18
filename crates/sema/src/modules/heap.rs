//! `heap` module — sema facet (ticket #978, D-0236).
//!
//! The `heap[T]` *type* (construction, O(n) heapify via `heap[T](xs)`, and the
//! `push`/`pop`/`peek`/`replace`/`pushpop` methods) is checked in `lib.rs`. This
//! file hosts only the two free FUNCTIONS that take an iterable and return a
//! fresh `list[T]`, mirroring Python's `heapq.nlargest` / `heapq.nsmallest`:
//!
//! ```text
//!     heap.nlargest(n, xs)            # the n largest, descending
//!     heap.nsmallest(n, xs)          # the n smallest, ascending
//!     heap.nlargest(n, xs, key=f)    # ordered by f(x), a fn(T) -> K selector
//! ```
//!
//! Both return `list[T]` (the element type of the iterable). Without `key=`, the
//! element type must itself be orderable at runtime; with `key=`, the element
//! type is unconstrained and the key's result type carries the ordering. Fn
//! values currently type as `Unknown` in sema (first-class-fn typing is loose,
//! D-0067), so the `key=` argument is inferred but not shape-checked here — the
//! lowering resolves its real `fn(T) -> K` signature and rejects a non-orderable
//! key. `n` must be an integer; it is clamped to `[0, len]` at runtime (a
//! negative or oversize `n` is not an error, matching Python).

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "heap",
    check,
};

/// Whether `t` is an element type the heap can order directly (no `key=`): any
/// integer width, `float64`, `str`, `char`, `bool`, or a named type (an enum is
/// ordered by its tier-1 discriminant; a non-orderable named type such as a
/// struct is rejected by the lowering with the exact kind). `Unknown` is
/// permissive so inference gaps do not cascade into spurious errors.
fn is_orderable(t: &Ty) -> bool {
    t.is_int()
        || matches!(
            t,
            Ty::Float64
                | Ty::UntypedFloat
                | Ty::Str
                | Ty::Char
                | Ty::Bool
                | Ty::Named(_)
                | Ty::Unknown
        )
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // heap.nlargest(n, xs[, key]) / heap.nsmallest(n, xs[, key]) -> list[T].
        "nlargest" | "nsmallest" => {
            if args.len() < 2 || args.len() > 3 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "heap.{method}(n, iterable, key=...) takes 2 arguments plus \
                         an optional key, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
                return Some(Ty::List(Box::new(Ty::Unknown)));
            }
            let nt = checker.infer(&args[0].value);
            if !nt.is_int() && !matches!(nt, Ty::Unknown) {
                checker.error(
                    ErrorCode::ZAP4024,
                    format!(
                        "heap.{method}() count must be an integer, found {}",
                        nt.display()
                    ),
                    expr_span(&args[0].value),
                );
            }
            let it = checker.infer(&args[1].value);
            // A `key=` selector is inferred (so it is still checked) but its
            // precise `fn(T) -> K` shape is validated in lowering (fn values are
            // loosely typed here).
            let key_given = args.len() == 3;
            if key_given {
                checker.infer(&args[2].value);
            }
            match &it {
                Ty::List(elem) => {
                    if !key_given && !is_orderable(elem) {
                        checker.error(
                            ErrorCode::ZAP4024,
                            format!(
                                "heap.{method}() needs a list of an orderable element \
                                 (integer, bool, char, enum, float64, or str), or a \
                                 key= selecting an orderable key, found list[{}]",
                                elem.display()
                            ),
                            expr_span(&args[1].value),
                        );
                    }
                    Some(Ty::List(elem.clone()))
                }
                Ty::Unknown => Some(Ty::List(Box::new(Ty::Unknown))),
                _ => {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!(
                            "heap.{method}() expects a list as its iterable argument, \
                             found {}",
                            it.display()
                        ),
                        expr_span(&args[1].value),
                    );
                    Some(Ty::List(Box::new(Ty::Unknown)))
                }
            }
        }
        _ => None,
    }
}
