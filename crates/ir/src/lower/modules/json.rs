//! `json` module — lowering facet (ticket #100/#131, D-0059).
//!
//! `json.loads(s)` lowers to a call to the Zap prelude builder `__json_loads`
//! (which constructs the `JsonValue` tree via ordinary enum/list/dict ops,
//! keeping layout knowledge out of the runtime). The internal `json._lex_*`
//! cursor primitives lower to the `zap_json_lex_*` C tokenizer in `zaprt.c`.
//! `str`/`bytes`/cursor-handle values are all `Ty::Ptr`.

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "json",
    lower,
};

/// Lower a one-argument call `func(arg)` with the given result type.
fn call1(
    lowerer: &mut FnLower,
    func: &str,
    arg: &ast::Arg,
    ret: Ty,
) -> Result<(Value, Ty), LowerError> {
    let (v, _) = lowerer.expr(&arg.value)?;
    let d = lowerer.new_value(ret);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: func.to_string(),
        args: vec![v],
    });
    Ok((d, ret))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let r = match method {
        // json.loads(s) -> JsonValue: call the prelude tree builder.
        "loads" => {
            let (s, _) = lowerer.expr(&args[0].value)?;
            let sid = lowerer
                .ctx
                .ids
                .get("JsonValue")
                .copied()
                .ok_or_else(|| err("JsonValue unavailable (built-in prelude missing)"))?;
            let d = lowerer.new_value(Ty::Struct(sid));
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "__json_loads".to_string(),
                args: vec![s],
            });
            (d, Ty::Struct(sid))
        }
        // Internal tokenizer-cursor primitives -> zap_json_lex_* (zaprt.c).
        "_lex_new" => call1(lowerer, "zap_json_lex_new", &args[0], Ty::Ptr)?,
        "_lex_next" => call1(lowerer, "zap_json_lex_next", &args[0], Ty::I64)?,
        "_lex_int" => call1(lowerer, "zap_json_lex_int", &args[0], Ty::I64)?,
        "_lex_float" => call1(lowerer, "zap_json_lex_float", &args[0], Ty::F64)?,
        "_lex_str" => call1(lowerer, "zap_json_lex_str", &args[0], Ty::Ptr)?,
        _ => return Ok(None),
    };
    Ok(Some(r))
}
