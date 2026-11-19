//! `fractions` module — sema (type-check) facet (ticket #982, epic #966).
//!
//! Exact rational arithmetic. The `Fraction` reference struct ships in the Zap
//! prelude (`zapc_syntax::parser::PRELUDE_SRC`); this module supplies the
//! constructor that builds it:
//!
//!   * `fractions.Fraction(num, den)` — a rational from two `int`s (a zero
//!     denominator raises `ValueError`); auto-reduced to lowest terms.
//!   * `fractions.Fraction(num)` — one argument: an `int` (exact), or a `str`
//!     such as `"3/4"` or `"0.25"` (parsed; malformed → `ValueError`).
//!
//! A `float` argument is a **type error** (ZAP4024): binary `float64` cannot
//! represent most base-10 values, so a silent float → Fraction hop would
//! reinstate exactly the rounding hazard the exact type exists to remove.
//! Construct from a decimal `str` (`fractions.Fraction("0.25")`) instead
//! (matching the sibling `decimal` type's float rejection).
//!
//! Everything else about a `Fraction` — `+ - * /`, comparisons,
//! `numerator`/`denominator`, `limit_denominator`, `to_float`, `str()` — is
//! pure prelude-struct Zap (dunder operators + ordinary methods with
//! keyword/default arguments, I#481), so it needs no module seam entry and no
//! runtime C.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "fractions",
    check,
};

/// The prelude `Fraction` reference struct type.
fn fraction_ty() -> Ty {
    Ty::Named("Fraction".to_string())
}

/// Is `t` an integer word acceptable as a numerator/denominator component?
fn is_int(t: &Ty) -> bool {
    matches!(t, Ty::Int32 | Ty::Int64 | Ty::UntypedInt)
}

/// Emit the float-rejection diagnostic (ZAP4024) steering to the `str` form.
fn reject_float(checker: &mut Checker, a: &Arg) {
    checker.error(
        ErrorCode::ZAP4024,
        "fractions.Fraction() refuses a float argument: binary float cannot \
         represent most base-10 values exactly. Construct from a str \
         (fractions.Fraction(\"0.25\") / fractions.Fraction(\"3/4\")) or from \
         integers (fractions.Fraction(3, 4))."
            .to_string(),
        expr_span(&a.value),
    );
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        "Fraction" => {
            match args.len() {
                // `Fraction(value)` — a str (parsed) or an int (exact).
                1 => {
                    let t = checker.infer(&args[0].value);
                    match t {
                        Ty::Str => {}
                        _ if is_int(&t) => {}
                        Ty::Float32 | Ty::Float64 | Ty::UntypedFloat => {
                            reject_float(checker, &args[0])
                        }
                        Ty::Unknown => {}
                        other => checker.error(
                            ErrorCode::ZAP4024,
                            format!(
                                "fractions.Fraction() expects a str or int argument, found {}",
                                other.display()
                            ),
                            expr_span(&args[0].value),
                        ),
                    }
                }
                // `Fraction(num, den)` — two ints; den == 0 raises at runtime.
                2 => {
                    for a in args {
                        let t = checker.infer(&a.value);
                        match t {
                            _ if is_int(&t) => {}
                            Ty::Float32 | Ty::Float64 | Ty::UntypedFloat => {
                                reject_float(checker, a)
                            }
                            Ty::Unknown => {}
                            other => checker.error(
                                ErrorCode::ZAP4024,
                                format!(
                                    "fractions.Fraction(num, den) expects int arguments, found {}",
                                    other.display()
                                ),
                                expr_span(&a.value),
                            ),
                        }
                    }
                }
                n => {
                    checker.error(
                        ErrorCode::ZAP4025,
                        format!(
                            "fractions.Fraction() takes 1 (int or str) or 2 (int, int) \
                             arguments, found {n}"
                        ),
                        span,
                    );
                    for a in args {
                        checker.infer(&a.value);
                    }
                }
            }
            Some(fraction_ty())
        }
        _ => None,
    }
}
