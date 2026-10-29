//! `xml` module — sema facet (ticket #351, D-0083).
//!
//! Core XML DOM for the AWS rest-xml / query protocols. The public surface is
//! `xml.parse(str|bytes) -> XmlElement` (the DOM root; `XmlElement`/`XmlAttr` are
//! compiler-known prelude structs), `xml.dumps(XmlElement) -> str` (deterministic
//! serialization) and `xml.element(str) -> XmlElement` (a fresh empty element).
//! The tree build and serialize walk live in Zap prelude functions (see
//! `zapc_syntax::parser::PRELUDE_SRC`) that drive a C tokenizer through the
//! internal `xml._lex_*` cursor primitives plus the `_esc_*`/`_join` helpers, so
//! XML<->XmlElement construction stays in compiler-emitted code and the runtime
//! never depends on the DOM layout (design/0033-xml.md).

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{assignable, expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck { name: "xml", check };

/// Require a `str`/`bytes` argument (both share the `ZapStr` layout); infer it
/// regardless so it is still type-checked.
fn expect_strbytes(checker: &mut Checker, func: &str, arg: &Arg) {
    let t = checker.infer(&arg.value);
    if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
        checker.error(
            ErrorCode::ZAP4024,
            format!(
                "{func}() expects a str or bytes argument, found {}",
                t.display()
            ),
            expr_span(&arg.value),
        );
    }
}

/// One-argument arity check (infers the argument regardless of arity).
fn expect_one(checker: &mut Checker, method: &str, args: &[Arg], span: Span) {
    if args.len() != 1 {
        checker.error(
            ErrorCode::ZAP4025,
            format!("xml.{method}() takes 1 argument, found {}", args.len()),
            span,
        );
    }
    for a in args {
        checker.infer(&a.value);
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // xml.parse(s: str|bytes) -> XmlElement (the DOM root).
        "parse" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!("xml.parse(s) takes 1 argument, found {}", args.len()),
                    span,
                );
            } else {
                expect_strbytes(checker, "parse", &args[0]);
            }
            Some(Ty::Named("XmlElement".to_string()))
        }
        // xml.dumps(e: XmlElement) -> str (deterministic serialization).
        "dumps" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!("xml.dumps(e) takes 1 argument, found {}", args.len()),
                    span,
                );
            } else {
                let t = checker.infer(&args[0].value);
                if !assignable(&t, &Ty::Named("XmlElement".to_string())) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!("xml.dumps() expects an XmlElement, found {}", t.display()),
                        expr_span(&args[0].value),
                    );
                }
            }
            Some(Ty::Str)
        }
        // xml.element(name: str) -> XmlElement (fresh empty element).
        "element" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!("xml.element(name) takes 1 argument, found {}", args.len()),
                    span,
                );
            } else {
                let t = checker.infer(&args[0].value);
                if !assignable(&t, &Ty::Str) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!("xml.element() expects a str name, found {}", t.display()),
                        expr_span(&args[0].value),
                    );
                }
            }
            Some(Ty::Named("XmlElement".to_string()))
        }
        // Internal tokenizer-cursor primitive: xml._lex_new(s) -> bytes (handle).
        "_lex_new" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!("xml._lex_new(s) takes 1 argument, found {}", args.len()),
                    span,
                );
            } else {
                expect_strbytes(checker, "_lex_new", &args[0]);
            }
            Some(Ty::Bytes)
        }
        // xml._lex_next(h) -> int64 token kind.
        "_lex_next" => {
            expect_one(checker, method, args, span);
            Some(Ty::Int64)
        }
        // xml._lex_name(h) -> str ; xml._lex_value(h) -> str (current token).
        "_lex_name" | "_lex_value" => {
            expect_one(checker, method, args, span);
            Some(Ty::Str)
        }
        // Serializer helpers: text/attribute escaping and fragment join.
        "_esc_text" | "_esc_attr" | "_join" => {
            expect_one(checker, method, args, span);
            Some(Ty::Str)
        }
        _ => None,
    }
}
