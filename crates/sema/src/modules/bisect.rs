//! `bisect` module — sema facet (ticket #102, D-0056).
//!
//! Binary search and sorted insertion over an existing `list[int64]` (Python
//! `bisect`). `bisect_left`/`bisect_right` return the insertion point;
//! `insort_left`/`insort_right` insert in place keeping the list sorted. The
//! list argument must be a `list` whose elements compare as `int64` for v1
//! (other element types are a documented follow-up); the value argument is an
//! integer. The caller is responsible for the list being sorted.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "bisect",
    check,
};

/// Check the shared `(xs: list, x: int)` signature: arg0 is a `list`, arg1 an
/// integer. Both argument expressions are inferred so they are still checked.
fn check_list_and_value(checker: &mut Checker, method: &str, args: &[Arg], span: Span) {
    if args.len() != 2 {
        checker.error(
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
    if !matches!(t, Ty::List(_)) {
        checker.error(
            format!(
                "bisect.{method}() expects a list argument, found {}",
                t.display()
            ),
            expr_span(&args[0].value),
        );
    }
    checker.infer(&args[1].value);
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
