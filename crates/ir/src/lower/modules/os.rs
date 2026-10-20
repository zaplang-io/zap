//! `os` module — lowering facet (ticket #97, D-0056).

use zapc_syntax::ast;

use super::support::{call_bool, call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "os", lower };

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
        _ => return Ok(None),
    }))
}
