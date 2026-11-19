//! `collections` module — sema facet (ticket #972, epic #966).
//!
//! Python-`collections` conveniences. Every type this module exposes —
//! `collections.Counter[K]`, `collections.defaultdict[K, V]`,
//! `collections.OrderedDict[K, V]` — is a **generic** prelude struct, so each is
//! constructed through the seam-generic path (D-0202): the module-qualified
//! spelling is pure front-end sugar (`zapc_syntax::seam`) rewritten to a bare
//! prelude free constructor (`new_counter` / `new_defaultdict` /
//! `new_ordered_dict`) long before this facet, exactly like `concurrent.Queue`.
//! The backing prelude structs (`CounterMap` / `DefaultDict` / `OrderedMap`)
//! carry all the behaviour as ordinary generic-struct methods (dunder index /
//! membership / len, I#481 keyword/default arguments), so there is **no** runtime
//! C and **no** module-facet method here.
//!
//! This module therefore exposes no flat functions; its sole purpose is to make
//! `import collections` a recognized built-in module (`is_builtin_module`, so
//! the import resolves to the built-in handling rather than a user file,
//! I#283/D-0081). The check hook always returns `None` — any `collections.foo(…)`
//! that is not one of the seam-sugared constructors is an unknown function, the
//! uniform diagnostic the registry already raises.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "collections",
    check,
};

fn check(_checker: &mut Checker, _method: &str, _args: &[Arg], _span: Span) -> Option<Ty> {
    // The Counter / defaultdict / OrderedDict constructors are seam-generic
    // sugar rewritten to bare prelude free functions before dispatch reaches
    // here, so no method is handled at the module facet.
    None
}
