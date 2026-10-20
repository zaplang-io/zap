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

mod argparse;
mod bisect;
mod crypto;
mod datetime;
mod deque;
mod fs;
mod gc;
mod http;
mod io;
mod json;
mod logging;
mod math;
mod net;
mod os;
mod oset;
mod process;
mod random;
mod runtime;
mod signal;
mod sync;
mod sys;
mod time;

/// Declare every built-in module's runtime entry points (ticket #121). Called
/// once from `Cg::translate`. Add a module: create `modules/<name>.rs` with
/// `pub(crate) fn declare_rt(&mut Cg)` and add one call here.
pub(crate) fn declare_runtime(cg: &mut Cg) {
    signal::declare_rt(cg);
    gc::declare_rt(cg);
    runtime::declare_rt(cg);
    os::declare_rt(cg);
    sys::declare_rt(cg);
    io::declare_rt(cg);
    fs::declare_rt(cg);
    process::declare_rt(cg);
    crypto::declare_rt(cg);
    math::declare_rt(cg);
    random::declare_rt(cg);
    time::declare_rt(cg);
    datetime::declare_rt(cg);
    json::declare_rt(cg);
    logging::declare_rt(cg);
    argparse::declare_rt(cg);
    deque::declare_rt(cg);
    bisect::declare_rt(cg);
    oset::declare_rt(cg);
    sync::declare_rt(cg);
    net::declare_rt(cg);
    http::declare_rt(cg);
}
