//! `fractions` module — lowering facet (ticket #982, epic #966).
//!
//! The constructor builds the prelude `Fraction` reference struct by delegating
//! to its prelude free functions (`zapc_syntax::parser::PRELUDE_SRC`), exactly
//! like `decimal.Decimal → __decimal_*`. There is no runtime C symbol — all
//! Fraction arithmetic is pure prelude Zap over `bigint`.
//!
//!   * `fractions.Fraction(n, d)` → `__fraction_from_pair(n, d)` (throwing: ValueError on d == 0)
//!   * `fractions.Fraction(i)`    → `__fraction_from_int(i)`     (int, exact)
//!   * `fractions.Fraction(s)`    → `__fraction_parse(s)`        (str, throwing: ValueError)
//!
//! `Fraction(value)` dispatches on the lowered argument type: an integer word
//! promotes exactly; anything else is the parsed `str` path (sema has already
//! rejected float / other argument types). The two-argument form always takes
//! two integers.

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "fractions",
    lower,
};

/// The id of the prelude `Fraction` struct (injected when a program names the
/// `fractions` module or the struct — see `references_prelude`).
fn fraction_sid(lowerer: &FnLower) -> Result<u32, LowerError> {
    lowerer
        .ctx
        .ids
        .get("Fraction")
        .copied()
        .ok_or_else(|| err("Fraction struct unavailable (built-in prelude missing)"))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    match method {
        "Fraction" => {
            let sid = fraction_sid(lowerer)?;
            let dest = lowerer.new_value(Ty::Struct(sid));
            match args.len() {
                1 => {
                    let (v, vt) = lowerer.expr(&args[0].value)?;
                    if vt.int_width().is_some() && !matches!(vt, Ty::Char) {
                        // Exact int → Fraction(num=i, den=1). Never raises.
                        let iv = lowerer.coerce(v, vt, Ty::I64);
                        lowerer.emit(Inst::Call {
                            dest: Some(dest),
                            func: "__fraction_from_int".to_string(),
                            args: vec![iv],
                        });
                    } else {
                        // str → parse (raises ValueError on a malformed literal).
                        let sv = lowerer.coerce(v, vt, Ty::Ptr);
                        lowerer.throwing_call(Some(dest), "__fraction_parse", vec![sv]);
                    }
                }
                2 => {
                    // (num, den) → build + reduce; a zero denominator raises.
                    let (nv, nt) = lowerer.expr(&args[0].value)?;
                    let niv = lowerer.coerce(nv, nt, Ty::I64);
                    let (dv, dt) = lowerer.expr(&args[1].value)?;
                    let div = lowerer.coerce(dv, dt, Ty::I64);
                    lowerer.throwing_call(Some(dest), "__fraction_from_pair", vec![niv, div]);
                }
                n => {
                    return Err(err(format!(
                        "fractions.Fraction takes 1 or 2 arguments, got {n}"
                    )))
                }
            }
            Ok(Some((dest, Ty::Struct(sid))))
        }
        _ => Ok(None),
    }
}
