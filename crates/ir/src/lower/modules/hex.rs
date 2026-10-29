//! `hex` module — lowering facet (ticket #337, D-0056).
//!
//! Emits `zap_hex_encode(data) -> str` / `zap_hex_decode(s) -> bytes`
//! (implemented in `zaprt.c`). `str`/`bytes` arguments are a `ZapStr*` and pass
//! straight through. Sema has type-checked these calls.

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "hex", lower };

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let decode = match method {
        "encode" => false,
        "decode" => true,
        _ => return Ok(None),
    };
    if args.len() != 1 {
        return Err(err("hex codec takes exactly one argument"));
    }
    let (v, _) = lowerer.expr(&args[0].value)?;
    if decode {
        // Decode validates its input (even length, [0-9a-fA-F]), raising a
        // catchable exception otherwise, so it routes through `throwing_call`.
        let d = lowerer.new_value(Ty::Bytes);
        lowerer.throwing_call(Some(d), "zap_hex_decode", vec![v]);
        Ok(Some((d, Ty::Bytes)))
    } else {
        let d = lowerer.new_value(Ty::Ptr);
        lowerer.emit(Inst::Call {
            dest: Some(d),
            func: "zap_hex_encode".to_string(),
            args: vec![v],
        });
        Ok(Some((d, Ty::Ptr)))
    }
}
