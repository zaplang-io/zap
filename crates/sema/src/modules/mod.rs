//! Built-in stdlib module registry — sema facet (ticket #121, D-0056/D-0058).
//!
//! Each compiler-known namespaced module (`signal`, `gc`, `runtime`, …) lives in
//! its own file and registers one [`ModuleCheck`] describing its flat name and a
//! per-method type-checker. Dispatch is a data table ([`MODULES`]), not a hand-
//! maintained `match (module, method)` block, so a new module is **parallel-
//! addable**: create `modules/<name>.rs` exposing `pub(super) const MODULE:
//! ModuleCheck`, then add one entry to [`MODULES`] — no edits to the giant
//! `lib.rs` surface that every module used to share.
//!
//! The twin of this registry on the lowering side is `zapc_ir::lower::modules`;
//! the runtime-symbol twin is `zapc_codegen::modules`. Behaviour is identical to
//! the pre-seam twin `match` blocks.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use crate::{Checker, Ty};

mod crypto;
mod datetime;
mod fs;
mod gc;
mod io;
mod json;
mod math;
mod os;
mod process;
mod random;
mod runtime;
mod signal;
mod sys;
mod time;

/// A built-in module's type-checking facet.
pub(crate) struct ModuleCheck {
    /// The module's flat name as written in source (`gc`, `signal`, …).
    pub name: &'static str,
    /// Type-check `name.method(args)`. Returns the result type for a method the
    /// module provides, or `None` if the module has no such method (the registry
    /// then raises the uniform "no function" diagnostic).
    pub check: fn(&mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty>,
}

/// The registered built-in modules (ticket #121).
///
/// Add a module: create `modules/<name>.rs` with `pub(super) const MODULE:
/// ModuleCheck` and append it here. Order is irrelevant (names are unique).
pub(crate) const MODULES: &[ModuleCheck] = &[
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
    json::MODULE,
];

/// A built-in module's *constant* facet: the type of a `module.CONST` attribute
/// access (not a call), e.g. `math.pi : float64`. This is an **additive**
/// sub-registry over [`MODULES`] — only modules that expose constants appear
/// here, so the common `ModuleCheck` shape is unchanged (ticket #98).
pub(crate) struct ModuleConst {
    /// The module's flat name as written in source (`math`, …).
    pub name: &'static str,
    /// Type of `name.<const>`, or `None` if the module has no such constant.
    pub const_ty: fn(&str) -> Option<Ty>,
}

/// The registered modules that expose compile-time constants (ticket #98). Add
/// a module: expose `pub(super) const MODULE_CONST: ModuleConst` and append it.
pub(crate) const CONST_MODULES: &[ModuleConst] = &[math::MODULE_CONST];

/// Type-check a built-in `signal`/`gc`/`runtime`-style module call, returning the
/// result type, or `None` if `module` is not a registered built-in module (so the
/// caller falls through to ordinary resolution). These modules are compiler-known
/// surface over the runtime diagnostics entry points; `import signal`/`gc`/
/// `runtime` is accepted but not required (ticket #77, D-0058).
pub(crate) fn check_builtin_module_call(
    checker: &mut Checker,
    module: &str,
    method: &str,
    args: &[Arg],
    span: Span,
) -> Option<Ty> {
    let desc = MODULES.iter().find(|m| m.name == module)?;
    Some(
        (desc.check)(checker, method, args, span).unwrap_or_else(|| {
            checker.error(
                format!("module '{module}' has no function '{method}'"),
                span,
            );
            Ty::Unknown
        }),
    )
}

/// Type a built-in `module.CONST` attribute access (e.g. `math.pi`), or return
/// `None` if `module` is not a registered constant-bearing module or has no such
/// constant (so the caller falls through to ordinary attribute resolution).
/// Like the call registry, these module names are not value bindings; the caller
/// only consults this when `module` is not shadowed by a local (ticket #98).
pub(crate) fn check_builtin_module_const(module: &str, name: &str) -> Option<Ty> {
    let desc = CONST_MODULES.iter().find(|m| m.name == module)?;
    (desc.const_ty)(name)
}

/// Shared helper: require exactly `n` arguments for `module.method`, emitting the
/// uniform arity diagnostic on mismatch, and infer every argument (so argument
/// expressions are still type-checked regardless of arity). Mirrors the pre-seam
/// `expect_argc` closure.
pub(super) fn expect_argc(
    checker: &mut Checker,
    module: &str,
    method: &str,
    args: &[Arg],
    n: usize,
    span: Span,
) {
    if args.len() != n {
        checker.error(
            format!(
                "{module}.{method}() takes {n} argument(s), found {}",
                args.len()
            ),
            span,
        );
    }
    for a in args {
        checker.infer(&a.value);
    }
}
