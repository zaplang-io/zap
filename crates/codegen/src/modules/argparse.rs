//! `argparse` module — runtime-symbol declarations (ticket #101, D-0056).
//!
//! CLI parsing over the global singleton parser. `str` flows as `ptr` (a
//! `ZapStr*`), the `add_int` default and `get_int` result as `int64`, the argv
//! `list[str]` as `ptr` (a `ZapList*`). Booleans (`get_flag`/`help_requested`)
//! flow as `int64` (`!= 0`), normalized to `i1` by the lowerer. See the
//! "logging / argparse" section of `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_argparse_set_prog", &[p.into()], None);
    cg.declare_rt("zap_argparse_set_description", &[p.into()], None);
    cg.declare_rt("zap_argparse_add_flag", &[p.into()], None);
    cg.declare_rt("zap_argparse_add_positional", &[p.into()], None);
    cg.declare_rt("zap_argparse_add_option", &[p.into(), p.into()], None);
    cg.declare_rt("zap_argparse_add_int", &[p.into(), i.into()], None);
    cg.declare_rt("zap_argparse_parse", &[p.into()], None);
    cg.declare_rt("zap_argparse_get", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_argparse_get_int", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_argparse_get_flag", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_argparse_format_help", &[], Some(p.into()));
    cg.declare_rt("zap_argparse_help_requested", &[], Some(i.into()));
    cg.declare_rt("zap_argparse_reset", &[], None);
}
