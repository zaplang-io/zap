//! `math` module — sema facet (ticket #98, D-0056).
//!
//! Elementary functions over the numeric tower, backed by libm. Arguments are
//! inferred (and widened to `f64` in lowering); every function returns `f64`
//! except `isnan`/`isinf`, which return `bool`. The module constants
//! (`pi`/`e`/`tau`/`inf`/`nan`) are *attributes* (`math.pi`), not calls, so they
//! are typed by [`const_ty`] through the const sub-registry in `super`.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck, ModuleConst};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "math",
    check,
};

pub(super) const MODULE_CONST: ModuleConst = ModuleConst {
    name: "math",
    const_ty,
};

/// Type of a `math` constant accessed as `math.<name>` (all `float64`).
fn const_ty(name: &str) -> Option<Ty> {
    match name {
        "pi" | "e" | "tau" | "inf" | "nan" => Some(Ty::Float64),
        _ => None,
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // One float argument -> float64.
        "sqrt" | "exp" | "log" | "log2" | "log10" | "sin" | "cos" | "tan" | "asin" | "acos"
        | "atan" | "floor" | "ceil" | "round" | "trunc" | "fabs" => {
            expect_argc(checker, "math", method, args, 1, span);
            Some(Ty::Float64)
        }
        // Two float arguments -> float64.
        "pow" | "atan2" | "hypot" | "fmod" => {
            expect_argc(checker, "math", method, args, 2, span);
            Some(Ty::Float64)
        }
        // One float argument -> bool.
        "isnan" | "isinf" => {
            expect_argc(checker, "math", method, args, 1, span);
            Some(Ty::Bool)
        }
        _ => None,
    }
}
