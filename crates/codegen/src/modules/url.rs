//! `url` module — runtime-symbol declarations (ticket #335, D-0056).
//!
//! The `encoding/url` percent codec lives in the url section of `zaprt.c`. Every
//! entry point takes and returns pointers: the codec functions take a `str` and
//! return a `str`; the query builders take a `dict[str,str]` and return a `str`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // Byte-level percent codec: str -> str.
    cg.declare_rt("zap_url_quote", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_quote_path", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_unquote", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_unquote_plus", &[p.into()], Some(p.into()));
    // Query builders: dict[str,str] -> str.
    cg.declare_rt("zap_url_encode_query", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_canonical_query", &[p.into()], Some(p.into()));
}
