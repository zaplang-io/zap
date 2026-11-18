//! `math` module — runtime-symbol declarations (ticket #98, D-0056).
//!
//! Thin libm wrappers: the unary/binary functions take and return `f64`;
//! `isnan`/`isinf` return an `i64` 0/1. `math.pow` reuses the existing
//! `zap_powf` (declared with the scalar numeric helpers in `lib.rs`), so it is
//! intentionally absent here. See the "math + random" section of `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let f = cg.ctx.f64_type();
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    for name in [
        "zap_math_sqrt",
        "zap_math_exp",
        "zap_math_log",
        "zap_math_log2",
        "zap_math_log10",
        "zap_math_sin",
        "zap_math_cos",
        "zap_math_tan",
        "zap_math_asin",
        "zap_math_acos",
        "zap_math_atan",
        "zap_math_floor",
        "zap_math_ceil",
        "zap_math_round",
        "zap_math_trunc",
        "zap_math_fabs",
        // #973 unary f64 -> f64: hyperbolics, angles, accurate variants, special
        // functions, and the modf/frexp component wrappers.
        "zap_math_degrees",
        "zap_math_radians",
        "zap_math_sinh",
        "zap_math_cosh",
        "zap_math_tanh",
        "zap_math_asinh",
        "zap_math_acosh",
        "zap_math_atanh",
        "zap_math_expm1",
        "zap_math_log1p",
        "zap_math_exp2",
        "zap_math_cbrt",
        "zap_math_gamma",
        "zap_math_lgamma",
        "zap_math_erf",
        "zap_math_erfc",
        "zap_math_modf_frac",
        "zap_math_modf_ipart",
        "zap_math_frexp_mant",
    ] {
        cg.declare_rt(name, &[f.into()], Some(f.into()));
    }
    for name in [
        "zap_math_atan2",
        "zap_math_hypot",
        "zap_math_fmod",
        // #973 binary f64 x f64 -> f64.
        "zap_math_copysign",
        "zap_math_nextafter",
        "zap_math_remainder",
    ] {
        cg.declare_rt(name, &[f.into(), f.into()], Some(f.into()));
    }
    // `fma(x, y, z)` — explicit fused multiply-add (#946, D-0196): 3 f64 -> f64.
    cg.declare_rt(
        "zap_math_fma",
        &[f.into(), f.into(), f.into()],
        Some(f.into()),
    );
    // #973 float -> i64 predicates / decompositions.
    for name in [
        "zap_math_isnan",
        "zap_math_isinf",
        "zap_math_isfinite",
        "zap_math_frexp_exp",
    ] {
        cg.declare_rt(name, &[f.into()], Some(i.into()));
    }
    // ldexp(x: f64, e: i64) -> f64.
    cg.declare_rt("zap_math_ldexp", &[f.into(), i.into()], Some(f.into()));
    // isclose(a, b, rel_tol, abs_tol) -> i64 (0/1); raises ValueError on a
    // negative tolerance.
    cg.declare_rt(
        "zap_math_isclose",
        &[f.into(), f.into(), f.into(), f.into()],
        Some(i.into()),
    );
    // #973 integer math (i64 words in, i64 out). gcd/lcm/comb/perm take two.
    for name in [
        "zap_math_gcd",
        "zap_math_lcm",
        "zap_math_comb",
        "zap_math_perm",
    ] {
        cg.declare_rt(name, &[i.into(), i.into()], Some(i.into()));
    }
    for name in ["zap_math_factorial", "zap_math_isqrt"] {
        cg.declare_rt(name, &[i.into()], Some(i.into()));
    }
    // #973 list reductions over a ZapList[T] handle (ptr). prod/fsum take one
    // list; dist/sumprod take two.
    cg.declare_rt("zap_math_prod_i64", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_math_prod_f64", &[p.into()], Some(f.into()));
    cg.declare_rt("zap_math_fsum", &[p.into()], Some(f.into()));
    cg.declare_rt("zap_math_dist", &[p.into(), p.into()], Some(f.into()));
    cg.declare_rt(
        "zap_math_sumprod_i64",
        &[p.into(), p.into()],
        Some(i.into()),
    );
    cg.declare_rt(
        "zap_math_sumprod_f64",
        &[p.into(), p.into()],
        Some(f.into()),
    );
}
