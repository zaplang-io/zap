//! `math` module — lowering facet (ticket #98, D-0056).
//!
//! Each function widens its argument(s) to `f64` and emits a single call to the
//! backing libm wrapper (`zap_math_*`, except `pow`, which reuses the existing
//! `zap_powf`). `isnan`/`isinf` return an `i64` 0/1 from C, compared against zero
//! to yield the `i1` boolean sema assigned. Constants (`math.pi`, …) are handled
//! by [`const_lower`] via the const sub-registry in `super`.

use zapc_syntax::ast;

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
    let func = match method {
        "sqrt" => "zap_math_sqrt",
        "exp" => "zap_math_exp",
        "log" => "zap_math_log",
        "log2" => "zap_math_log2",
        "log10" => "zap_math_log10",
        "sin" => "zap_math_sin",
        "cos" => "zap_math_cos",
        "tan" => "zap_math_tan",
        "asin" => "zap_math_asin",
        "acos" => "zap_math_acos",
        "atan" => "zap_math_atan",
        "floor" => "zap_math_floor",
        "ceil" => "zap_math_ceil",
        "round" => "zap_math_round",
        "trunc" => "zap_math_trunc",
        "fabs" => "zap_math_fabs",
        // `pow` reuses the `**`-operator libm wrapper (ticket #19).
        "pow" => return Ok(Some(binary(lowerer, "zap_powf", args)?)),
        "atan2" => return Ok(Some(binary(lowerer, "zap_math_atan2", args)?)),
        "hypot" => return Ok(Some(binary(lowerer, "zap_math_hypot", args)?)),
        "fmod" => return Ok(Some(binary(lowerer, "zap_math_fmod", args)?)),
        "isnan" => return Ok(Some(predicate(lowerer, "zap_math_isnan", args)?)),
        "isinf" => return Ok(Some(predicate(lowerer, "zap_math_isinf", args)?)),
        _ => return Ok(None),
    };
    Ok(Some(unary(lowerer, func, args)?))
}

/// `func(f64) -> f64`.
fn unary(lowerer: &mut FnLower, func: &str, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
    let x = f64_arg(lowerer, func, args, 0)?;
    let d = lowerer.new_value(Ty::F64);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: func.to_string(),
        args: vec![x],
    });
    Ok((d, Ty::F64))
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
