//! `random` module — lowering facet (ticket #98, D-0056; NumPy `Generator`
//! #930/#931, D-0189).
//!
//! Two coexisting surfaces (see the sema facet). The pre-existing xoshiro
//! generator lowers `seed`/`random`/`uniform`/`randint`/`choice`/`shuffle` to the
//! module-level `zap_random_*` calls. The NumPy-compatible PCG64 `Generator`
//! (D-0189): `default_rng(seed)` allocates an opaque handle (`zap_random_default_
//! rng*`); each draw lowers to a scalar `zap_random_gen_<dist>` call, or — when a
//! `size=` keyword is present — allocates a result array (int or shape-tuple) and
//! lowers to the matching `..._fill` call that writes the draws. `choice` fills an
//! i64 index array (scalar form reads element 0); `shuffle` permutes in place;
//! `permutation` returns a shuffled arange (int arg) or a shuffled copy (array).
//!
//! Dispatch: the NumPy-only method names always take the handle path; the four
//! names shared with the Python surface (`random`/`uniform`/`choice`/`shuffle`)
//! take it when their first argument lowers to a `Ptr` (the handle).

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "random",
    lower,
};

/// NumPy-only method names (always the `Generator` surface).
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

/// The positional (unnamed) argument expressions, in order.
fn positional(args: &[ast::Arg]) -> Vec<&ast::Expr> {
    args.iter()
        .filter(|a| a.name.is_none())
        .map(|a| &a.value)
        .collect()
}

/// A named keyword argument's expression, if present.
fn kw<'a>(args: &'a [ast::Arg], name: &str) -> Option<&'a ast::Expr> {
    args.iter()
        .find(|a| a.name.as_deref() == Some(name))
        .map(|a| &a.value)
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // default_rng: construct a handle (no generator argument).
    if method == "default_rng" {
        let pos = positional(args);
        let d = lowerer.new_value(Ty::Ptr);
        if pos.is_empty() {
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "zap_random_default_rng_entropy".to_string(),
                args: vec![],
            });
        } else {
            let (v, t) = lowerer.expr(pos[0])?;
            let seed = lowerer.coerce(v, t, Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "zap_random_default_rng".to_string(),
                args: vec![seed],
            });
        }
        return Ok(Some((d, Ty::Ptr)));
    }
    // Python-only names (no NumPy overload): handled by the legacy path.
    if matches!(method, "seed" | "randint") {
        return lower_python(lowerer, method, args);
    }
    // Other NumPy-only draws: the generator is positional[0].
    if is_numpy_only(method) {
        let pos = positional(args);
        let g = lowerer.expr(pos[0])?.0;
        return lower_numpy(lowerer, method, args, g);
    }
    // Overloaded names: random / uniform / choice / shuffle.
    let pos = positional(args);
    if pos.is_empty() {
        // random() — the Python 0-arg module generator.
        if method == "random" {
            let d = lowerer.new_value(Ty::F64);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "zap_random_f64".to_string(),
                args: vec![],
            });
            return Ok(Some((d, Ty::F64)));
        }
        return Err(err(format!("random.{method}: missing arguments")));
    }
    // Lower the first positional ONCE; a Ptr means the NumPy `Generator` path.
    let (a0, t0) = lowerer.expr(pos[0])?;
    if matches!(t0, Ty::Ptr) {
        return lower_numpy(lowerer, method, args, a0);
    }
    lower_python_overloaded(lowerer, method, args, a0, t0)
}

// ---- NumPy `Generator` surface ----

/// Lower a `size=` keyword to a freshly allocated result array of `dtype`,
/// returning `(array, ndim)`; `None` when there is no `size=` (a scalar draw).
/// `size` is an int (1-D) or a shape tuple (N-D).
fn size_array(
    lowerer: &mut FnLower,
    args: &[ast::Arg],
    dtype: Dtype,
) -> Result<Option<(Value, u8)>, LowerError> {
    let Some(e) = kw(args, "size") else {
        return Ok(None);
    };
    if let ast::Expr::Tuple(elems, _) = e {
        let mut dims = Vec::with_capacity(elems.len());
        for el in elems {
            let (v, t) = lowerer.expr(el)?;
            dims.push(lowerer.coerce(v, t, Ty::I64));
        }
        let nd = dims.len() as u8;
        let arr = lowerer.array_alloc_shaped(dtype, &dims);
        Ok(Some((arr, nd.max(1))))
    } else {
        let (v, t) = lowerer.expr(e)?;
        let len = lowerer.coerce(v, t, Ty::I64);
        let arr = lowerer.array_alloc_shaped(dtype, &[len]);
        Ok(Some((arr, 1)))
    }
}

/// A required positional parameter lowered to `f64`.
fn f64_req(lowerer: &mut FnLower, pos: &[&ast::Expr], i: usize) -> Result<Value, LowerError> {
    let e = pos.get(i).ok_or_else(|| err("random: missing argument"))?;
    let (v, t) = lowerer.expr(e)?;
    Ok(lowerer.coerce(v, t, Ty::F64))
}
/// A required positional parameter lowered to `i64`.
fn i64_req(lowerer: &mut FnLower, pos: &[&ast::Expr], i: usize) -> Result<Value, LowerError> {
    let e = pos.get(i).ok_or_else(|| err("random: missing argument"))?;
    let (v, t) = lowerer.expr(e)?;
    Ok(lowerer.coerce(v, t, Ty::I64))
}
/// An optional positional `f64` parameter, defaulting to `default`.
fn f64_opt(
    lowerer: &mut FnLower,
    pos: &[&ast::Expr],
    i: usize,
    default: f64,
) -> Result<Value, LowerError> {
    match pos.get(i) {
        Some(e) => {
            let (v, t) = lowerer.expr(e)?;
            Ok(lowerer.coerce(v, t, Ty::F64))
        }
        None => Ok(lowerer.fconst(Ty::F64, default)),
    }
}
/// A boolean-ish keyword lowered to an `i64` flag (0/1), defaulting to `default`.
fn flag_kw(
    lowerer: &mut FnLower,
    args: &[ast::Arg],
    name: &str,
    default: i64,
) -> Result<Value, LowerError> {
    match kw(args, name) {
        Some(e) => {
            let (v, t) = lowerer.expr(e)?;
            Ok(lowerer.coerce(v, t, Ty::I64))
        }
        None => Ok(lowerer.iconst(Ty::I64, default)),
    }
}

/// Emit either a scalar draw (`sfn(g, rest...)`) or, when `size=` is present, an
/// array fill (`ffn(g, arr, rest...)`) into a freshly shaped array.
fn draw(
    lowerer: &mut FnLower,
    args: &[ast::Arg],
    dtype: Dtype,
    sfn: &str,
    ffn: &str,
    g: Value,
    rest: Vec<Value>,
) -> Result<(Value, Ty), LowerError> {
    // The scalar element type follows the dtype (f64 draws vs i64 draws).
    let scalar_ty = if matches!(dtype, Dtype::I64) {
        Ty::I64
    } else {
        Ty::F64
    };
    if let Some((arr, nd)) = size_array(lowerer, args, dtype)? {
        let mut a = vec![g, arr];
        a.extend(rest);
        lowerer.emit(Inst::Call {
            dest: None,
            func: ffn.to_string(),
            args: a,
        });
        Ok((arr, Ty::Array(dtype, nd)))
    } else {
        let mut a = vec![g];
        a.extend(rest);
        let d = lowerer.new_value(scalar_ty);
        lowerer.emit(Inst::Call {
            dest: Some(d),
            func: sfn.to_string(),
            args: a,
        });
        Ok((d, scalar_ty))
    }
}

fn lower_numpy(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
    g: Value,
) -> Result<Option<(Value, Ty)>, LowerError> {
    let pos = positional(args);
    let r = match method {
        "random" => draw(
            lowerer,
            args,
            Dtype::F64,
            "zap_random_gen_random",
            "zap_random_gen_random_fill",
            g,
            vec![],
        )?,
        "standard_normal" => draw(
            lowerer,
            args,
            Dtype::F64,
            "zap_random_gen_standard_normal",
            "zap_random_gen_standard_normal_fill",
            g,
            vec![],
        )?,
        "standard_exponential" => draw(
            lowerer,
            args,
            Dtype::F64,
            "zap_random_gen_standard_exponential",
            "zap_random_gen_standard_exponential_fill",
            g,
            vec![],
        )?,
        "uniform" => {
            let lo = f64_req(lowerer, &pos, 1)?;
            let hi = f64_req(lowerer, &pos, 2)?;
            draw(
                lowerer,
                args,
                Dtype::F64,
                "zap_random_gen_uniform",
                "zap_random_gen_uniform_fill",
                g,
                vec![lo, hi],
            )?
        }
        "normal" => {
            let loc = f64_opt(lowerer, &pos, 1, 0.0)?;
            let scale = f64_opt(lowerer, &pos, 2, 1.0)?;
            draw(
                lowerer,
                args,
                Dtype::F64,
                "zap_random_gen_normal",
                "zap_random_gen_normal_fill",
                g,
                vec![loc, scale],
            )?
        }
        "exponential" => {
            let scale = f64_opt(lowerer, &pos, 1, 1.0)?;
            draw(
                lowerer,
                args,
                Dtype::F64,
                "zap_random_gen_exponential",
                "zap_random_gen_exponential_fill",
                g,
                vec![scale],
            )?
        }
        "standard_gamma" => {
            let shape = f64_req(lowerer, &pos, 1)?;
            draw(
                lowerer,
                args,
                Dtype::F64,
                "zap_random_gen_standard_gamma",
                "zap_random_gen_standard_gamma_fill",
                g,
                vec![shape],
            )?
        }
        "gamma" => {
            let shape = f64_req(lowerer, &pos, 1)?;
            let scale = f64_opt(lowerer, &pos, 2, 1.0)?;
            draw(
                lowerer,
                args,
                Dtype::F64,
                "zap_random_gen_gamma",
                "zap_random_gen_gamma_fill",
                g,
                vec![shape, scale],
            )?
        }
        "beta" => {
            let a = f64_req(lowerer, &pos, 1)?;
            let b = f64_req(lowerer, &pos, 2)?;
            draw(
                lowerer,
                args,
                Dtype::F64,
                "zap_random_gen_beta",
                "zap_random_gen_beta_fill",
                g,
                vec![a, b],
            )?
        }
        "integers" => {
            let lo = i64_req(lowerer, &pos, 1)?;
            let hi = i64_req(lowerer, &pos, 2)?;
            let endpoint = flag_kw(lowerer, args, "endpoint", 0)?;
            draw(
                lowerer,
                args,
                Dtype::I64,
                "zap_random_gen_integers",
                "zap_random_gen_integers_fill",
                g,
                vec![lo, hi, endpoint],
            )?
        }
        "poisson" => {
            let lam = f64_opt(lowerer, &pos, 1, 1.0)?;
            draw(
                lowerer,
                args,
                Dtype::I64,
                "zap_random_gen_poisson",
                "zap_random_gen_poisson_fill",
                g,
                vec![lam],
            )?
        }
        "binomial" => {
            let n = i64_req(lowerer, &pos, 1)?;
            let p = f64_req(lowerer, &pos, 2)?;
            draw(
                lowerer,
                args,
                Dtype::I64,
                "zap_random_gen_binomial",
                "zap_random_gen_binomial_fill",
                g,
                vec![n, p],
            )?
        }
        "choice" => lower_choice(lowerer, args, &pos, g)?,
        "shuffle" => {
            let (arr, _) = lowerer.expr(pos[1])?;
            lowerer.emit(Inst::Call {
                dest: None,
                func: "zap_random_gen_shuffle".to_string(),
                args: vec![g, arr],
            });
            (lowerer.iconst(Ty::I32, 0), Ty::I32)
        }
        "permutation" => {
            let (x, xt) = lowerer.expr(pos[1])?;
            match xt {
                Ty::Array(dt, nd) => {
                    let d = lowerer.new_value(Ty::Array(dt, nd));
                    lowerer.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_random_gen_permutation".to_string(),
                        args: vec![g, x],
                    });
                    (d, Ty::Array(dt, nd))
                }
                _ => {
                    let n = lowerer.coerce(x, xt, Ty::I64);
                    let out = Ty::Array(Dtype::I64, 1);
                    let d = lowerer.new_value(out);
                    lowerer.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_random_gen_permutation_n".to_string(),
                        args: vec![g, n],
                    });
                    (d, out)
                }
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(r))
}

/// `choice(g, a, size=, replace=, p=, shuffle=)` — `a` is an int population, so
/// the result is an i64 index (scalar) or index array (with `size=`). Fills an
/// i64 array via `zap_random_gen_choice_fill`; the scalar form reads element 0.
fn lower_choice(
    lowerer: &mut FnLower,
    args: &[ast::Arg],
    pos: &[&ast::Expr],
    g: Value,
) -> Result<(Value, Ty), LowerError> {
    let pop = i64_req(lowerer, pos, 1)?;
    let replace = flag_kw(lowerer, args, "replace", 1)?;
    let shuffle = flag_kw(lowerer, args, "shuffle", 1)?;
    let p = match kw(args, "p") {
        Some(e) => lowerer.expr(e)?.0,
        None => {
            let n = lowerer.new_value(Ty::Ptr);
            lowerer.emit(Inst::NullConst { dest: n });
            n
        }
    };
    if let Some((arr, nd)) = size_array(lowerer, args, Dtype::I64)? {
        lowerer.emit(Inst::Call {
            dest: None,
            func: "zap_random_gen_choice_fill".to_string(),
            args: vec![g, arr, pop, replace, p, shuffle],
        });
        Ok((arr, Ty::Array(Dtype::I64, nd)))
    } else {
        let one = lowerer.iconst(Ty::I64, 1);
        let arr = lowerer.array_alloc_shaped(Dtype::I64, &[one]);
        lowerer.emit(Inst::Call {
            dest: None,
            func: "zap_random_gen_choice_fill".to_string(),
            args: vec![g, arr, pop, replace, p, shuffle],
        });
        let zero = lowerer.iconst(Ty::I64, 0);
        let d = lowerer.new_value(Ty::I64);
        lowerer.emit(Inst::Call {
            dest: Some(d),
            func: "zap_array_get_i64".to_string(),
            args: vec![arr, zero],
        });
        Ok((d, Ty::I64))
    }
}

// ---- pre-existing Python xoshiro surface ----

fn lower_python(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    match method {
        "seed" => {
            let (v, t) = lowerer.expr(&args[0].value)?;
            let n = lowerer.coerce(v, t, Ty::I64);
            lowerer.emit(Inst::Call {
                dest: None,
                func: "zap_random_seed".to_string(),
                args: vec![n],
            });
            Ok(Some((lowerer.iconst(Ty::I32, 0), Ty::I32)))
        }
        "randint" => {
            let a = i64_req(lowerer, &positional(args), 0)?;
            let b = i64_req(lowerer, &positional(args), 1)?;
            let d = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "zap_random_randint".to_string(),
                args: vec![a, b],
            });
            Ok(Some((d, Ty::I64)))
        }
        _ => Ok(None),
    }
}

/// The Python overloaded forms, with the first argument already lowered as
/// `(a0, t0)` (used to disambiguate from the NumPy `Generator` path).
fn lower_python_overloaded(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
    a0: Value,
    t0: Ty,
) -> Result<Option<(Value, Ty)>, LowerError> {
    let pos = positional(args);
    let r = match method {
        "uniform" => {
            let a = lowerer.coerce(a0, t0, Ty::F64);
            let b = f64_req(lowerer, &pos, 1)?;
            let d = lowerer.new_value(Ty::F64);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "zap_random_uniform".to_string(),
                args: vec![a, b],
            });
            (d, Ty::F64)
        }
        "choice" => {
            let elem = match t0 {
                Ty::List(elem) => elem,
                _ => return Err(err("random.choice expects a list")),
            };
            let idx = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(idx),
                func: "zap_random_list_index".to_string(),
                args: vec![a0],
            });
            let d = lowerer.new_value(elem.ty());
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: format!("zap_list_get_{}", elem.val_suffix()),
                args: vec![a0, idx],
            });
            (d, elem.ty())
        }
        "shuffle" => {
            lowerer.emit(Inst::Call {
                dest: None,
                func: "zap_random_shuffle".to_string(),
                args: vec![a0],
            });
            (lowerer.iconst(Ty::I32, 0), Ty::I32)
        }
        _ => return Ok(None),
    };
    Ok(Some(r))
}
