//! `statistics` module — sema facet (#980, D-0243, epic #966).
//!
//! The ergonomic scalar-list descriptive-statistics surface (Python
//! `statistics`). Every function takes a numeric list — `list[float64]` OR
//! `list[int64]` (an integer list is read element-wise and promoted to `float`
//! at the runtime boundary) — and returns `float64`, except `multimode` /
//! `quantiles`, which return `list[float64]`, and `linear_regression`, which
//! returns a `(float64, float64)` tuple `(slope, intercept)`.
//!
//! All results are `float` (D-0243): unlike Python, `median_low`/`median_high`/
//! `mode` return the selected element's `float` value rather than the input
//! type — the module keeps one static return type. Empty / invalid input raises
//! `ValueError` (the StatisticsError-equivalent — Python's `StatisticsError`
//! subclasses `ValueError`). Built on list iteration + `math`, NOT on the
//! numeric-array reductions (#505).

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, expect_argc_range, ModuleCheck};
use crate::{expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "statistics",
    check,
};

/// Infer `arg` and require it to be a numeric list (`list[int]` / `list[float]`).
/// `Unknown` stays permissive so inference gaps do not cascade; any other type
/// is reported with `ZAP4024`.
fn check_num_list(checker: &mut Checker, method: &str, arg: &Arg) {
    let t = checker.infer(&arg.value);
    match &t {
        Ty::List(elem) if elem.is_int() || matches!(**elem, Ty::Float64 | Ty::UntypedFloat) => {}
        Ty::List(elem) if matches!(**elem, Ty::Unknown) => {}
        Ty::Unknown => {}
        Ty::List(elem) => checker.error(
            ErrorCode::ZAP4024,
            format!(
                "statistics.{method}() needs a list of int or float, found list[{}]",
                elem.display()
            ),
            expr_span(&arg.value),
        ),
        _ => checker.error(
            ErrorCode::ZAP4024,
            format!(
                "statistics.{method}() expects a numeric list, found {}",
                t.display()
            ),
            expr_span(&arg.value),
        ),
    }
}

/// Infer a trailing scalar argument so it is still type-checked (the optional
/// `xbar`/`mu` centre, `quantiles`' `n`, `linear_regression`'s `proportional`).
fn infer_scalar(checker: &mut Checker, arg: &Arg) {
    let _ = checker.infer(&arg.value);
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // Means + order statistics: one numeric list -> float64.
        "mean" | "fmean" | "geometric_mean" | "harmonic_mean" | "median" | "median_low"
        | "median_high" | "mode" => {
            expect_argc(checker, "statistics", method, args, 1, span);
            if let Some(a) = args.first() {
                check_num_list(checker, method, a);
            }
            Some(Ty::Float64)
        }
        // `multimode(xs)` -> list[float64] (every value tied for the mode).
        "multimode" => {
            expect_argc(checker, "statistics", method, args, 1, span);
            if let Some(a) = args.first() {
                check_num_list(checker, method, a);
            }
            Some(Ty::List(Box::new(Ty::Float64)))
        }
        // Spread with an optional centre: `variance(xs[, xbar])` etc. -> float64.
        "variance" | "pvariance" | "stdev" | "pstdev" => {
            expect_argc_range(checker, "statistics", method, args, 1, 2, span);
            if let Some(a) = args.first() {
                check_num_list(checker, method, a);
            }
            if let Some(c) = args.get(1) {
                infer_scalar(checker, c);
            }
            Some(Ty::Float64)
        }
        // `quantiles(xs[, n][, method])` -> list[float64] (n-1 cut points).
        "quantiles" => {
            expect_argc_range(checker, "statistics", method, args, 1, 3, span);
            if let Some(a) = args.first() {
                check_num_list(checker, method, a);
            }
            if let Some(n) = args.get(1) {
                infer_scalar(checker, n);
            }
            if let Some(m) = args.get(2) {
                infer_scalar(checker, m);
            }
            Some(Ty::List(Box::new(Ty::Float64)))
        }
        // Two-list relationships over equal-length numeric lists.
        "covariance" | "correlation" => {
            expect_argc(checker, "statistics", method, args, 2, span);
            if let Some(a) = args.first() {
                check_num_list(checker, method, a);
            }
            if let Some(b) = args.get(1) {
                check_num_list(checker, method, b);
            }
            Some(Ty::Float64)
        }
        // `linear_regression(xs, ys[, proportional])` -> (slope, intercept).
        "linear_regression" => {
            expect_argc_range(checker, "statistics", method, args, 2, 3, span);
            if let Some(a) = args.first() {
                check_num_list(checker, method, a);
            }
            if let Some(b) = args.get(1) {
                check_num_list(checker, method, b);
            }
            if let Some(p) = args.get(2) {
                infer_scalar(checker, p);
            }
            Some(Ty::Tuple(vec![Ty::Float64, Ty::Float64]))
        }
        _ => None,
    }
}
