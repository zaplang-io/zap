//! `json` module — sema facet (ticket #100/#131, D-0059).
//!
//! Native JSON. The public surface is `json.loads(str|bytes) -> JsonValue` (the
//! dynamic DOM; `JsonValue` is the compiler-known prelude enum). The tree build
//! lives in Zap prelude functions (see `zapc_syntax::parser::PRELUDE_SRC`) that
//! drive a C tokenizer through the internal `json._lex_*` cursor primitives, so
//! JSON<->JsonValue construction stays in compiler-emitted code and the runtime
//! never depends on the `JsonValue` layout. `json.dumps` and the typed
//! `@derive(Json)` codec are follow-up slices (D-0059).

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{assignable, expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "json",
    check,
};

/// Require a `str`/`bytes` argument (both share the `ZapStr` layout); infer it
/// regardless so it is still type-checked.
fn expect_strbytes(checker: &mut Checker, func: &str, arg: &Arg) {
    let t = checker.infer(&arg.value);
    if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
        checker.error(
            format!(
                "{func}() expects a str or bytes argument, found {}",
                t.display()
            ),
            expr_span(&arg.value),
        );
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // json.loads(s: str|bytes) -> JsonValue (the dynamic DOM).
        "loads" => {
            if args.len() != 1 {
                checker.error(
                    format!("json.loads(s) takes 1 argument, found {}", args.len()),
                    span,
                );
            } else {
                expect_strbytes(checker, "loads", &args[0]);
            }
            Some(Ty::Named("JsonValue".to_string()))
        }
        // Internal tokenizer-cursor primitives (driven by the prelude builders).
        // json._lex_new(s: str|bytes) -> bytes (opaque cursor handle).
        "_lex_new" => {
            if args.len() != 1 {
                checker.error(
                    format!("json._lex_new(s) takes 1 argument, found {}", args.len()),
                    span,
                );
            } else {
                expect_strbytes(checker, "_lex_new", &args[0]);
            }
            Some(Ty::Bytes)
        }
        // json._lex_next(h) -> int64 token kind; json._lex_int(h) -> int64.
        "_lex_next" | "_lex_int" => {
            if args.len() != 1 {
                checker.error(
                    format!("json.{method}(h) takes 1 argument, found {}", args.len()),
                    span,
                );
            } else {
                checker.infer(&args[0].value);
            }
            Some(Ty::Int64)
        }
        // json._lex_float(h) -> float64.
        "_lex_float" => {
            if args.len() != 1 {
                checker.error(
                    format!("json._lex_float(h) takes 1 argument, found {}", args.len()),
                    span,
                );
            } else {
                checker.infer(&args[0].value);
            }
            Some(Ty::Float64)
        }
        // json._lex_str(h) -> str.
        "_lex_str" => {
            if args.len() != 1 {
                checker.error(
                    format!("json._lex_str(h) takes 1 argument, found {}", args.len()),
                    span,
                );
            } else {
                checker.infer(&args[0].value);
            }
            Some(Ty::Str)
        }
        _ => None,
    }
}
