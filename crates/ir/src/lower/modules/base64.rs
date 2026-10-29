//! `base64` module — lowering facet (ticket #337, D-0056).
//!
//! Emits `zap_base64_encode(data, urlsafe, pad)` / `zap_base64_decode(s,
//! urlsafe)` (implemented in `zaprt.c`). `str`/`bytes` arguments are already a
//! `ZapStr*`, so they pass straight through; the `urlsafe`/`pad` selectors are
//! `i64` constants. Encode results are `str` (`Ptr`); decode results are
//! `bytes` (`Ty::Bytes`). Sema has type-checked these calls.

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "base64",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // Each method is `(runtime selectors, result type)`: encoders pick the
    // alphabet (urlsafe) and padding; decoders pick only the alphabet.
    let (urlsafe, pad, decode) = match method {
        "encode" => (0, 1, false),
        "encode_nopad" => (0, 0, false),
        "urlsafe_encode" => (1, 1, false),
        "urlsafe_encode_nopad" => (1, 0, false),
        "decode" => (0, 0, true),
        "urlsafe_decode" => (1, 0, true),
        _ => return Ok(None),
    };
    if args.len() != 1 {
        return Err(err("base64 codec takes exactly one argument"));
    }
    let (v, _) = lowerer.expr(&args[0].value)?;
    if decode {
        // Decode validates its input, raising a catchable exception on a bad
        // character, so it routes through `throwing_call`.
        let us = lowerer.iconst(Ty::I64, urlsafe);
        let d = lowerer.new_value(Ty::Bytes);
        lowerer.throwing_call(Some(d), "zap_base64_decode", vec![v, us]);
        Ok(Some((d, Ty::Bytes)))
    } else {
        let us = lowerer.iconst(Ty::I64, urlsafe);
        let pd = lowerer.iconst(Ty::I64, pad);
        let d = lowerer.new_value(Ty::Ptr);
        lowerer.emit(Inst::Call {
            dest: Some(d),
            func: "zap_base64_encode".to_string(),
            args: vec![v, us, pd],
        });
        Ok(Some((d, Ty::Ptr)))
    }
}
