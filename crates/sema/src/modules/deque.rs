//! `deque` module — sema facet (ticket #102, D-0056).
//!
//! A double-ended queue with O(1) push/pop at both ends and an optional bounded
//! `maxlen` (Python `collections.deque`). Elements are `int64` words for v1
//! (pointer/string element kinds are a documented follow-up). Following the
//! `crypto` streaming pattern, a `deque` value is an opaque handle typed as
//! `bytes`: `deque.new()`/`deque.bounded(n)` create one and the remaining
//! functions operate on it. Native `for x in d` iteration is deferred — use
//! `deque.len(d)` with `deque.get(d, i)`.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "deque",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // deque.new() -> handle (unbounded).
        "new" => {
            expect_argc(checker, "deque", "new", args, 0, span);
            Some(Ty::Bytes)
        }
        // deque.bounded(maxlen) -> handle (fixed ring; overflow drops the far end).
        "bounded" => {
            expect_argc(checker, "deque", "bounded", args, 1, span);
            Some(Ty::Bytes)
        }
        // deque.append(d, x) / deque.appendleft(d, x): push a value, no result.
        "append" | "appendleft" => {
            expect_argc(checker, "deque", method, args, 2, span);
            Some(Ty::Unit)
        }
        // deque.pop(d) / deque.popleft(d) -> int64 (aborts if empty, ticket #22).
        "pop" | "popleft" => {
            expect_argc(checker, "deque", method, args, 1, span);
            Some(Ty::Int64)
        }
        // deque.len(d) -> int64.
        "len" => {
            expect_argc(checker, "deque", "len", args, 1, span);
            Some(Ty::Int64)
        }
        // deque.get(d, i) -> int64 (index from the left, 0 == leftmost).
        "get" => {
            expect_argc(checker, "deque", "get", args, 2, span);
            Some(Ty::Int64)
        }
        // deque.clear(d): drop all elements, no result.
        "clear" => {
            expect_argc(checker, "deque", "clear", args, 1, span);
            Some(Ty::Unit)
        }
        _ => None,
    }
}
