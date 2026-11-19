//! `decimal` module — sema (type-check) facet (ticket #981, #981).
//!
//! Exact base-10 arithmetic (money / billing). The `Decimal` reference struct
//! ships in the Zap prelude (`zapc_syntax::parser::PRELUDE_SRC`); this module
//! supplies the two constructors that build it:
//!
//!   * `decimal.Decimal(s)` — parse a `str`, or promote an `int` exactly.
//!   * `decimal.from_float(f)` — the *explicit* (opt-in) float conversion.
//!
//! A bare `decimal.Decimal(<float>)` is a **type error** (ZAP4024): binary
//! `float64` cannot represent most base-10 values, so a silent float → Decimal
//! conversion would reinstate exactly the rounding hazard the type exists to
//! remove. The opt-in `from_float` makes the lossful hop explicit and greppable
//! (it converts through the float's shortest round-trip decimal string).
//!
//! Everything else about a `Decimal` — `+ - * /`, `quantize`, comparisons,
//! `str()` — is pure prelude-struct Zap (dunder operators + ordinary methods
//! with keyword/default arguments, I#481), so it needs no module seam entry and
//! no runtime C.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "decimal",
    check,
};

/// The prelude `Decimal` reference struct type.
fn decimal_ty() -> Ty {
    Ty::Named("Decimal".to_string())
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // `decimal.Decimal(value)` — construct from a `str` (parsed, raises
        // ValueError on a malformed literal) or an `int` (exact). A `float`
        // argument is rejected: use `decimal.from_float` for the explicit,
        // lossful conversion.
        "Decimal" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "decimal.Decimal(value) takes exactly 1 argument, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
                return Some(decimal_ty());
            }
            let t = checker.infer(&args[0].value);
            match t {
                Ty::Str => {}
                Ty::Int32 | Ty::Int64 | Ty::UntypedInt => {}
                Ty::Float32 | Ty::Float64 | Ty::UntypedFloat => checker.error(
                    ErrorCode::ZAP4024,
                    "decimal.Decimal() refuses a float argument: binary float cannot \
                     represent most base-10 values exactly. Construct from a str \
                     (decimal.Decimal(\"1.50\")) or an int, or opt in to the lossful \
                     conversion with decimal.from_float(x)."
                        .to_string(),
                    expr_span(&args[0].value),
                ),
                Ty::Unknown => {}
                other => checker.error(
                    ErrorCode::ZAP4024,
                    format!(
                        "decimal.Decimal() expects a str or int argument, found {}",
                        other.display()
                    ),
                    expr_span(&args[0].value),
                ),
            }
            Some(decimal_ty())
        }
        // `decimal.from_float(f)` — the explicit opt-in float conversion. It goes
        // through the float's shortest round-trip decimal string (so
        // `from_float(0.1)` is `Decimal("0.1")`, matching Go's shopspring
        // NewFromFloat, NOT Python's exact-binary expansion — documented).
        "from_float" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "decimal.from_float(f) takes exactly 1 argument, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
                return Some(decimal_ty());
            }
            let t = checker.infer(&args[0].value);
            if !matches!(
                t,
                Ty::Float32 | Ty::Float64 | Ty::UntypedFloat | Ty::Unknown
            ) {
                checker.error(
                    ErrorCode::ZAP4024,
                    format!(
                        "decimal.from_float() expects a float argument, found {}",
                        t.display()
                    ),
                    expr_span(&args[0].value),
                );
            }
            Some(decimal_ty())
        }
        _ => None,
    }
}
