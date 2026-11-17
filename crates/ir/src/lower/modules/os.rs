//! `os` module — lowering facet (ticket #97, D-0056).

use zapc_syntax::ast;

use super::support::{call_bool, call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "os", lower };

/// Build a `(str, str)` tuple from two path getters over `path` (#1030, D-0219)
/// — the `process.run` aggregate path (intern + StructNew + FieldSet), so
/// codegen needs no path-specific handling. Neither getter raises.
fn build_path_pair(lowerer: &mut FnLower, path: Value, first: &str, second: &str) -> (Value, Ty) {
    let a = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(a),
        func: first.to_string(),
        args: vec![path],
    });
    let b = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(b),
        func: second.to_string(),
        args: vec![path],
    });
    let sid = lowerer
        .ctx
        .tuples
        .borrow_mut()
        .intern(vec![Ty::Ptr, Ty::Ptr]);
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 0,
        val: a,
    });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 1,
        val: b,
    });
    (dest, Ty::Struct(sid))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "getenv" => call_ret(lowerer, "zap_os_getenv", a, Ty::Ptr, false),
        "hasenv" => call_bool(lowerer, "zap_os_hasenv", a),
        "setenv" => call_void(lowerer, "zap_os_setenv", a, true),
        "getpid" => call_ret(lowerer, "zap_os_getpid", a, Ty::I64, false),
        "getcwd" => call_ret(lowerer, "zap_os_getcwd", a, Ty::Ptr, true),
        "chdir" => call_void(lowerer, "zap_os_chdir", a, true),
        "mkdir" => call_void(lowerer, "zap_os_mkdir", a, true),
        "remove" => call_void(lowerer, "zap_os_remove", a, true),
        "rmdir" => call_void(lowerer, "zap_os_rmdir", a, true),
        "listdir" => call_ret(lowerer, "zap_os_listdir", a, Ty::List(Elem::Ptr), true),
        "path_join" => call_ret(lowerer, "zap_os_path_join", a, Ty::Ptr, false),
        "path_exists" => call_bool(lowerer, "zap_os_path_exists", a),
        "path_basename" => call_ret(lowerer, "zap_os_path_basename", a, Ty::Ptr, false),
        "path_dirname" => call_ret(lowerer, "zap_os_path_dirname", a, Ty::Ptr, false),
        // Filesystem gaps (#968, D-0211). environ() builds a dict[str,str]
        // (key + value are plain str reference words -> Elem::Ptr).
        "environ" => call_ret(
            lowerer,
            "zap_os_environ",
            a,
            Ty::Dict(Elem::Ptr, Elem::Ptr),
            false,
        ),
        "sep" => call_ret(lowerer, "zap_os_sep", a, Ty::Ptr, false),
        "path_isabs" => call_bool(lowerer, "zap_os_path_isabs", a),
        "path_expanduser" => call_ret(lowerer, "zap_os_path_expanduser", a, Ty::Ptr, false),
        "path_normpath" => call_ret(lowerer, "zap_os_path_normpath", a, Ty::Ptr, false),
        "path_abspath" => call_ret(lowerer, "zap_os_path_abspath", a, Ty::Ptr, true),
        "path_realpath" => call_ret(lowerer, "zap_os_path_realpath", a, Ty::Ptr, true),
        "path_relpath" => call_ret(lowerer, "zap_os_path_relpath", a, Ty::Ptr, true),
        // Tuple-returning decomposition (#1030, D-0219).
        "path_split" => build_path_pair(
            lowerer,
            a[0],
            "zap_os_path_split_head",
            "zap_os_path_split_tail",
        ),
        "path_splitext" => build_path_pair(
            lowerer,
            a[0],
            "zap_os_path_splitext_root",
            "zap_os_path_splitext_ext",
        ),
        _ => return Ok(None),
    }))
}
