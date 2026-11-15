//! `random` module — sema facet (ticket #98, D-0056; NumPy `Generator`
//! #930/#931, D-0189).
//!
//! Two coexisting surfaces under one module:
//!   * The pre-existing module-level xoshiro256** generator (Python `random`):
//!     `seed(n)`, `random()`, `uniform(a, b)`, `randint(a, b)`, `choice(seq)`,
//!     `shuffle(seq)`. A "sequence" is a `list[T]`.
//!   * A NumPy-compatible, **bit-exact** PCG64 array `Generator` (D-0189):
//!     `default_rng(seed)` -> an opaque handle; the draw methods take the handle
//!     as their first argument and reproduce `numpy.random.default_rng(seed)`'s
//!     stream value-for-value. A trailing `size=` keyword turns a scalar draw
//!     into an array (int or shape-tuple); without it the draw is a scalar.
//!
//! The handle is an opaque, pointer-free value typed as `bytes` (the deque /
//! crypto handle convention). The four method names shared with the Python
//! surface (`random` / `uniform` / `choice` / `shuffle`) dispatch to the NumPy
//! path when the first argument is a `Generator` handle; the NumPy-only names
//! always do. Arrays type loosely (`Ty::Unknown`) in sema, like the rest of the
//! array surface, so params/sizes are inferred (type-checked), not shape- or
//! dtype-constrained here; the runtime validates and the lowering emits the
//! concrete result.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "random",
    check,
};

/// A `Generator` handle is an opaque, pointer-free value typed as `bytes`.
const GENERATOR_TY: Ty = Ty::Bytes;

/// NumPy-only method names (always route to the `Generator` surface).
fn is_numpy_only(method: &str) -> bool {
    matches!(
        method,
        "default_rng"
            | "integers"
            | "standard_normal"
            | "normal"
            | "standard_exponential"
            | "exponential"
            | "standard_gamma"
            | "gamma"
            | "beta"
            | "poisson"
            | "binomial"
            | "permutation"
    )
}

/// Names shared with the Python surface — NumPy iff the first arg is a handle.
fn is_overloaded(method: &str) -> bool {
    matches!(method, "random" | "uniform" | "choice" | "shuffle")
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    // Route to the NumPy `Generator` surface for its own method names, or for an
    // overloaded name whose first argument is a `Generator` handle.
    let numpy = is_numpy_only(method)
        || (is_overloaded(method)
            && !args.is_empty()
            && matches!(checker.infer(&args[0].value), GENERATOR_TY));
    if numpy {
        return check_numpy(checker, method, args, span);
    }

    match method {
        // random.seed(n): reseed the module generator from an integer.
        "seed" => {
            expect_argc(checker, "random", "seed", args, 1, span);
            Some(Ty::Unit)
        }
        // random.random() -> float64 in [0, 1).
        "random" => {
            expect_argc(checker, "random", "random", args, 0, span);
            Some(Ty::Float64)
        }
        // random.uniform(a, b) -> float64 in [a, b].
        "uniform" => {
            expect_argc(checker, "random", "uniform", args, 2, span);
            Some(Ty::Float64)
        }
        // random.randint(a, b) -> int64 in [a, b] (both endpoints inclusive).
        "randint" => {
            expect_argc(checker, "random", "randint", args, 2, span);
            Some(Ty::Int64)
        }
        // random.choice(seq) -> element of the list.
        "choice" => {
            let ty = require_list(checker, "choice", args, span);
            Some(ty)
        }
        // random.shuffle(seq): permute the list in place.
        "shuffle" => {
            require_list(checker, "shuffle", args, span);
            Some(Ty::Unit)
        }
        _ => None,
    }
}

/// Whether the call carries a `size=` keyword (array form) vs a scalar draw.
fn has_size(args: &[Arg]) -> bool {
    args.iter().any(|a| a.name.as_deref() == Some("size"))
}

/// The NumPy `Generator` surface. Infers every argument (type-checking them),
/// enforces the positional arity, and returns the scalar element type for a
/// scalar draw or `Ty::Unknown` (a loosely-typed array) when `size=` is present.
fn check_numpy(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    let positional = args.iter().filter(|a| a.name.is_none()).count();
    // (min, max) positional arity; `size=`/`replace=`/`p=`/`endpoint=` are kw.
    let (min, max, scalar): (usize, usize, Ty) = match method {
        "default_rng" => (0, 1, GENERATOR_TY),
        "random" => (1, 1, Ty::Float64),
        "standard_normal" => (1, 1, Ty::Float64),
        "standard_exponential" => (1, 1, Ty::Float64),
        "uniform" => (3, 3, Ty::Float64),
        "normal" => (1, 3, Ty::Float64),
        "exponential" => (1, 2, Ty::Float64),
        "standard_gamma" => (2, 2, Ty::Float64),
        "gamma" => (2, 3, Ty::Float64),
        "beta" => (3, 3, Ty::Float64),
        "integers" => (3, 3, Ty::Int64),
        "poisson" => (1, 2, Ty::Int64),
        "binomial" => (3, 3, Ty::Int64),
        // choice(g, a): `a` is an int population -> int64 index/indices.
        "choice" => (2, 2, Ty::Int64),
        // shuffle(g, arr): in place, no value.
        "shuffle" => (2, 2, Ty::Unit),
        // permutation(g, x): x is an int or an array -> a new array.
        "permutation" => (2, 2, Ty::Unknown),
        _ => return None,
    };
    if positional < min || positional > max {
        let want = if min == max {
            format!("{min}")
        } else {
            format!("{min}..{max}")
        };
        checker.error(
            ErrorCode::ZAP4025,
            format!("random.{method}() takes {want} positional argument(s), found {positional}"),
            span,
        );
    }
    for a in args {
        checker.infer(&a.value);
    }
    // default_rng / shuffle / permutation have a fixed result; the draws return
    // their scalar element type, or a (loosely-typed) array when `size=` is set.
    let result = match method {
        "default_rng" => GENERATOR_TY,
        "shuffle" => Ty::Unit,
        "permutation" => Ty::Unknown,
        _ if has_size(args) => Ty::Unknown,
        _ => scalar,
    };
    Some(result)
}

/// Require a single `list[T]` argument, returning its element type `T` (or
/// `Unknown` on error). Shared by Python `choice` (which yields `T`) and
/// `shuffle`.
fn require_list(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Ty {
    if args.len() != 1 {
        checker.error(
            ErrorCode::ZAP4025,
            format!(
                "random.{method}(seq) takes 1 argument, found {}",
                args.len()
            ),
            span,
        );
        for a in args {
            checker.infer(&a.value);
        }
        return Ty::Unknown;
    }
    match checker.infer(&args[0].value) {
        Ty::List(elem) => *elem,
        Ty::Unknown => Ty::Unknown,
        other => {
            checker.error(
                ErrorCode::ZAP4024,
                format!("random.{method} expects a list, found {}", other.display()),
                expr_span(&args[0].value),
            );
            Ty::Unknown
        }
    }
}
