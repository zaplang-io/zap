//! `linalg` module — sema facet (#926, D-0183).
//!
//! Linear algebra over numeric arrays, LAPACK-backed (the #912 acceleration
//! seam). Surface, grown across slices:
//!   solve / inv / det (#926); cholesky / qr / svd (#927, decompositions);
//!   eigvals / eigh / eigvalsh / lstsq / norm / pinv / matrix_power (#928).
//! `eigvals` returns a `list[complex]`; qr/svd/eigh/lstsq return tuples; `norm`
//! returns an `f64` and takes an optional `ord`. `eig` (complex eigenvector
//! matrix) is deferred until a complex array dtype exists.
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
    // `norm` has an optional trailing `ord`: `norm(a)` or `norm(a, ord)` (#928).
    if method == "norm" {
        super::expect_argc_range(checker, "linalg", method, args, 1, 2, span);
        return Some(Ty::Float64);
    }
    let (argc, result) = match method {
        "solve" => (2, Ty::Unknown),
        "inv" => (1, Ty::Unknown),
        "det" => (1, Ty::Float64),
        // Decompositions (#927). Arrays type loosely as `Unknown`; qr/svd return a
        // tuple of arrays (`q, r = linalg.qr(a)` / `u, s, vt = linalg.svd(a)`).
        "cholesky" => (1, Ty::Unknown),
        "qr" => (1, Ty::Tuple(vec![Ty::Unknown, Ty::Unknown])),
        "svd" => (1, Ty::Tuple(vec![Ty::Unknown, Ty::Unknown, Ty::Unknown])),
        // Eigensolvers / least squares / pinv / matrix_power (#928).
        // `eigvals` (general) returns the eigenvalues as a `list[complex]` (real
        // matrices have real or conjugate-pair eigenvalues); `eig` itself is
        // deferred until a complex array dtype can hold the eigenvectors.
        "eigvals" => (1, Ty::List(Box::new(Ty::Complex))),
        // `eigh` (symmetric) returns (w, v): real eigenvalues ascending + real
        // eigenvector matrix; `eigvalsh` returns just w.
        "eigh" => (1, Ty::Tuple(vec![Ty::Unknown, Ty::Unknown])),
        "eigvalsh" => (1, Ty::Unknown),
        // `lstsq(a, b)` returns (solution, residuals, rank, singular_values);
        // rank is an int, the rest are arrays.
        "lstsq" => (
            2,
            Ty::Tuple(vec![Ty::Unknown, Ty::Unknown, Ty::Int64, Ty::Unknown]),
        ),
        "pinv" => (1, Ty::Unknown),
        // `matrix_power(a, p)` — a square matrix raised to an integer power.
        "matrix_power" => (2, Ty::Unknown),
        _ => return None,
    };
    super::expect_argc(checker, "linalg", method, args, argc, span);
    Some(result)
}
