//! `fs` module — lowering facet (ticket #97, D-0056; filesystem gaps #968,
//! D-0211).
//!
//! Most entries lower to a single backing `zap_fs_*` runtime call. `fs.stat`
//! builds the prelude `Stat` value struct — `StructNew` then a `FieldSet` per
//! field, each fed by a `zap_fs_stat_<field>(path)` runtime getter (the `KeyInfo`
//! pattern, crypto #643). `size` is emitted first as a throwing call: it is the
//! validating gate, so a non-existent path raises a catchable error before any
//! other field is read.

use zapc_syntax::ast;

use super::support::{call_bool, call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "fs", lower };

/// Build the prelude `Stat` value struct from the per-field getters. Field order
/// matches `PRELUDE_SRC`: size(0), mtime(1), ctime(2), mode(3) are `int64`;
/// is_dir(4), is_file(5), is_symlink(6) are `bool`.
fn lower_stat(lowerer: &mut FnLower, arg: &ast::Arg) -> Result<(Value, Ty), LowerError> {
    let sid = *lowerer
        .ctx
        .ids
        .get("Stat")
        .ok_or_else(|| err("Stat struct unavailable (built-in prelude missing)"))?;
    let (path, _) = lowerer.expr(&arg.value)?;
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });

    // Field 0: size (int64) — the validating gate, throwing (raises on a bad path).
    let size = lowerer.new_value(Ty::I64);
    lowerer.throwing_call(Some(size), "zap_fs_stat_size", vec![path]);
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 0,
        val: size,
    });

    // Fields 1..=3: mtime / ctime / mode (int64). Reached only once `size`
    // validated the path, so they never raise for a readable path.
    for (idx, func) in [
        (1u32, "zap_fs_stat_mtime"),
        (2, "zap_fs_stat_ctime"),
        (3, "zap_fs_stat_mode"),
    ] {
        let v = lowerer.new_value(Ty::I64);
        lowerer.emit(Inst::Call {
            dest: Some(v),
            func: func.to_string(),
            args: vec![path],
        });
        lowerer.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx,
            val: v,
        });
    }

    // Fields 4..=6: is_dir / is_file / is_symlink (bool) — C int64 0/1 -> i1.
    for (idx, func) in [
        (4u32, "zap_fs_stat_is_dir"),
        (5, "zap_fs_stat_is_file"),
        (6, "zap_fs_stat_is_symlink"),
    ] {
        let (v, _) = call_bool(lowerer, func, vec![path]);
        lowerer.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx,
            val: v,
        });
    }

    Ok((dest, Ty::Struct(sid)))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // stat builds a value struct from the path arg; the rest are single runtime
    // calls over the lowered argument words.
    if method == "stat" {
        return Ok(Some(lower_stat(lowerer, &args[0])?));
    }
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "read_file" => call_ret(lowerer, "zap_fs_read_file", a, Ty::Ptr, true),
        "write_file" => call_void(lowerer, "zap_fs_write_file", a, true),
        "exists" => call_bool(lowerer, "zap_fs_exists", a),
        "is_dir" => call_bool(lowerer, "zap_fs_is_dir", a),
        "is_file" => call_bool(lowerer, "zap_fs_is_file", a),
        "size" => call_ret(lowerer, "zap_fs_size", a, Ty::I64, true),
        "listdir" => call_ret(lowerer, "zap_os_listdir", a, Ty::List(Elem::Ptr), true),
        "mkdtemp" => call_ret(lowerer, "zap_fs_mkdtemp", a, Ty::Ptr, true),
        // Filesystem gaps (#968, D-0211).
        "mtime" => call_ret(lowerer, "zap_fs_mtime", a, Ty::I64, true),
        "rename" => call_void(lowerer, "zap_fs_rename", a, true),
        "move" => call_void(lowerer, "zap_fs_move", a, true),
        "copy" => call_void(lowerer, "zap_fs_copy", a, true),
        "makedirs" => call_void(lowerer, "zap_fs_makedirs", a, true),
        "rmtree" => call_void(lowerer, "zap_fs_rmtree", a, true),
        "read_bytes" => call_ret(lowerer, "zap_fs_read_bytes", a, Ty::Bytes, true),
        "write_bytes" => call_void(lowerer, "zap_fs_write_bytes", a, true),
        "append" => call_void(lowerer, "zap_fs_append", a, true),
        _ => return Ok(None),
    }))
}
