//! `gc` module — lowering facet (ticket #77, D-0058).

use zapc_syntax::ast;

use super::{call0, ModuleLower};
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "gc", lower };

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    _args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    match method {
        "collect" => Ok(Some(call0(lowerer, "zap_gc_collect", Ty::Void))),
        "stats" => Ok(Some(lower_gc_stats(lowerer)?)),
        _ => Ok(None),
    }
}

/// Build a `MemStats` value struct from the runtime GC/scheduler getters
/// (ticket #77). Field order matches the prelude's `MemStats`:
/// `heap_bytes, live_bytes, free_bytes, gc_count, goroutines`.
fn lower_gc_stats(lowerer: &mut FnLower) -> Result<(Value, Ty), LowerError> {
    let sid = *lowerer
        .ctx
        .ids
        .get("MemStats")
        .ok_or_else(|| err("MemStats struct unavailable (built-in prelude missing)"))?;
    let getters = [
        "zap_gc_heap_bytes",
        "zap_gc_live_bytes",
        "zap_gc_free_bytes",
        "zap_gc_count",
        "zap_runtime_num_goroutines",
    ];
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    for (idx, func) in getters.iter().enumerate() {
        let v = lowerer.new_value(Ty::I64);
        lowerer.emit(Inst::Call {
            dest: Some(v),
            func: (*func).to_string(),
            args: vec![],
        });
        lowerer.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: idx as u32,
            val: v,
        });
    }
    Ok((dest, Ty::Struct(sid)))
}
