//! `math` module — sema facet (ticket #98, D-0056; scalar additions #973, D-0234).
//!
//! Elementary functions over the numeric tower, backed by libm. The float
//! transcendentals infer their argument (widened to `f64` in lowering) and
//! return `f64`; the classification predicates (`isnan`/`isinf`/`isfinite`,
//! `isclose`) return `bool`. The module constants (`pi`/`e`/`tau`/`inf`/`nan`)
//! are *attributes* (`math.pi`), not calls, typed by [`const_ty`].
//!
//! #973 adds the everyday scalars missing from the elementary set:
//!   * **integer math** — `gcd`/`lcm`/`factorial`/`comb`/`perm`/`isqrt` take
//!     integer arguments and return `int` (a float argument is rejected);
//!   * **angles / hyperbolics / accurate variants / special functions** — more
//!     unary `f64 -> f64` libm wrappers (`degrees`/`radians`, the `*h`/`a*h`
//!     set, `expm1`/`log1p`/`exp2`/`cbrt`, `gamma`/`lgamma`/`erf`/`erfc`);
//!   * **classify / compare / compose** — `isfinite`, `isclose(a, b,
//!     rel_tol=1e-9, abs_tol=0.0)`, `copysign`/`nextafter`/`remainder`,
//!     `ldexp`, and the tuple-returning `modf` (`(float, float)`) / `frexp`
//!     (`(float, int)`);
//!   * **list reductions** — `prod`/`sumprod` are element-typed (an `int` list
//!     yields `int`, a `float` list `float`), `fsum`/`dist` take `float` lists
//!     and return `float`.
//!
//! `floor`/`ceil` keep returning `f64` (not `int` as in Python): the scalar and
//! the #505 array-mapped forms stay one type, and `int(math.floor(x))` provides
//! the integer when wanted (D-0234).

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, expect_argc_range, ModuleCheck, ModuleConst};
use crate::{expr_span, Checker, Ty};

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

/// Infer every argument (so each is still type-checked) and reject any whose
/// type is concretely non-integer — the integer-math group (`gcd`, `factorial`,
/// …) is defined on `int` only. `Unknown`/`UntypedInt` stay permissive so
/// inference gaps do not cascade into spurious errors.
fn expect_int_args(checker: &mut Checker, method: &str, args: &[Arg]) {
    for a in args {
        let t = checker.infer(&a.value);
        if !t.is_int() && !matches!(t, Ty::Unknown) {
            checker.error(
                ErrorCode::ZAP4024,
                format!(
                    "math.{method}() expects integer argument(s), found {}",
                    t.display()
                ),
                expr_span(&a.value),
            );
        }
    }
}

/// Infer `args[0]` as a numeric `list[T]` and return the reduction's element
/// type: `int` for an integer list, `float64` for a `float64` list. A non-list
/// or non-numeric-element argument is reported and `float64` assumed so later
/// checking proceeds. Used by the element-typed reductions `prod`/`sumprod`.
fn reduction_elem_ty(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Ty {
    // Type-check every argument (prod takes one list, sumprod two).
    let mut elem_out = Ty::Float64;
    for (i, a) in args.iter().enumerate() {
        let t = checker.infer(&a.value);
        let elem = match &t {
            Ty::List(elem) => (**elem).clone(),
            Ty::Unknown => continue,
            _ => {
                checker.error(
                    ErrorCode::ZAP4024,
                    format!(
                        "math.{method}() expects a list argument, found {}",
                        t.display()
                    ),
                    expr_span(&a.value),
                );
                continue;
            }
        };
        let resolved = if elem.is_int() {
            Ty::Int64
        } else if matches!(elem, Ty::Float64 | Ty::UntypedFloat) {
            Ty::Float64
        } else {
            checker.error(
                ErrorCode::ZAP4024,
                format!(
                    "math.{method}() needs a list of int or float64, found list[{}]",
                    elem.display()
                ),
                expr_span(&a.value),
            );
            Ty::Float64
        };
        if i == 0 {
            elem_out = resolved;
        } else if resolved != elem_out && !matches!(resolved, Ty::Unknown) {
            checker.error(
                ErrorCode::ZAP4024,
                format!(
                    "math.{method}() needs both lists to hold the same element type, \
                     found {} and {}",
                    elem_out.display(),
                    resolved.display()
                ),
                span,
            );
        }
    }
    elem_out
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // One numeric argument. For a scalar -> float64; for a numeric `array`
        // (#505) `math.<fn>` maps element-wise and yields an **array**, which
        // sema types loosely (`Unknown`) so downstream `sum`/indexing on the
        // result stay in the loose array path instead of being mis-typed scalar.
        "sqrt" | "exp" | "log" | "log2" | "log10" | "sin" | "cos" | "tan" | "asin" | "acos"
        | "atan" | "floor" | "ceil" | "round" | "trunc" | "fabs" => {
            expect_argc(checker, "math", method, args, 1, span);
            let arg_is_array = matches!(
                args.first().map(|a| checker.infer(&a.value)),
                Some(Ty::Unknown)
            );
            Some(if arg_is_array {
                Ty::Unknown
            } else {
                Ty::Float64
            })
        }
        // #973 unary scalar `f64 -> f64`: angles, hyperbolics, accurate
        // variants, and the special functions. Scalar-only (no array mapping is
        // wired, so lowering rejects an array argument cleanly).
        "degrees" | "radians" | "sinh" | "cosh" | "tanh" | "asinh" | "acosh" | "atanh"
        | "expm1" | "log1p" | "exp2" | "cbrt" | "gamma" | "lgamma" | "erf" | "erfc" => {
            expect_argc(checker, "math", method, args, 1, span);
            Some(Ty::Float64)
        }
        // Two float arguments -> float64.
        "pow" | "atan2" | "hypot" | "fmod" | "copysign" | "nextafter" | "remainder" => {
            expect_argc(checker, "math", method, args, 2, span);
            Some(Ty::Float64)
        }
        // `ldexp(x, e)` -> float64: x widens to f64, e is an integer exponent.
        "ldexp" => {
            expect_argc(checker, "math", method, args, 2, span);
            Some(Ty::Float64)
        }
        // `fma(x, y, z)` — the explicit fused multiply-add (#946, D-0196): one
        // rounding of `x*y + z`, portable and bit-identical across targets
        // (hardware FMA where present, correctly-rounded libm `fma` otherwise).
        // Three float arguments -> float64.
        "fma" => {
            expect_argc(checker, "math", method, args, 3, span);
            Some(Ty::Float64)
        }
        // One float argument -> bool.
        "isnan" | "isinf" | "isfinite" => {
            expect_argc(checker, "math", method, args, 1, span);
            Some(Ty::Bool)
        }
        // `isclose(a, b, rel_tol=1e-9, abs_tol=0.0)` -> bool. The two tolerances
        // are optional keyword arguments (defaults wired via `seam_sigs`).
        "isclose" => {
            expect_argc_range(checker, "math", method, args, 2, 4, span);
            Some(Ty::Bool)
        }
        // Integer math -> int. A non-integer argument is rejected.
        "gcd" | "lcm" | "comb" | "perm" => {
            expect_argc(checker, "math", method, args, 2, span);
            expect_int_args(checker, method, args);
            Some(Ty::Int64)
        }
        "factorial" | "isqrt" => {
            expect_argc(checker, "math", method, args, 1, span);
            expect_int_args(checker, method, args);
            Some(Ty::Int64)
        }
        // `modf(x)` -> (frac, int_part); `frexp(x)` -> (mantissa, exponent).
        "modf" => {
            expect_argc(checker, "math", method, args, 1, span);
            Some(Ty::Tuple(vec![Ty::Float64, Ty::Float64]))
        }
        "frexp" => {
            expect_argc(checker, "math", method, args, 1, span);
            Some(Ty::Tuple(vec![Ty::Float64, Ty::Int64]))
        }
        // Element-typed reductions: `int` list -> int, `float64` list -> float64.
        "prod" => {
            expect_argc(checker, "math", method, args, 1, span);
            Some(reduction_elem_ty(checker, method, args, span))
        }
        "sumprod" => {
            expect_argc(checker, "math", method, args, 2, span);
            Some(reduction_elem_ty(checker, method, args, span))
        }
        // Float reductions: a `float64` list (`fsum`) / two of them (`dist`).
        "fsum" => {
            expect_argc(checker, "math", method, args, 1, span);
            check_float_list_args(checker, method, args);
            Some(Ty::Float64)
        }
        "dist" => {
            expect_argc(checker, "math", method, args, 2, span);
            check_float_list_args(checker, method, args);
            Some(Ty::Float64)
        }
        _ => None,
    }
}

/// Infer each argument and require it to be a `list[float64]` (`fsum`/`dist`).
/// `Unknown` stays permissive; an integer-element list is rejected with a hint
/// to use `prod`/`sumprod` or convert the elements.
fn check_float_list_args(checker: &mut Checker, method: &str, args: &[Arg]) {
    for a in args {
        let t = checker.infer(&a.value);
        match &t {
            Ty::List(elem) if matches!(**elem, Ty::Float64 | Ty::UntypedFloat) => {}
            Ty::Unknown => {}
            Ty::List(elem) => checker.error(
                ErrorCode::ZAP4024,
                format!(
                    "math.{method}() needs a list of float64, found list[{}]",
                    elem.display()
                ),
                expr_span(&a.value),
            ),
            _ => checker.error(
                ErrorCode::ZAP4024,
                format!(
                    "math.{method}() expects a list of float64, found {}",
                    t.display()
                ),
                expr_span(&a.value),
            ),
        }
    }
}
