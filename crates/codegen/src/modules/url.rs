//! `url` module — runtime-symbol declarations (ticket #335, D-0056; parse/build
//! #970, D-0209).
//!
//! The `encoding/url` percent codec lives in the url section of `zaprt.c`. Every
//! entry point takes and returns pointers: the codec functions take a `str` and
//! return a `str`; the query builders take a `dict[str,str]` and return a `str`.
//!
//! The parse/build half (#970) adds: the eight `UrlParts` field getters that
//! `url.split` composes (each `str -> str`, except `has_authority`/`port` which
//! return an `int64`), the `url.build` reassembler (eight component args ->
//! `str`), the query-string readers (`str -> dict`), the repeated-key encoder
//! (`dict[str,list[str]] -> str`) and the RFC-3986 reference resolver
//! `url.join` (`(str, str) -> str`).

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let p = cg.ctx.ptr_type(AddressSpace::default());
    let i64t = cg.ctx.i64_type();
    // Byte-level percent codec: str -> str.
    cg.declare_rt("zap_url_quote", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_quote_path", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_unquote", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_unquote_plus", &[p.into()], Some(p.into()));
    // Query builders: dict[str,str] -> str.
    cg.declare_rt("zap_url_encode_query", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_canonical_query", &[p.into()], Some(p.into()));

    // `UrlParts` field getters (url.split): str -> str, except the two scalar
    // fields which return int64 (has_authority: 0/1; port: value or -1).
    cg.declare_rt("zap_url_scheme", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_has_authority", &[p.into()], Some(i64t.into()));
    cg.declare_rt("zap_url_userinfo", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_host", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_port", &[p.into()], Some(i64t.into()));
    cg.declare_rt("zap_url_path", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_query", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_fragment", &[p.into()], Some(p.into()));
    // url.build: (scheme, has_authority, userinfo, host, port, path, query,
    // fragment) -> str.
    cg.declare_rt(
        "zap_url_build",
        &[
            p.into(),
            i64t.into(),
            p.into(),
            p.into(),
            i64t.into(),
            p.into(),
            p.into(),
            p.into(),
        ],
        Some(p.into()),
    );
    // Query readers: str -> dict (parse_query: dict[str,list[str]];
    // parse_query_first: dict[str,str]).
    cg.declare_rt("zap_url_parse_query", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_url_parse_query_first", &[p.into()], Some(p.into()));
    // Repeated-key encoder: dict[str,list[str]] -> str.
    cg.declare_rt("zap_url_encode_query_multi", &[p.into()], Some(p.into()));
    // Reference resolver: (base, ref) -> str.
    cg.declare_rt("zap_url_join", &[p.into(), p.into()], Some(p.into()));
}
