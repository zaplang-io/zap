//! Built-in stdlib module registry — runtime-symbol facet (ticket #121).
//!
//! The codegen twin of `zapc_sema::modules` / `zapc_ir::lower::modules`. Each
//! compiler-known namespaced module (`signal`, `gc`, `runtime`, …) declares its
//! backing C runtime entry points (`zap_*`) in its own file via [`Cg::declare_rt`]
//! instead of a single hand-maintained cluster in `lib.rs`, so a new module's
//! runtime surface is **parallel-addable**: create `modules/<name>.rs` exposing
//! `pub(crate) fn declare_rt(&mut Cg)`, then add one call to [`declare_runtime`].
//!
//! The C implementations live in per-module sections of `zaprt.c` (and, for the
//! precise collector, `zapgc.inc.c`). Declaration order is irrelevant — the
//! symbols land in a map — so this is strictly behaviour-preserving.

use super::Cg;

mod gc;
mod runtime;
mod signal;

/// Declare every built-in module's runtime entry points (ticket #121). Called
/// once from `Cg::translate`. Add a module: create `modules/<name>.rs` with
/// `pub(crate) fn declare_rt(&mut Cg)` and add one call here.
pub(crate) fn declare_runtime(cg: &mut Cg) {
    signal::declare_rt(cg);
    gc::declare_rt(cg);
    runtime::declare_rt(cg);
}
