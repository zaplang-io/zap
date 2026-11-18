//! `math` module — sema shape/arity/type checks for the #973 scalar additions
//! (D-0234).
//!
//! Exhaustive type-checking for the new surface: the integer-math group returns
//! `int` (and rejects a float argument); the angle / hyperbolic / accurate /
//! special functions and the compose forms return `float64`; `isfinite` and
//! `isclose` return `bool`; `modf`/`frexp` return the right tuple shapes; and
//! the reductions are element-typed (`prod`/`sumprod`) or float (`fsum`/`dist`).
//! Also the argument type/arity diagnostics. Host-only (parse + sema), no LLVM.

use zapc_sema::check_str;

#[track_caller]
fn sema_errors(src: &str) -> Vec<(String, String)> {
    let (terrs, perrs, lerrs) = check_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    terrs
        .into_iter()
        .map(|e| (e.code.to_string(), e.message))
        .collect()
}

#[track_caller]
fn assert_ok(src: &str) {
    let errs = sema_errors(src);
    assert!(errs.is_empty(), "expected no sema errors, got: {errs:?}");
}

#[track_caller]
fn assert_code(src: &str, code: &str) {
    let errs = sema_errors(src);
    assert!(
        errs.iter().any(|(c, _)| c == code),
        "expected a {code} error, got: {errs:?}",
    );
}

/// Assert the program produces at least one sema error (code-agnostic), used for
/// the return-type negatives where a float result is assigned to an `int` slot.
#[track_caller]
fn assert_any_err(src: &str) {
    let errs = sema_errors(src);
    assert!(!errs.is_empty(), "expected a sema error, got none");
}

fn prog(body: &str) -> String {
    format!("def f():\n{body}\n")
}

// ---------------------------------------------------------------------------
// Integer math -> int. An annotated `int` local forces the return-type check.
// ---------------------------------------------------------------------------

#[test]
fn integer_math_returns_int() {
    assert_ok(&prog("    int g = math.gcd(12, 8)\n"));
    assert_ok(&prog("    int l = math.lcm(4, 6)\n"));
    assert_ok(&prog("    int fa = math.factorial(6)\n"));
    assert_ok(&prog("    int c = math.comb(5, 2)\n"));
    assert_ok(&prog("    int p = math.perm(5, 2)\n"));
    assert_ok(&prog("    int s = math.isqrt(17)\n"));
}

#[test]
fn integer_math_rejects_float_argument() {
    assert_code(&prog("    auto g = math.gcd(12.0, 8.0)\n"), "ZAP4024");
    assert_code(&prog("    auto fa = math.factorial(6.5)\n"), "ZAP4024");
    assert_code(&prog("    auto c = math.comb(5.0, 2)\n"), "ZAP4024");
    assert_code(&prog("    auto s = math.isqrt(17.0)\n"), "ZAP4024");
}

#[test]
fn integer_math_arity() {
    assert_code(&prog("    auto g = math.gcd(12)\n"), "ZAP4025");
    assert_code(&prog("    auto l = math.lcm(1, 2, 3)\n"), "ZAP4025");
    assert_code(&prog("    auto fa = math.factorial(1, 2)\n"), "ZAP4025");
    assert_code(&prog("    auto c = math.comb(5)\n"), "ZAP4025");
}

// ---------------------------------------------------------------------------
// Unary / binary float additions -> float64.
// ---------------------------------------------------------------------------

#[test]
fn unary_float_additions_return_float() {
    for f in [
        "degrees", "radians", "sinh", "cosh", "tanh", "asinh", "acosh", "atanh", "expm1", "log1p",
        "exp2", "cbrt", "gamma", "lgamma", "erf", "erfc",
    ] {
        assert_ok(&prog(&format!("    float v = math.{f}(1.0)\n")));
        assert_code(
            &prog(&format!("    auto v = math.{f}(1.0, 2.0)\n")),
            "ZAP4025",
        );
    }
}

#[test]
fn binary_float_additions_return_float() {
    assert_ok(&prog("    float v = math.copysign(1.0, -2.0)\n"));
    assert_ok(&prog("    float v = math.nextafter(1.0, 2.0)\n"));
    assert_ok(&prog("    float v = math.remainder(7.0, 3.0)\n"));
    assert_ok(&prog("    float v = math.ldexp(1.5, 3)\n"));
    assert_code(&prog("    auto v = math.copysign(1.0)\n"), "ZAP4025");
    assert_code(&prog("    auto v = math.ldexp(1.5)\n"), "ZAP4025");
}

// An integer argument widens to the float slot (as for `sqrt(16)`), so a mixed
// int/float call still type-checks.
#[test]
fn float_additions_accept_int_argument() {
    assert_ok(&prog("    float v = math.cbrt(27)\n"));
    assert_ok(&prog("    float v = math.ldexp(2, 3)\n"));
}

// ---------------------------------------------------------------------------
// Classification / compare -> bool.
// ---------------------------------------------------------------------------

#[test]
fn isfinite_returns_bool() {
    assert_ok(&prog("    bool b = math.isfinite(1.0)\n"));
    assert_code(&prog("    auto b = math.isfinite(1.0, 2.0)\n"), "ZAP4025");
}

#[test]
fn isclose_returns_bool_and_arity_range() {
    assert_ok(&prog("    bool b = math.isclose(1.0, 1.0)\n"));
    assert_ok(&prog("    bool b = math.isclose(1.0, 1.1, rel_tol=0.2)\n"));
    assert_ok(&prog("    bool b = math.isclose(1.0, 1.1, 0.2, 0.0)\n"));
    // Below the minimum (2) and above the maximum (4) argument count.
    assert_code(&prog("    auto b = math.isclose(1.0)\n"), "ZAP4025");
    assert_code(
        &prog("    auto b = math.isclose(1.0, 1.0, 0.1, 0.2, 0.3)\n"),
        "ZAP4025",
    );
}

// ---------------------------------------------------------------------------
// modf -> (float, float); frexp -> (float, int). Unpack + annotate each slot.
// ---------------------------------------------------------------------------

#[test]
fn modf_returns_float_pair() {
    assert_ok(&prog(
        "    auto frac, ip = math.modf(3.5)\n    float a = frac\n    float b = ip\n",
    ));
}

#[test]
fn frexp_returns_float_int_pair() {
    assert_ok(&prog(
        "    auto m, e = math.frexp(8.0)\n    float a = m\n    int b = e\n",
    ));
    // The exponent is an int, not a float: assigning it to a float widens (ok),
    // but the mantissa assigned to an int must fail.
    assert_any_err(&prog("    auto m, e = math.frexp(8.0)\n    int bad = m\n"));
}

// ---------------------------------------------------------------------------
// Reductions. prod/sumprod are element-typed; fsum/dist are float.
// ---------------------------------------------------------------------------

#[test]
fn prod_is_element_typed() {
    assert_ok(&prog(
        "    auto xs = [1, 2, 3]\n    int p = math.prod(xs)\n",
    ));
    assert_ok(&prog(
        "    auto xs = [1.0, 2.0, 3.0]\n    float p = math.prod(xs)\n",
    ));
}

#[test]
fn sumprod_is_element_typed() {
    assert_ok(&prog(
        "    auto xs = [1, 2, 3]\n    auto ys = [4, 5, 6]\n    int s = math.sumprod(xs, ys)\n",
    ));
    assert_ok(&prog(
        "    auto xs = [1.0, 2.0]\n    auto ys = [3.0, 4.0]\n    float s = math.sumprod(xs, ys)\n",
    ));
}

#[test]
fn fsum_and_dist_return_float() {
    assert_ok(&prog(
        "    auto xs = [0.1, 0.2, 0.3]\n    float s = math.fsum(xs)\n",
    ));
    assert_ok(&prog(
        "    auto p = [1.0, 2.0]\n    auto q = [4.0, 6.0]\n    float d = math.dist(p, q)\n",
    ));
}

/// The golden fixture must at least parse + sema-check cleanly on the host (the
/// runtime values are verified by the golden runner on CI).
#[test]
fn golden_fixture_sema_checks() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/golden/math_scalar_fns.zap"
    );
    let src = std::fs::read_to_string(path).expect("golden fixture should exist");
    let errs = sema_errors(&src);
    assert!(errs.is_empty(), "golden fixture has sema errors: {errs:?}");
}

#[test]
fn reductions_reject_bad_arguments() {
    // prod needs a list.
    assert_code(&prog("    auto p = math.prod(5)\n"), "ZAP4024");
    // fsum needs a float list, not an int list (use prod for int products).
    assert_code(
        &prog("    auto xs = [1, 2, 3]\n    auto s = math.fsum(xs)\n"),
        "ZAP4024",
    );
    // Arity.
    assert_code(
        &prog("    auto xs = [1.0]\n    auto d = math.dist(xs)\n"),
        "ZAP4025",
    );
    assert_code(
        &prog("    auto xs = [1.0]\n    auto s = math.prod(xs, xs)\n"),
        "ZAP4025",
    );
}
