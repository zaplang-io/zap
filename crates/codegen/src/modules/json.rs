//! `json` module — runtime-symbol declarations (ticket #100/#131, D-0059).
//!
//! The JSON tokenizer cursor lives in the json section of `zaprt.c`; the
//! high-level tree build (`json.loads`) is a Zap prelude function, so only the
//! `zap_json_lex_*` cursor primitives are runtime symbols here. All
//! `str`/`bytes`/handle values are pointers; `_lex_float` returns an `f64`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let f = cg.ctx.f64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // Cursor: open over a str/bytes source, advance, read the current scalar.
    cg.declare_rt("zap_json_lex_new", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_json_lex_next", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_json_lex_int", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_json_lex_float", &[p.into()], Some(f.into()));
    cg.declare_rt("zap_json_lex_str", &[p.into()], Some(p.into()));
    // dumps helpers (object keys, string escaping, fragment join). Number
    // formatting reuses the core zap_i64_to_str / zap_f64_to_str declarations.
    cg.declare_rt("zap_dict_keys", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_json_escape", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_json_join", &[p.into()], Some(p.into()));
    // @derive(Json) `to_json` codec: JSON boolean literal.
    cg.declare_rt("zap_json_bool_str", &[i.into()], Some(p.into()));
}
