//! Built-in stdlib module registry — lowering facet (ticket #121, D-0056/D-0058).
//!
//! The lowering twin of `zapc_sema::modules`. Each compiler-known namespaced
//! module (`signal`, `gc`, `runtime`, …) lives in its own file and registers one
//! [`ModuleLower`] describing its flat name and a per-method lowering hook that
//! emits the backing runtime calls. Dispatch is a data table ([`MODULES`]), not a
//! hand-maintained `match (module, method)` block, so a new module is **parallel-
//! addable**: create `lower/modules/<name>.rs` exposing `pub(super) const MODULE:
//! ModuleLower`, then add one entry to [`MODULES`].
//!
//! Sema has already type-checked these calls; lowering just emits ZIR. A module
//! whose name matches but whose method is unknown returns `Ok(None)` (unreachable
//! after a successful sema pass), matching the pre-seam fall-through.

use zapc_syntax::ast;

use super::{FnLower, LowerError};
use crate::ir::*;

mod argparse;
mod crypto;
mod datetime;
mod fs;
mod gc;
mod io;
mod logging;
mod math;
mod os;
mod process;
mod random;
mod runtime;
mod signal;
mod support;
mod sys;
mod time;

/// A module's per-method lowering hook: lower `method(args)` to ZIR, emitting the
/// backing runtime calls. `Ok(Some((value, ty)))` for a method the module
/// provides; `Ok(None)` if the module has no such method (the caller then falls
/// through). Each `modules/<name>.rs` defines one of these.
pub(super) type LowerHook =
    fn(&mut FnLower, method: &str, args: &[ast::Arg]) -> Result<Option<(Value, Ty)>, LowerError>;

/// A built-in module's lowering facet.
pub(super) struct ModuleLower {
    /// The module's flat name as written in source (`gc`, `signal`, …).
    pub name: &'static str,
    /// Lower `name.method(args)` to ZIR.
    pub lower: LowerHook,
}

/// The registered built-in modules (ticket #121). Keep in step with
/// `zapc_sema::modules::MODULES`. Add a module: create `lower/modules/<name>.rs`
/// with `pub(super) const MODULE: ModuleLower` and append it here.
pub(super) const MODULES: &[ModuleLower] = &[
    signal::MODULE,
    gc::MODULE,
    runtime::MODULE,
    os::MODULE,
    sys::MODULE,
    io::MODULE,
    fs::MODULE,
    process::MODULE,
    crypto::MODULE,
    math::MODULE,
    random::MODULE,
    time::MODULE,
    datetime::MODULE,
    logging::MODULE,
    argparse::MODULE,
];

/// A module's *constant* lowering hook: lower `module.CONST` (an attribute, not
/// a call) to a ZIR value, e.g. `math.pi` -> an `f64` `FConst`. An **additive**
/// sub-registry over [`MODULES`] — only constant-bearing modules appear, so the
/// common `ModuleLower` shape is unchanged (ticket #98).
pub(super) struct ModuleConst {
    /// The module's flat name as written in source (`math`, …).
    pub name: &'static str,
    /// Lower `name.<const>`, or `None` if the module has no such constant.
    pub lower: fn(&mut FnLower, &str) -> Option<(Value, Ty)>,
}

/// The registered modules that expose compile-time constants (ticket #98). Keep
/// in step with `zapc_sema::modules::CONST_MODULES`.
pub(super) const CONST_MODULES: &[ModuleConst] = &[math::MODULE_CONST, logging::MODULE_CONST];

/// Lower a built-in `signal`/`gc`/`runtime`-style module call, or return
/// `Ok(None)` if `module` is not a registered built-in module (so the caller
/// falls through to ordinary resolution). See `zapc_sema::modules` for the type-
/// checking twin (ticket #77, D-0058).
pub(super) fn lower_builtin_module_call(
    lowerer: &mut FnLower,
    module: &str,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    match MODULES.iter().find(|m| m.name == module) {
        Some(desc) => (desc.lower)(lowerer, method, args),
        None => Ok(None),
    }
}

/// Lower a built-in `module.CONST` attribute access (e.g. `math.pi`), or return
/// `None` if `module` is not a registered constant-bearing module or has no such
/// constant (so the caller falls through to ordinary attribute resolution). See
/// `zapc_sema::modules::check_builtin_module_const` for the type-checking twin
/// (ticket #98).
pub(super) fn lower_builtin_module_const(
    lowerer: &mut FnLower,
    module: &str,
    name: &str,
) -> Option<(Value, Ty)> {
    let desc = CONST_MODULES.iter().find(|m| m.name == module)?;
    (desc.lower)(lowerer, name)
}

/// Shared helper: emit a zero-argument runtime call. A `Ty::Void` return models a
/// statement-position call (no result), yielding a throwaway `i32 0` so the call
/// expression still has a value; any other `ret` yields a fresh typed value.
/// Mirrors the pre-seam `call0` closure.
pub(super) fn call0(lowerer: &mut FnLower, func: &str, ret: Ty) -> (Value, Ty) {
    if ret == Ty::Void {
        lowerer.emit(Inst::Call {
            dest: None,
            func: func.to_string(),
            args: vec![],
        });
        (lowerer.iconst(Ty::I32, 0), Ty::I32)
    } else {
        let d = lowerer.new_value(ret);
        lowerer.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args: vec![],
        });
        (d, ret)
    }
}
