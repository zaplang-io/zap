//! `xml` module — runtime-symbol declarations (ticket #351, D-0085).
//!
//! The XML tokenizer cursor lives in the xml section of `zaprt.c`; the high-level
//! tree build (`xml.parse`) and serialize walk (`xml.dumps`) are Zap prelude
//! functions, so only the `zap_xml_lex_*` cursor primitives and the `_esc_*`
//! helpers are runtime symbols here. The fragment join reuses `zap_json_join`
//! (declared by the json module). All `str`/`bytes`/handle values are pointers.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // Cursor: open over a str/bytes source, advance, read the current token's
    // name/value.
    cg.declare_rt("zap_xml_lex_new", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_xml_lex_next", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_xml_lex_name", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_xml_lex_value", &[p.into()], Some(p.into()));
    // Serializer helpers: text / attribute-value escaping (the join reuses
    // zap_json_join).
    cg.declare_rt("zap_xml_esc_text", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_xml_esc_attr", &[p.into()], Some(p.into()));
}
