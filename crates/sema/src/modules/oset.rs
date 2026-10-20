//! `oset` module — sema facet (ticket #102, D-0056).
//!
//! An insertion-ordered set of `int64` values: iteration (via `len`+`get`)
//! visits elements in the order they were first added, which the built-in
//! `set` (an open-addressing hash table) does not guarantee. Backed by an
//! existing `list` (order) plus `set` (membership). Like `deque`, the value is
//! an opaque handle typed as `bytes`. `discard`/removal is a documented
//! follow-up (the built-in set has no remove, and removal from an ordered store
//! needs compaction). Element types other than `int64` are also a follow-up.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "oset",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // oset.new() -> handle.
        "new" => {
            expect_argc(checker, "oset", "new", args, 0, span);
            Some(Ty::Bytes)
        }
        // oset.add(s, x): add a value if absent (first-add order preserved), no result.
        "add" => {
            expect_argc(checker, "oset", "add", args, 2, span);
            Some(Ty::Unit)
        }
        // oset.contains(s, x) -> bool.
        "contains" => {
            expect_argc(checker, "oset", "contains", args, 2, span);
            Some(Ty::Bool)
        }
        // oset.len(s) -> int64.
        "len" => {
            expect_argc(checker, "oset", "len", args, 1, span);
            Some(Ty::Int64)
        }
        // oset.get(s, i) -> int64 (the i-th element in insertion order).
        "get" => {
            expect_argc(checker, "oset", "get", args, 2, span);
            Some(Ty::Int64)
        }
        _ => None,
    }
}
