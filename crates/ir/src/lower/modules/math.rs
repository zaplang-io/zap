//! `math` module — lowering facet (ticket #98, D-0056).
//!
//! Each function widens its argument(s) to `f64` and emits a single call to the
//! backing libm wrapper (`zap_math_*`, except `pow`, which reuses the existing
//! `zap_powf`). `isnan`/`isinf` return an `i64` 0/1 from C, compared against zero
//! to yield the `i1` boolean sema assigned. Constants (`math.pi`, …) are handled
//! by [`const_lower`] via the const sub-registry in `super`.

use zapc_syntax::ast;

use super::support::call_ret;
use super::{ModuleConst, ModuleLower};
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "math",
    lower,
};

pub(super) const MODULE_CONST: ModuleConst = ModuleConst {
    name: "math",
    lower: const_lower,
};

/// Lower a `math.<const>` attribute access to an `f64` constant (ticket #98).
fn const_lower(lowerer: &mut FnLower, name: &str) -> Option<(Value, Ty)> {
    let v = match name {
        "pi" => std::f64::consts::PI,
        "e" => std::f64::consts::E,
        "tau" => std::f64::consts::TAU,
        "inf" => f64::INFINITY,
        "nan" => f64::NAN,
        _ => return None,
    };
    Some((lowerer.fconst(Ty::F64, v), Ty::F64))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // Unary libm wrappers. The second element is the element-wise array op
    // (#505): when the argument is a numeric `array`, `math.<fn>(a)` maps the
    // function over it (a new result array); `None` means array mapping is not
    // wired for that function yet (a scalar-only libm wrapper).
    let (func, array_op): (&str, Option<ArrayMathOp>) = match method {
        "sqrt" => ("zap_math_sqrt", Some(ArrayMathOp::Sqrt)),
        "exp" => ("zap_math_exp", Some(ArrayMathOp::Exp)),
        "log" => ("zap_math_log", Some(ArrayMathOp::Log)),
        "log2" => ("zap_math_log2", None),
        "log10" => ("zap_math_log10", None),
        "sin" => ("zap_math_sin", Some(ArrayMathOp::Sin)),
        "cos" => ("zap_math_cos", Some(ArrayMathOp::Cos)),
        "tan" => ("zap_math_tan", Some(ArrayMathOp::Tan)),
        "asin" => ("zap_math_asin", None),
        "acos" => ("zap_math_acos", None),
        "atan" => ("zap_math_atan", None),
        "floor" => ("zap_math_floor", None),
        "ceil" => ("zap_math_ceil", None),
        "round" => ("zap_math_round", None),
        "trunc" => ("zap_math_trunc", None),
        "fabs" => ("zap_math_fabs", Some(ArrayMathOp::Fabs)),
        // #973 unary scalar `f64 -> f64` (no array mapping wired yet).
        "degrees" => ("zap_math_degrees", None),
        "radians" => ("zap_math_radians", None),
        "sinh" => ("zap_math_sinh", None),
        "cosh" => ("zap_math_cosh", None),
        "tanh" => ("zap_math_tanh", None),
        "asinh" => ("zap_math_asinh", None),
        "acosh" => ("zap_math_acosh", None),
        "atanh" => ("zap_math_atanh", None),
        "expm1" => ("zap_math_expm1", None),
        "log1p" => ("zap_math_log1p", None),
        "exp2" => ("zap_math_exp2", None),
        "cbrt" => ("zap_math_cbrt", None),
        "gamma" => ("zap_math_gamma", None),
        "lgamma" => ("zap_math_lgamma", None),
        "erf" => ("zap_math_erf", None),
        "erfc" => ("zap_math_erfc", None),
        // `pow` reuses the `**`-operator libm wrapper (ticket #19).
        "pow" => return Ok(Some(binary(lowerer, "zap_powf", args)?)),
        "atan2" => return Ok(Some(binary(lowerer, "zap_math_atan2", args)?)),
        "hypot" => return Ok(Some(binary(lowerer, "zap_math_hypot", args)?)),
        "fmod" => return Ok(Some(binary(lowerer, "zap_math_fmod", args)?)),
        // #973 binary / compose forms.
        "copysign" => return Ok(Some(binary(lowerer, "zap_math_copysign", args)?)),
        "nextafter" => return Ok(Some(binary(lowerer, "zap_math_nextafter", args)?)),
        "remainder" => return Ok(Some(binary(lowerer, "zap_math_remainder", args)?)),
        "ldexp" => return Ok(Some(ldexp(lowerer, args)?)),
        "isfinite" => return Ok(Some(predicate(lowerer, "zap_math_isfinite", args)?)),
        "isclose" => return Ok(Some(isclose(lowerer, args)?)),
        // Integer math. `gcd` is total; the rest raise (domain / overflow) and so
        // route through `throwing_call` (the `true` flag) to be catchable.
        "gcd" => return Ok(Some(int_binary(lowerer, "zap_math_gcd", args, false)?)),
        "lcm" => return Ok(Some(int_binary(lowerer, "zap_math_lcm", args, true)?)),
        "comb" => return Ok(Some(int_binary(lowerer, "zap_math_comb", args, true)?)),
        "perm" => return Ok(Some(int_binary(lowerer, "zap_math_perm", args, true)?)),
        "factorial" => return Ok(Some(int_unary(lowerer, "zap_math_factorial", args, true)?)),
        "isqrt" => return Ok(Some(int_unary(lowerer, "zap_math_isqrt", args, true)?)),
        // Tuple-returning decompositions.
        "modf" => return Ok(Some(modf(lowerer, args)?)),
        "frexp" => return Ok(Some(frexp(lowerer, args)?)),
        // List reductions.
        "prod" => return Ok(Some(reduce_one(lowerer, "prod", args)?)),
        // `fsum` only raises on OOM (a catchable RuntimeError), so it still
        // routes through `throwing_call` to stay catchable (D-0234 / the
        // "raising runtime calls need throwing_call" rule).
        "fsum" => {
            return Ok(Some(reduce_list(
                lowerer,
                "zap_math_fsum",
                args,
                Ty::F64,
                true,
            )?))
        }
        "dist" => return Ok(Some(reduce_two(lowerer, "dist", args)?)),
        "sumprod" => return Ok(Some(reduce_two(lowerer, "sumprod", args)?)),
        // `fma(x, y, z)` — explicit fused multiply-add (#946, D-0196): a single
        // rounding of `x*y + z`, independent of the implicit-`x*y+z` contraction
        // policy. Lowers to a libm `fma` wrapper (correctly rounded, hardware FMA
        // where present) so the bits are identical on every target.
        "fma" => return Ok(Some(ternary(lowerer, "zap_math_fma", args)?)),
        "isnan" => return Ok(Some(predicate(lowerer, "zap_math_isnan", args)?)),
        "isinf" => return Ok(Some(predicate(lowerer, "zap_math_isinf", args)?)),
        _ => return Ok(None),
    };
    Ok(Some(unary_or_array(lowerer, method, func, array_op, args)?))
}

/// `math.<fn>(x)`: a scalar `func(f64) -> f64`, or — when `x` is a numeric
/// `array` — an element-wise map over it (#505, D-0168), yielding a new float
/// array (`f32` for an `f32` source, else `f64`). A function with no array op
/// wired yet is rejected cleanly on an array argument.
fn unary_or_array(
    lowerer: &mut FnLower,
    method: &str,
    func: &str,
    array_op: Option<ArrayMathOp>,
    args: &[ast::Arg],
) -> Result<(Value, Ty), LowerError> {
    let arg = args
        .first()
        .ok_or_else(|| err(format!("math.{method} missing its argument")))?;
    let (v, t) = lowerer.expr(&arg.value)?;
    if let Ty::Array(dt, ndim) = t {
        let op = array_op.ok_or_else(|| {
            err(format!(
                "math.{method} is not supported on arrays yet (try sqrt/exp/log/sin/cos/tan/fabs)"
            ))
        })?;
        // Float-domain result: keep an f32 source at f32, otherwise f64.
        let out = if matches!(dt, Dtype::F32) {
            Dtype::F32
        } else {
            Dtype::F64
        };
        let dest = lowerer.new_value(Ty::Array(out, ndim));
        lowerer.emit(Inst::ArrayUnaryOp {
            dest,
            op,
            out,
            src: v,
            src_dtype: dt,
        });
        Ok((dest, Ty::Array(out, ndim)))
    } else {
        let x = lowerer.coerce(v, t, Ty::F64);
        let d = lowerer.new_value(Ty::F64);
        lowerer.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args: vec![x],
        });
        Ok((d, Ty::F64))
    }
}

/// `func(f64, f64) -> f64`.
fn binary(lowerer: &mut FnLower, func: &str, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
    let a = f64_arg(lowerer, func, args, 0)?;
    let b = f64_arg(lowerer, func, args, 1)?;
    let d = lowerer.new_value(Ty::F64);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: func.to_string(),
        args: vec![a, b],
    });
    Ok((d, Ty::F64))
}

/// `func(f64, f64, f64) -> f64` (the explicit `fma`, #946).
fn ternary(
    lowerer: &mut FnLower,
    func: &str,
    args: &[ast::Arg],
) -> Result<(Value, Ty), LowerError> {
    let a = f64_arg(lowerer, func, args, 0)?;
    let b = f64_arg(lowerer, func, args, 1)?;
    let c = f64_arg(lowerer, func, args, 2)?;
    let d = lowerer.new_value(Ty::F64);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: func.to_string(),
        args: vec![a, b, c],
    });
    Ok((d, Ty::F64))
}

/// `func(f64) -> i64` (0/1), reduced to an `i1` boolean.
fn predicate(
    lowerer: &mut FnLower,
    func: &str,
    args: &[ast::Arg],
) -> Result<(Value, Ty), LowerError> {
    let x = f64_arg(lowerer, func, args, 0)?;
    let r = lowerer.new_value(Ty::I64);
    lowerer.emit(Inst::Call {
        dest: Some(r),
        func: func.to_string(),
        args: vec![x],
    });
    let z = lowerer.iconst(Ty::I64, 0);
    let d = lowerer.new_value(Ty::I1);
    lowerer.emit(Inst::ICmp {
        dest: d,
        pred: CmpPred::Ne,
        lhs: r,
        rhs: z,
    });
    Ok((d, Ty::I1))
}

/// Lower argument `i` and widen it to `f64` (sema has already checked arity).
fn f64_arg(
    lowerer: &mut FnLower,
    func: &str,
    args: &[ast::Arg],
    i: usize,
) -> Result<Value, LowerError> {
    let arg = args
        .get(i)
        .ok_or_else(|| err(format!("math.{func} missing argument {i}")))?;
    let (v, t) = lowerer.expr(&arg.value)?;
    Ok(lowerer.coerce(v, t, Ty::F64))
}

/// Lower argument `i` and narrow/widen it to `i64` (the integer-math group).
fn i64_arg(
    lowerer: &mut FnLower,
    func: &str,
    args: &[ast::Arg],
    i: usize,
) -> Result<Value, LowerError> {
    let arg = args
        .get(i)
        .ok_or_else(|| err(format!("math.{func} missing argument {i}")))?;
    let (v, t) = lowerer.expr(&arg.value)?;
    Ok(lowerer.coerce(v, t, Ty::I64))
}

/// `ldexp(x, e)` -> `f64`: x widens to `f64`, e to `i64` (clamped to the C `int`
/// range inside the runtime wrapper).
fn ldexp(lowerer: &mut FnLower, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
    let x = f64_arg(lowerer, "ldexp", args, 0)?;
    let e = i64_arg(lowerer, "ldexp", args, 1)?;
    Ok(call_ret(
        lowerer,
        "zap_math_ldexp",
        vec![x, e],
        Ty::F64,
        false,
    ))
}

/// `isclose(a, b, rel_tol=1e-9, abs_tol=0.0)` -> bool. The tolerances default
/// when omitted (seam defaults fill an interior hole; a trailing omit is filled
/// here). A negative tolerance raises `ValueError`, so the runtime call is a
/// `throwing_call`; its `i64` 0/1 result reduces to the `i1` sema assigned.
fn isclose(lowerer: &mut FnLower, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
    let a = f64_arg(lowerer, "isclose", args, 0)?;
    let b = f64_arg(lowerer, "isclose", args, 1)?;
    let rel_tol = match args.get(2) {
        Some(_) => f64_arg(lowerer, "isclose", args, 2)?,
        None => lowerer.fconst(Ty::F64, 1e-9),
    };
    let abs_tol = match args.get(3) {
        Some(_) => f64_arg(lowerer, "isclose", args, 3)?,
        None => lowerer.fconst(Ty::F64, 0.0),
    };
    let word = lowerer.new_value(Ty::I64);
    lowerer.throwing_call(Some(word), "zap_math_isclose", vec![a, b, rel_tol, abs_tol]);
    let zero = lowerer.iconst(Ty::I64, 0);
    let d = lowerer.new_value(Ty::I1);
    lowerer.emit(Inst::ICmp {
        dest: d,
        pred: CmpPred::Ne,
        lhs: word,
        rhs: zero,
    });
    Ok((d, Ty::I1))
}

/// `func(i64, i64) -> i64` (gcd/lcm/comb/perm). `throwing` is set for the forms
/// that raise on a domain violation or an overflowing result.
fn int_binary(
    lowerer: &mut FnLower,
    func: &str,
    args: &[ast::Arg],
    throwing: bool,
) -> Result<(Value, Ty), LowerError> {
    let a = i64_arg(lowerer, func, args, 0)?;
    let b = i64_arg(lowerer, func, args, 1)?;
    Ok(call_ret(lowerer, func, vec![a, b], Ty::I64, throwing))
}

/// `func(i64) -> i64` (factorial/isqrt); both raise, so `throwing` is `true`.
fn int_unary(
    lowerer: &mut FnLower,
    func: &str,
    args: &[ast::Arg],
    throwing: bool,
) -> Result<(Value, Ty), LowerError> {
    let a = i64_arg(lowerer, func, args, 0)?;
    Ok(call_ret(lowerer, func, vec![a], Ty::I64, throwing))
}

/// `modf(x)` -> `(frac, int_part)`: two `f64` component calls assembled into a
/// `(float64, float64)` tuple (the `os.path_split` build pattern).
fn modf(lowerer: &mut FnLower, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
    let x = f64_arg(lowerer, "modf", args, 0)?;
    let frac = lowerer.new_value(Ty::F64);
    lowerer.emit(Inst::Call {
        dest: Some(frac),
        func: "zap_math_modf_frac".to_string(),
        args: vec![x],
    });
    let ipart = lowerer.new_value(Ty::F64);
    lowerer.emit(Inst::Call {
        dest: Some(ipart),
        func: "zap_math_modf_ipart".to_string(),
        args: vec![x],
    });
    Ok(build_pair(lowerer, frac, Ty::F64, ipart, Ty::F64))
}

/// `frexp(x)` -> `(mantissa, exponent)` as a `(float64, int)` tuple.
fn frexp(lowerer: &mut FnLower, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
    let x = f64_arg(lowerer, "frexp", args, 0)?;
    let mant = lowerer.new_value(Ty::F64);
    lowerer.emit(Inst::Call {
        dest: Some(mant),
        func: "zap_math_frexp_mant".to_string(),
        args: vec![x],
    });
    let exp = lowerer.new_value(Ty::I64);
    lowerer.emit(Inst::Call {
        dest: Some(exp),
        func: "zap_math_frexp_exp".to_string(),
        args: vec![x],
    });
    Ok(build_pair(lowerer, mant, Ty::F64, exp, Ty::I64))
}

/// Assemble a two-field tuple value `(a: at, b: bt)` (the interned-tuple struct
/// shape the sema `Ty::Tuple` lowers to).
fn build_pair(lowerer: &mut FnLower, a: Value, at: Ty, b: Value, bt: Ty) -> (Value, Ty) {
    let sid = lowerer.ctx.tuples.borrow_mut().intern(vec![at, bt]);
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 0,
        val: a,
    });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 1,
        val: b,
    });
    (dest, Ty::Struct(sid))
}

/// Map a numeric list element kind to its runtime suffix + result type
/// (`int` -> `_i64`, `float64` -> `_f64`). Used by `prod`/`sumprod`. Integer
/// widths / `bool` / `char` / enum share the `i64` word, so they fold to `i64`.
fn list_elem_suffix(method: &str, elem: Elem) -> Result<(&'static str, Ty), LowerError> {
    match elem {
        Elem::I64 | Elem::Bool | Elem::Char | Elem::Enum(_) => Ok(("i64", Ty::I64)),
        Elem::F64 => Ok(("f64", Ty::F64)),
        _ => Err(err(format!(
            "math.{method}() needs a list of int or float64"
        ))),
    }
}

/// `prod(xs)` -> element-typed scalar: dispatch on the element kind of the list
/// argument and call the matching `zap_math_prod_{i64,f64}` (never raises).
fn reduce_one(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<(Value, Ty), LowerError> {
    let arg = args
        .first()
        .ok_or_else(|| err(format!("math.{method} missing its list argument")))?;
    let (v, t) = lowerer.expr(&arg.value)?;
    let Ty::List(elem) = t else {
        return Err(err(format!(
            "math.{method}() expects a list argument, found {t:?}"
        )));
    };
    let (suffix, ret) = list_elem_suffix(method, elem)?;
    Ok(call_ret(
        lowerer,
        &format!("zap_math_{method}_{suffix}"),
        vec![v],
        ret,
        false,
    ))
}

/// A single-list reduction over a `float64` list (`fsum`): the list handle is a
/// `ptr`, the result `ret`.
fn reduce_list(
    lowerer: &mut FnLower,
    func: &str,
    args: &[ast::Arg],
    ret: Ty,
    throwing: bool,
) -> Result<(Value, Ty), LowerError> {
    let arg = args
        .first()
        .ok_or_else(|| err(format!("{func} missing its list argument")))?;
    let (v, _t) = lowerer.expr(&arg.value)?;
    Ok(call_ret(lowerer, func, vec![v], ret, throwing))
}

/// Two-list reductions. `dist(p, q)` -> `float64` (both lists are `float64`);
/// `sumprod(p, q)` is element-typed from the first list. Both raise on a length
/// mismatch, so they route through `throwing_call`.
fn reduce_two(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<(Value, Ty), LowerError> {
    let (p, pt) = lowerer.expr(&args[0].value)?;
    let (q, _qt) = lowerer.expr(&args[1].value)?;
    if method == "dist" {
        return Ok(call_ret(
            lowerer,
            "zap_math_dist",
            vec![p, q],
            Ty::F64,
            true,
        ));
    }
    // sumprod — element-typed from the first list's element kind.
    let Ty::List(elem) = pt else {
        return Err(err(format!(
            "math.{method}() expects list arguments, found {pt:?}"
        )));
    };
    let (suffix, ret) = list_elem_suffix(method, elem)?;
    Ok(call_ret(
        lowerer,
        &format!("zap_math_sumprod_{suffix}"),
        vec![p, q],
        ret,
        true,
    ))
}
