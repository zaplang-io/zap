//! `yaml` module — lowering facet (ticket #149, D-0063).
//!
//! `yaml.load(s)` lowers to the Zap prelude builder `__yaml_loads` (which calls
//! `yaml._to_json` then the existing `__json_loads`, so the YAML subset becomes
//! a `JsonValue` tree without the runtime knowing the DOM layout). `yaml.dump(v)`
//! lowers to the prelude block-style writer `__yaml_dumps`. The internal
//! `yaml._to_json` lowers to the `zap_yaml_to_json` C transpiler in `zaprt.c`.
//! `str`/`bytes` values are pointers.

use zapc_syntax::ast;

use super::support::{call_ret, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "yaml",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // yaml.load(s) -> JsonValue: call the prelude tree builder, whose result
    // type is the compiler-known `JsonValue` enum struct.
    if method == "load" {
        let (s, _) = lowerer.expr(&args[0].value)?;
        let sid = lowerer
            .ctx
            .ids
            .get("JsonValue")
            .copied()
            .ok_or_else(|| err("JsonValue unavailable (built-in prelude missing)"))?;
        let d = lowerer.new_value(Ty::Struct(sid));
        // Malformed YAML (e.g. tab indentation) raises a catchable exception in
        // the `zap_yaml_to_json` C transpiler reached through `__yaml_loads`
        // (#1566), so this must lower through an unwind edge to the handler.
        lowerer.throwing_call(Some(d), "__yaml_loads", vec![s]);
        return Ok(Some((d, Ty::Struct(sid))));
    }
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        // yaml.dump(v) -> str: the prelude block-style serializer.
        "dump" => call_ret(lowerer, "__yaml_dumps", a, Ty::Ptr, false),
        // Internal YAML -> JSON transpiler (C runtime). Raises a catchable
        // exception on malformed YAML (#1566).
        "_to_json" => call_ret(lowerer, "zap_yaml_to_json", a, Ty::Ptr, true),
        _ => return Ok(None),
    }))
}
