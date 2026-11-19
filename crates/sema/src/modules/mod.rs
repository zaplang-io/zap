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

use zapc_diag::ErrorCode;
use zapc_syntax::ast::{Arg, ArgSpread, Expr};
use zapc_syntax::seam_sigs::{seam_param_default, SeamDefault};
use zapc_syntax::span::Span;

use crate::{Checker, Ty};

/// Synthesize the argument a seam parameter's [`SeamDefault`] stands for, used
/// to fill an *interior* optional hole (#986) so the per-method checker, which
/// reads positionally, still finds the slot. The synthesized expression types
/// and lowers like the literal a caller would have written.
fn seam_default_arg(def: SeamDefault, span: Span) -> Arg {
    let value = match def {
        // `{}` — an empty dict display; infers to `dict[?, ?]` and lowers to an
        // empty dict (the runtime reads it as "no caller headers").
        SeamDefault::EmptyDict => Expr::Dict(Vec::new(), span),
        // A float literal (its raw lexeme), e.g. `math.isclose`'s `rel_tol=1e-9`.
        SeamDefault::Float(lexeme) => Expr::Float(lexeme.to_string(), span),
        // An int literal (its raw lexeme), e.g. `statistics.quantiles`'s `n=4`.
        SeamDefault::Int(lexeme) => Expr::Int(lexeme.to_string(), span),
        // A plain string literal, e.g. `process.run`'s `cwd=""` / `input=""`.
        SeamDefault::Str(s) => Expr::Str {
            value: s.to_string(),
            raw: false,
            bytes: false,
            fstring: false,
            span,
            fstring_fields: Vec::new(),
        },
        // A bool literal, e.g. `process.run`'s `capture_output=True`.
        SeamDefault::Bool(b) => Expr::Bool(b, span),
    };
    Arg {
        name: None,
        value,
        spread: ArgSpread::None,
        span,
    }
}

mod arch;
mod argparse;
mod base64;
mod bisect;
mod concurrent;
mod crypto;
mod datetime;
mod decimal;
mod fft;
mod fractions;
mod fs;
mod gc;
mod gzip;
mod hash;
mod heap;
mod hex;
mod http;
mod io;
mod json;
mod linalg;
mod logging;
mod math;
mod net;
mod os;
mod process;
mod random;
mod runtime;
mod signal;
mod statistics;
mod sync;
mod sys;
mod threading;
mod time;
mod tls;
mod url;
mod uuid;
mod xml;
mod yaml;
mod zlib;

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
    base64::MODULE,
    hex::MODULE,
    hash::MODULE,
    math::MODULE,
    statistics::MODULE,
    decimal::MODULE,
    fractions::MODULE,
    fft::MODULE,
    linalg::MODULE,
    random::MODULE,
    time::MODULE,
    datetime::MODULE,
    json::MODULE,
    yaml::MODULE,
    logging::MODULE,
    argparse::MODULE,
    bisect::MODULE,
    heap::MODULE,
    zlib::MODULE,
    gzip::MODULE,
    sync::MODULE,
    threading::MODULE,
    net::MODULE,
    http::MODULE,
    tls::MODULE,
    url::MODULE,
    uuid::MODULE,
    concurrent::MODULE,
    xml::MODULE,
    // Architecture-specific low-level intrinsics (I#139 Tier 2, D-0199). Dotted
    // registry names; dispatched via the import-alias map (see `Checker`).
    arch::MODULE_X86_64,
    arch::MODULE_AARCH64,
    // The portable `arch` facade: the CPU-feature query `arch.cpu_has` (#963,
    // D-0200). Compiles on every target (unlike the arch sub-modules).
    arch::MODULE_FACADE,
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
pub(crate) const CONST_MODULES: &[ModuleConst] = &[math::MODULE_CONST, logging::MODULE_CONST];

/// Whether `name` is a compiler-known built-in (stdlib) module — one registered
/// in [`MODULES`] or exposing constants in [`CONST_MODULES`] (D-0056). Zap-to-Zap
/// module resolution (I#283/D-0081) consults this so an `import <name>` whose
/// leading segment is a stdlib module is left to the existing built-in handling
/// instead of being resolved to a user file.
pub fn is_builtin_module(name: &str) -> bool {
    MODULES.iter().any(|m| m.name == name) || CONST_MODULES.iter().any(|m| m.name == name)
}

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
    // Seam calling convention (#998, D-0204): if this method has a declared
    // signature in the registry, bind keyword arguments by name (reorder, fill
    // omitted trailing optionals, reject an unknown keyword with ZAP4023) and
    // hand the per-method checker the arguments normalized to positional order.
    // A method with no row keeps the legacy positional-only behavior.
    let bound;
    let args = match zapc_syntax::seam_sigs::seam_method_params(module, method) {
        Some(params) => {
            bound = bind_seam_args(checker, module, method, params, args, span);
            &bound[..]
        }
        None => args,
    };
    Some(
        (desc.check)(checker, method, args, span).unwrap_or_else(|| {
            checker.error(
                ErrorCode::ZAP3003,
                format!("module '{module}' has no function '{method}'"),
                span,
            );
            Ty::Unknown
        }),
    )
}

/// Bind a seam call's arguments to a declared positional parameter list so a
/// built-in module function can be called with **keyword arguments**, in any
/// order, with omitted trailing optionals (#998). This is the opt-in half of
/// the seam calling convention: a per-method checker that wants Python-shaped
/// keyword/default binding calls this first with its parameter names (in
/// declared order) and then type-checks the returned, normalized-to-positional
/// argument list exactly as before — `args[0]`, `args.get(1)`, the arity check,
/// all unchanged. A method that does not call this keeps the legacy
/// positional-only behavior (names ignored), so migration is incremental and
/// risk-free.
///
/// Binding rules mirror the user-function convention (and Python): positionals
/// fill parameters left to right, each keyword binds to the parameter of that
/// name, and the result is the supplied arguments in declared order with their
/// `name`s stripped (so downstream positional reads just work) and any omitted
/// trailing parameters simply absent. Diagnostics mirror the user-function
/// path — an unknown keyword is the same `ZAP4023` a user function emits.
///
/// On any binding error the original `args` are returned unchanged (after the
/// diagnostic) so the per-method checker still infers every argument and no
/// cascade of confusing follow-on errors appears. Spread arguments (`*`/`**`,
/// #597) are left entirely to the legacy path: if any argument is a spread, the
/// original list is returned untouched.
pub(super) fn bind_seam_args(
    checker: &mut Checker,
    module: &str,
    method: &str,
    params: &[&str],
    args: &[Arg],
    span: Span,
) -> Vec<Arg> {
    use zapc_syntax::ast::ArgSpread;

    // Nothing to reorder, or a spread is present → legacy positional path.
    if args.iter().all(|a| a.name.is_none())
        || args.iter().any(|a| !matches!(a.spread, ArgSpread::None))
    {
        return args.to_vec();
    }

    let mut slots: Vec<Option<Arg>> = vec![None; params.len()];
    // Positionals beyond the declared arity: keep them so the per-method arity
    // check still fires its own `ZAP4025`/`ZAP4020`, rather than hiding the
    // overflow here.
    let mut overflow: Vec<Arg> = Vec::new();
    let mut next_pos = 0usize;
    let mut seen_keyword = false;

    for a in args {
        match &a.name {
            None => {
                if seen_keyword {
                    checker.error(
                        ErrorCode::ZAP4023,
                        format!(
                            "positional argument follows keyword argument in the call to \
                             '{module}.{method}'"
                        ),
                        a.span,
                    );
                    return args.to_vec();
                }
                if next_pos < slots.len() {
                    slots[next_pos] = Some(a.clone());
                } else {
                    overflow.push(a.clone());
                }
                next_pos += 1;
            }
            Some(name) => {
                seen_keyword = true;
                match params.iter().position(|p| p == name) {
                    None => {
                        checker.error(
                            ErrorCode::ZAP4023,
                            format!("'{module}.{method}' has no parameter named '{name}'"),
                            a.span,
                        );
                        return args.to_vec();
                    }
                    Some(idx) => {
                        if slots[idx].is_some() {
                            checker.error(
                                ErrorCode::ZAP4023,
                                format!(
                                    "'{module}.{method}' got multiple values for parameter \
                                     '{name}'"
                                ),
                                a.span,
                            );
                            return args.to_vec();
                        }
                        let mut bound = a.clone();
                        bound.name = None; // normalize to positional for the checker
                        slots[idx] = Some(bound);
                    }
                }
            }
        }
    }

    // Flatten declared order. A filled parameter after an omitted earlier one is
    // a hole (e.g. `compress(level=9)` with `data` missing): reject it rather
    // than silently shifting `level` into the `data` slot (the old footgun) —
    // UNLESS the omitted parameter carries a `SeamDefault`, in which case the
    // hole is an *interior optional* we fill with its default (#986), so e.g.
    // `http.get(url, timeout=5000)` binds with `headers` defaulted to `{}`.
    // A trailing omitted parameter (nothing filled after it) stays simply absent.
    let last_filled = slots.iter().rposition(|s| s.is_some());
    let mut out: Vec<Arg> = Vec::new();
    for (i, slot) in slots.into_iter().enumerate() {
        match slot {
            Some(a) => out.push(a),
            None => {
                let interior = last_filled.is_some_and(|lf| i < lf);
                if interior {
                    match seam_param_default(module, method, params[i]) {
                        Some(def) => out.push(seam_default_arg(def, span)),
                        None => {
                            let later = last_filled.map(|lf| params[lf]).unwrap_or("");
                            checker.error(
                                ErrorCode::ZAP4025,
                                format!(
                                    "'{module}.{method}' is missing a value for parameter '{}' \
                                     (supplied later parameter '{later}')",
                                    params[i]
                                ),
                                span,
                            );
                            return args.to_vec();
                        }
                    }
                }
            }
        }
    }
    out.extend(overflow);
    out
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
            ErrorCode::ZAP4025,
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

/// Like [`expect_argc`] but admits an argument count in `[min, max]` (a method
/// with an optional trailing argument, e.g. `linalg.norm(a[, ord])`). Still
/// type-checks (infers) every supplied argument.
pub(super) fn expect_argc_range(
    checker: &mut Checker,
    module: &str,
    method: &str,
    args: &[Arg],
    min: usize,
    max: usize,
    span: Span,
) {
    if args.len() < min || args.len() > max {
        checker.error(
            ErrorCode::ZAP4025,
            format!(
                "{module}.{method}() takes {min} to {max} argument(s), found {}",
                args.len()
            ),
            span,
        );
    }
    for a in args {
        checker.infer(&a.value);
    }
}
