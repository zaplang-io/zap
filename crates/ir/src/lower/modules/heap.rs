//! `heap` module — lowering facet (ticket #978, D-0236).
//!
//! The `heap[T]` *type* — construction, O(n) heapify via `heap[T](xs)`, and the
//! `push`/`pop`/`peek`/`replace`/`pushpop` methods — is lowered in `crate::lower`
//! directly (it is a built-in generic collection, not a seam module). This file
//! hosts only the two free FUNCTIONS that take an iterable and return a fresh
//! `list[T]`: `heap.nlargest` / `heap.nsmallest`, each with an optional `key=`
//! selector. This mirrors Python, where the top-K helpers live in the `heapq`
//! module rather than on a heap object; the actual loop (which evaluates `key=`
//! per element) is built by [`FnLower::lower_heap_topk`].

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "heap",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // `dir` selects the heap direction and thus the result order: nlargest uses
    // a max-heap (largest first), nsmallest a min-heap (smallest first).
    let dir = match method {
        "nlargest" => 1,
        "nsmallest" => 0,
        _ => return Ok(None),
    };
    Ok(Some(lowerer.lower_heap_topk(dir, method, args)?))
}
