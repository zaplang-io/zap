//! `concurrent` module — lowering facet (ticket #184, D-0078).
//!
//! `concurrent.Counter(start?)` builds the prelude `struct Counter` (a lock-free
//! counter over `Atomic[int64]`). Rather than assemble the nested
//! `Counter { _cell: Atomic[int64] }` here, the hook emits a call to the prelude
//! constructor `__concurrent_counter(start)` (see `crates/syntax/src/parser.rs`),
//! which does the construction in plain Zap over the `Atomic` intrinsics — so
//! this hook stays a thin shim and the monomorphizer handles the inner
//! `Atomic[int64]` instance as usual. `start` defaults to 0.
//!
//! The MPSC `Queue[T]` is generic and therefore NOT module-qualified: it is a
//! bare prelude type (like `Atomic[T]`/`Optional[T]`) built with the prelude
//! `new_queue[T]()` free function, so it needs no entry here.
//!
//! No runtime-C / GC code backs any of this — the types are pure Zap over the
//! existing `Atomic[T]` lowering (the central finding of the design, §4).

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "concurrent",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    if method == "Counter" {
        let sid = lowerer
            .ctx
            .ids
            .get("Counter")
            .copied()
            .ok_or_else(|| err("Counter unavailable (built-in prelude missing)"))?;
        // The initial value: the lone optional argument, or 0.
        let start = match args.first() {
            Some(a) => {
                let (v, vt) = lowerer.expr(&a.value)?;
                lowerer.coerce(v, vt, Ty::I64)
            }
            None => {
                let d = lowerer.new_value(Ty::I64);
                lowerer.emit(Inst::IConst { dest: d, val: 0 });
                d
            }
        };
        let dest = lowerer.new_value(Ty::Struct(sid));
        lowerer.throwing_call(Some(dest), "__concurrent_counter", vec![start]);
        return Ok(Some((dest, Ty::Struct(sid))));
    }
    Ok(None)
}
