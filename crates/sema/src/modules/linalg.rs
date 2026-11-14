//! `linalg` module — sema facet (#926, D-0183).
//!
//! Linear algebra over numeric arrays, LAPACK-backed (the #912 acceleration
//! seam). First slice:
//!   `linalg.solve(a, b) -> array`   (solve `a @ x == b`; `a` square, `b` 1-D/2-D)
//!   `linalg.inv(a) -> array`        (matrix inverse)
//!   `linalg.det(a) -> f64`          (determinant)
//!
//! Arrays are typed loosely (`Ty::Unknown`) in sema, as the rest of the array
//! surface is, so the arguments are only inferred (type-checked), not shape- or
//! dtype-constrained here; the runtime validates squareness / alignment and
//! raises a catchable `ValueError` on a bad shape or a singular matrix. Results
//! are computed in f64 (this slice; f32-precision is a follow-up).

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "linalg",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    let (argc, result) = match method {
        "solve" => (2, Ty::Unknown),
        "inv" => (1, Ty::Unknown),
        "det" => (1, Ty::Float64),
        _ => return None,
    };
    super::expect_argc(checker, "linalg", method, args, argc, span);
    Some(result)
}
