//! `bisect` module — sema facet (ticket #102, generalized #983).
//!
//! Binary search and sorted insertion over an existing sorted `list[T]` (Python
//! `bisect`). `bisect_left`/`bisect_right` return the insertion point;
//! `insort_left`/`insort_right` insert in place keeping the list sorted. The
//! list argument must be a `list[T]` whose element type `T` is `Comparable`
//! (integer, float64, str, or char — the orderable primitives the runtime
//! compares); the value argument must be assignable to `T`. The caller is
//! responsible for the list being sorted.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{assignable, expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "bisect",
    check,
};

/// Whether `t` is an element type `bisect` can order at runtime (the orderable
/// primitives: any integer width, `float64`, `str`, `char`). `Unknown` is
/// permissive so inference gaps do not cascade.
fn is_bisect_comparable(t: &Ty) -> bool {
    t.is_int()
        || matches!(
            t,
            Ty::Float64 | Ty::UntypedFloat | Ty::Str | Ty::Char | Ty::Unknown
        )
}

/// Check the shared `(xs: list[T], x: T)` signature: arg0 is a `list` whose
/// element is `Comparable`, arg1 a value assignable to that element. Both
/// argument expressions are inferred so they are still checked.
fn check_list_and_value(checker: &mut Checker, method: &str, args: &[Arg], span: Span) {
    if args.len() != 2 {
        checker.error(
            ErrorCode::ZAP4025,
            format!(
                "bisect.{method}(xs, x) takes 2 arguments, found {}",
                args.len()
            ),
            span,
        );
        for a in args {
            checker.infer(&a.value);
        }
        return;
    }
    let t = checker.infer(&args[0].value);
    let vt = checker.infer(&args[1].value);
    match &t {
        Ty::List(elem) => {
            if !is_bisect_comparable(elem) {
                checker.error(
                    ErrorCode::ZAP4024,
                    format!(
                        "bisect.{method}() needs a list of a comparable element \
                         (integer, float64, str, or char), found list[{}]",
                        elem.display()
                    ),
                    expr_span(&args[0].value),
                );
            } else if !assignable(&vt, elem) && !matches!(vt, Ty::Unknown) {
                checker.error(
                    ErrorCode::ZAP4024,
                    format!(
                        "bisect.{method}() value has type {} but the list holds {}",
                        vt.display(),
                        elem.display()
                    ),
                    expr_span(&args[1].value),
                );
            }
        }
        Ty::Unknown => {}
        _ => checker.error(
            ErrorCode::ZAP4024,
            format!(
                "bisect.{method}() expects a list argument, found {}",
                t.display()
            ),
            expr_span(&args[0].value),
        ),
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // bisect.bisect_left(xs, x) / bisect_right(xs, x) -> int64 (insertion point).
        "bisect_left" | "bisect_right" => {
            check_list_and_value(checker, method, args, span);
            Some(Ty::Int64)
        }
        // bisect.insort_left(xs, x) / insort_right(xs, x): insert in place, no result.
        "insort_left" | "insort_right" => {
            check_list_and_value(checker, method, args, span);
            Some(Ty::Unit)
        }
        _ => None,
    }
}
