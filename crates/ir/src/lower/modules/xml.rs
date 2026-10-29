//! `xml` module — lowering facet (ticket #351, D-0083).
//!
//! `xml.parse(s)` / `xml.dumps(e)` / `xml.element(name)` lower to the Zap prelude
//! builders `__xml_parse` / `__xml_dumps` / `__xml_element` (which construct and
//! walk the `XmlElement` tree via ordinary struct/list/dict ops, keeping layout
//! knowledge out of the runtime). The internal `xml._lex_*` cursor primitives and
//! the serializer helpers (`_esc_text`/`_esc_attr`/`_join`) lower to the
//! `zap_xml_*` C runtime in `zaprt.c`. `str`/`bytes`/cursor-handle/list values
//! are all pointers.

use zapc_syntax::ast;

use super::support::{call_ret, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "xml", lower };

/// Lower a call whose result is the compiler-known `XmlElement` struct, built by
/// a prelude function (`__xml_parse` / `__xml_element`).
fn call_xml_element(
    lowerer: &mut FnLower,
    func: &str,
    args: Vec<Value>,
) -> Result<(Value, Ty), LowerError> {
    let sid = lowerer
        .ctx
        .ids
        .get("XmlElement")
        .copied()
        .ok_or_else(|| err("XmlElement unavailable (built-in prelude missing)"))?;
    let d = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: func.to_string(),
        args,
    });
    Ok((d, Ty::Struct(sid)))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // xml.parse(s) -> XmlElement / xml.element(name) -> XmlElement: call the
    // prelude builders, whose result type is the compiler-known `XmlElement`.
    if method == "parse" {
        let (s, _) = lowerer.expr(&args[0].value)?;
        return Ok(Some(call_xml_element(lowerer, "__xml_parse", vec![s])?));
    }
    if method == "element" {
        let (s, _) = lowerer.expr(&args[0].value)?;
        return Ok(Some(call_xml_element(lowerer, "__xml_element", vec![s])?));
    }
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        // xml.dumps(e) -> str: call the prelude serializer.
        "dumps" => call_ret(lowerer, "__xml_dumps", a, Ty::Ptr, false),
        // Tokenizer cursor primitives.
        "_lex_new" => call_ret(lowerer, "zap_xml_lex_new", a, Ty::Ptr, false),
        "_lex_next" => call_ret(lowerer, "zap_xml_lex_next", a, Ty::I64, false),
        "_lex_name" => call_ret(lowerer, "zap_xml_lex_name", a, Ty::Ptr, false),
        "_lex_value" => call_ret(lowerer, "zap_xml_lex_value", a, Ty::Ptr, false),
        // Serializer helpers (text/attr escaping, fragment join).
        "_esc_text" => call_ret(lowerer, "zap_xml_esc_text", a, Ty::Ptr, false),
        "_esc_attr" => call_ret(lowerer, "zap_xml_esc_attr", a, Ty::Ptr, false),
        "_join" => call_ret(lowerer, "zap_json_join", a, Ty::Ptr, false),
        _ => return Ok(None),
    }))
}
