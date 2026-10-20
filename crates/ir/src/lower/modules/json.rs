//! `json` module — lowering facet (ticket #100/#131, D-0059).
//!
//! `json.loads(s)` / `json.dumps(v)` lower to the Zap prelude builders
//! `__json_loads` / `__json_dumps` (which construct/walk the `JsonValue` tree via
//! ordinary enum/list/dict ops, keeping layout knowledge out of the runtime).
//! The internal `json._lex_*` cursor primitives and the `dumps` helpers
//! (`_escape`/`_obj_keys`/`_join`, plus integer/float formatting) lower to the
//! `zap_json_*` C runtime in `zaprt.c`. `str`/`bytes`/cursor-handle/list values
//! are all pointers.

use zapc_syntax::ast;

use super::support::{call_ret, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "json",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // json.loads(s) -> JsonValue: call the prelude tree builder, whose result
    // type is the compiler-known `JsonValue` enum struct.
    if method == "loads" {
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
        return Ok(Some((d, Ty::Struct(sid))));
    }
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        // json.dumps(v) -> str: call the prelude serializer.
        "dumps" => call_ret(lowerer, "__json_dumps", a, Ty::Ptr, false),
        // Tokenizer cursor primitives.
        "_lex_new" => call_ret(lowerer, "zap_json_lex_new", a, Ty::Ptr, false),
        "_lex_next" => call_ret(lowerer, "zap_json_lex_next", a, Ty::I64, false),
        "_lex_int" => call_ret(lowerer, "zap_json_lex_int", a, Ty::I64, false),
        "_lex_float" => call_ret(lowerer, "zap_json_lex_float", a, Ty::F64, false),
        "_lex_str" => call_ret(lowerer, "zap_json_lex_str", a, Ty::Ptr, false),
        // dumps helpers.
        "_escape" => call_ret(lowerer, "zap_json_escape", a, Ty::Ptr, false),
        "_i64_str" => call_ret(lowerer, "zap_i64_to_str", a, Ty::Ptr, false),
        "_f64_str" => call_ret(lowerer, "zap_f64_to_str", a, Ty::Ptr, false),
        "_obj_keys" => call_ret(lowerer, "zap_dict_keys", a, Ty::List(Elem::Ptr), false),
        "_join" => call_ret(lowerer, "zap_json_join", a, Ty::Ptr, false),
        _ => return Ok(None),
    }))
}
