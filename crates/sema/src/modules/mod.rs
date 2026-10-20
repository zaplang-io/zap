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
mod gc;
mod runtime;
mod signal;
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
    crypto::MODULE,
    time::MODULE,
    datetime::MODULE,
];

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
