//! `math` module — lowering (ZIR) checks for the #973 scalar additions (D-0234).
//!
//! Lowers a program exercising every new `math` function to ZIR on the host (no
//! LLVM) and asserts the expected backing runtime symbols are emitted — in
//! particular that the element-typed reductions (`prod`/`sumprod`) and the
//! tuple decompositions (`modf`/`frexp`) dispatch to the right symbols. The
//! end-to-end *values* are covered by the `math_scalar_fns` golden on CI.

use zapc_ir::{lower_module, Inst, Module};
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting no lex/parse/lower errors, returning the ZIR.
fn lower_ok(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    module
}

/// Every runtime symbol named by a `Call`/`Invoke` anywhere in the module.
fn called_symbols(m: &Module) -> Vec<String> {
    let mut out = Vec::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                match inst {
                    Inst::Call { func, .. } | Inst::Invoke { func, .. } => out.push(func.clone()),
                    _ => {}
                }
            }
        }
    }
    out
}

#[test]
fn all_new_math_functions_lower() {
    // One program touching every new surface. Integer math, angles, hyperbolics,
    // accurate variants, special functions, classify/compose, tuple decompose,
    // and element-typed + float reductions.
    let m = lower_ok(
        "def main():\n\
        \x20   auto g = math.gcd(12, 8)\n\
        \x20   auto l = math.lcm(4, 6)\n\
        \x20   auto fa = math.factorial(6)\n\
        \x20   auto c = math.comb(5, 2)\n\
        \x20   auto p = math.perm(5, 2)\n\
        \x20   auto s = math.isqrt(17)\n\
        \x20   auto d = math.degrees(3.0)\n\
        \x20   auto r = math.radians(180.0)\n\
        \x20   auto sh = math.sinh(1.0)\n\
        \x20   auto ah = math.acosh(2.0)\n\
        \x20   auto e1 = math.expm1(0.5)\n\
        \x20   auto cb = math.cbrt(27.0)\n\
        \x20   auto gm = math.gamma(5.0)\n\
        \x20   auto ef = math.erf(1.0)\n\
        \x20   auto cs = math.copysign(1.0, -2.0)\n\
        \x20   auto na = math.nextafter(1.0, 2.0)\n\
        \x20   auto rm = math.remainder(7.0, 3.0)\n\
        \x20   auto le = math.ldexp(1.5, 3)\n\
        \x20   auto isf = math.isfinite(1.0)\n\
        \x20   auto icd = math.isclose(1.0, 1.0)\n\
        \x20   auto ica = math.isclose(1.0, 1.0, abs_tol=0.1)\n\
        \x20   auto frac, ip = math.modf(3.5)\n\
        \x20   auto mant, ex = math.frexp(8.0)\n\
        \x20   auto xi = [1, 2, 3]\n\
        \x20   auto xf = [1.0, 2.0, 3.0]\n\
        \x20   auto pi = math.prod(xi)\n\
        \x20   auto pf = math.prod(xf)\n\
        \x20   auto fs = math.fsum(xf)\n\
        \x20   auto di = math.dist(xf, xf)\n\
        \x20   auto spi = math.sumprod(xi, xi)\n\
        \x20   auto spf = math.sumprod(xf, xf)\n",
    );
    let syms = called_symbols(&m);
    for want in [
        "zap_math_gcd",
        "zap_math_lcm",
        "zap_math_factorial",
        "zap_math_comb",
        "zap_math_perm",
        "zap_math_isqrt",
        "zap_math_degrees",
        "zap_math_radians",
        "zap_math_sinh",
        "zap_math_acosh",
        "zap_math_expm1",
        "zap_math_cbrt",
        "zap_math_gamma",
        "zap_math_erf",
        "zap_math_copysign",
        "zap_math_nextafter",
        "zap_math_remainder",
        "zap_math_ldexp",
        "zap_math_isfinite",
        "zap_math_isclose",
        "zap_math_modf_frac",
        "zap_math_modf_ipart",
        "zap_math_frexp_mant",
        "zap_math_frexp_exp",
        // Element-typed dispatch: both the i64 and f64 reduction symbols appear.
        "zap_math_prod_i64",
        "zap_math_prod_f64",
        "zap_math_fsum",
        "zap_math_dist",
        "zap_math_sumprod_i64",
        "zap_math_sumprod_f64",
    ] {
        assert!(
            syms.iter().any(|s| s == want),
            "expected a call to {want}, got: {syms:?}"
        );
    }
}

/// The golden fixture lowers to ZIR cleanly on the host (its `try`/`except`
/// raising paths exercise the `throwing_call` wiring of the integer-math and
/// reduction functions). Runtime values are checked by the golden runner on CI.
#[test]
fn golden_fixture_lowers() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/golden/math_scalar_fns.zap"
    );
    let src = std::fs::read_to_string(path).expect("golden fixture should exist");
    lower_ok(&src);
}

#[test]
fn isclose_fills_default_tolerances_without_error() {
    // No-kwarg, interior-hole (abs_tol only), and both-supplied forms all lower.
    lower_ok(
        "def main():\n\
        \x20   auto a = math.isclose(1.0, 1.0000001)\n\
        \x20   auto b = math.isclose(1.0, 1.1, abs_tol=0.2)\n\
        \x20   auto c = math.isclose(1.0, 1.1, rel_tol=0.2, abs_tol=0.0)\n",
    );
}
