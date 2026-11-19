//! `decimal` module — lowering facet (ticket #981, #981).
//!
//! The two constructors build the prelude `Decimal` reference struct by
//! delegating to its prelude free functions (`zapc_syntax::parser::PRELUDE_SRC`),
//! exactly like `argparse.ArgumentParser → __argparse_new`. There is no runtime
//! C symbol — all Decimal arithmetic is pure prelude Zap over `bigint`.
//!
//!   * `decimal.Decimal(s)`   → `__decimal_parse(s)`     (str, throwing: ValueError)
//!   * `decimal.Decimal(i)`   → `__decimal_from_int(i)`  (int, exact)
//!   * `decimal.from_float(f)`→ `__decimal_from_float(f)`(throwing: ValueError)
//!
//! `Decimal(value)` dispatches on the lowered argument type: an integer word
//! promotes exactly; anything else is the parsed `str` path (sema has already
//! rejected float / other argument types).

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "decimal",
    lower,
};

/// The id of the prelude `Decimal` struct (injected when a program names the
/// `decimal` module or the struct — see `references_prelude`).
fn decimal_sid(lowerer: &FnLower) -> Result<u32, LowerError> {
    lowerer
        .ctx
        .ids
        .get("Decimal")
        .copied()
        .ok_or_else(|| err("Decimal struct unavailable (built-in prelude missing)"))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    match method {
        "Decimal" => {
            if args.len() != 1 {
                return Err(err("decimal.Decimal takes exactly one argument"));
            }
            let sid = decimal_sid(lowerer)?;
            let (v, vt) = lowerer.expr(&args[0].value)?;
            let dest = lowerer.new_value(Ty::Struct(sid));
            if vt.int_width().is_some() && !matches!(vt, Ty::Char) {
                // Exact int → Decimal(coeff=i, exp=0). Never raises.
                let iv = lowerer.coerce(v, vt, Ty::I64);
                lowerer.emit(Inst::Call {
                    dest: Some(dest),
                    func: "__decimal_from_int".to_string(),
                    args: vec![iv],
                });
            } else {
                // str → parse (raises ValueError on a malformed literal).
                let sv = lowerer.coerce(v, vt, Ty::Ptr);
                lowerer.throwing_call(Some(dest), "__decimal_parse", vec![sv]);
            }
            Ok(Some((dest, Ty::Struct(sid))))
        }
        "from_float" => {
            if args.len() != 1 {
                return Err(err("decimal.from_float takes exactly one argument"));
            }
            let sid = decimal_sid(lowerer)?;
            let (v, vt) = lowerer.expr(&args[0].value)?;
            let fv = lowerer.coerce(v, vt, Ty::F64);
            let dest = lowerer.new_value(Ty::Struct(sid));
            lowerer.throwing_call(Some(dest), "__decimal_from_float", vec![fv]);
            Ok(Some((dest, Ty::Struct(sid))))
        }
        _ => Ok(None),
    }
}
