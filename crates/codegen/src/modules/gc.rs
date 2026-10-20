//! `gc` module — runtime-symbol declarations (ticket #77, D-0058).
//!
//! The `gc` module entry points over the collector: `zap_gc_collect()` forces a
//! collection; `zap_gc_stats(heap, live)` and the `zap_gc_*_bytes`/`zap_gc_count`
//! getters back `gc.stats()`'s `MemStats`. `zap_gc_finalizer_selftest` is a
//! finalizer integration-test hook (ticket #92, gc-immix): it exercises the
//! precise collector's register + post-mark finalizer queue and returns the
//! number of finalizers that ran. See `zaprt.c` / `zapgc.inc.c`.

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(inkwell::AddressSpace::default());
    cg.declare_rt("zap_gc_collect", &[], None);
    cg.declare_rt("zap_gc_stats", &[p.into(), p.into()], None);
    cg.declare_rt("zap_gc_count", &[], Some(i.into()));
    cg.declare_rt("zap_gc_heap_bytes", &[], Some(i.into()));
    cg.declare_rt("zap_gc_live_bytes", &[], Some(i.into()));
    cg.declare_rt("zap_gc_free_bytes", &[], Some(i.into()));
    cg.declare_rt("zap_gc_finalizer_selftest", &[], Some(i.into()));
}
