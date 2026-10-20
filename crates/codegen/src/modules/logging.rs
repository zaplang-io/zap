//! `logging` module — runtime-symbol declarations (ticket #101, D-0056).
//!
//! Leveled logging over a raw fd (default stderr). `str` flows as `ptr` (a
//! `ZapStr*`); the level is an `int64`. The level thresholds (`logging.DEBUG`,
//! …) are constants emitted inline by the lowerer, so they declare no runtime
//! symbol. See the "logging / argparse" section of `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_logging_log", &[i.into(), p.into()], None);
    cg.declare_rt("zap_logging_set_level", &[i.into()], None);
    cg.declare_rt("zap_logging_get_level", &[], Some(i.into()));
    cg.declare_rt("zap_logging_set_stream", &[i.into()], None);
    cg.declare_rt("zap_logging_set_format", &[p.into()], None);
}
