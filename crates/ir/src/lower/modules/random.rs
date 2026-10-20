//! `random` module — lowering facet (ticket #98, D-0056).
//!
//! Scalar generators (`seed`/`random`/`uniform`/`randint`) are single runtime
//! calls into the xoshiro256** generator in `zaprt.c`. `choice`/`shuffle` act on
//! a `list[T]`: `choice` asks the runtime for a random valid index
//! (`zap_random_list_index`, which aborts on an empty list) and then reuses the
//! list's typed element getter, mirroring how `l[i]` lowers; `shuffle` permutes
//! the list's element words in place (`zap_random_shuffle`).

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "random",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    match method {
        "seed" => {
            let (v, t) = lowerer.expr(&arg(args, 0)?.value)?;
            let n = lowerer.coerce(v, t, Ty::I64);
            Ok(Some(call_void(lowerer, "zap_random_seed", vec![n])))
        }
        "random" => {
            let d = lowerer.new_value(Ty::F64);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "zap_random_f64".to_string(),
                args: vec![],
            });
            Ok(Some((d, Ty::F64)))
        }
        "uniform" => {
            let a = f64_arg(lowerer, args, 0)?;
            let b = f64_arg(lowerer, args, 1)?;
            let d = lowerer.new_value(Ty::F64);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "zap_random_uniform".to_string(),
                args: vec![a, b],
            });
            Ok(Some((d, Ty::F64)))
        }
        "randint" => {
            let a = i64_arg(lowerer, args, 0)?;
            let b = i64_arg(lowerer, args, 1)?;
            let d = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "zap_random_randint".to_string(),
                args: vec![a, b],
            });
            Ok(Some((d, Ty::I64)))
        }
        "choice" => {
            let (lst, elem) = list_arg(lowerer, args)?;
            let idx = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(idx),
                func: "zap_random_list_index".to_string(),
                args: vec![lst],
            });
            let d = lowerer.new_value(elem.ty());
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: format!("zap_list_get_{}", elem.val_suffix()),
                args: vec![lst, idx],
            });
            Ok(Some((d, elem.ty())))
        }
        "shuffle" => {
            let (lst, _elem) = list_arg(lowerer, args)?;
            Ok(Some(call_void(lowerer, "zap_random_shuffle", vec![lst])))
        }
        _ => Ok(None),
    }
}

/// Emit a statement-position runtime call with no result, yielding a throwaway
/// `i32 0` so the call expression still has a value (mirrors `super::call0`).
fn call_void(lowerer: &mut FnLower, func: &str, args: Vec<Value>) -> (Value, Ty) {
    lowerer.emit(Inst::Call {
        dest: None,
        func: func.to_string(),
        args,
    });
    (lowerer.iconst(Ty::I32, 0), Ty::I32)
}

fn arg(args: &[ast::Arg], i: usize) -> Result<&ast::Arg, LowerError> {
    args.get(i)
        .ok_or_else(|| err(format!("random: missing argument {i}")))
}

/// Lower argument `i` and widen it to `f64`.
fn f64_arg(lowerer: &mut FnLower, args: &[ast::Arg], i: usize) -> Result<Value, LowerError> {
    let (v, t) = lowerer.expr(&arg(args, i)?.value)?;
    Ok(lowerer.coerce(v, t, Ty::F64))
}

/// Lower argument `i` and widen it to `i64`.
fn i64_arg(lowerer: &mut FnLower, args: &[ast::Arg], i: usize) -> Result<Value, LowerError> {
    let (v, t) = lowerer.expr(&arg(args, i)?.value)?;
    Ok(lowerer.coerce(v, t, Ty::I64))
}

/// Lower the single `list[T]` argument, returning its value and element kind.
fn list_arg(lowerer: &mut FnLower, args: &[ast::Arg]) -> Result<(Value, Elem), LowerError> {
    let (lst, lty) = lowerer.expr(&arg(args, 0)?.value)?;
    match lty {
        Ty::List(elem) => Ok((lst, elem)),
        _ => Err(err("random.choice/shuffle expects a list")),
    }
}
