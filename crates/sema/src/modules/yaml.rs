//! `yaml` module — sema facet (ticket #149, D-0063).
//!
//! A pragmatic YAML subset serde module, the third format on the shared derive
//! substrate (after json/csv). YAML 1.2 is a JSON superset, so `yaml.load`
//! reuses the compiler-known `JsonValue` DOM rather than a parallel type:
//! `yaml.load(str|bytes) -> JsonValue` and `yaml.dump(JsonValue) -> str`. The
//! tree build and serialization live in Zap prelude functions
//! (`__yaml_loads`/`__yaml_dumps`, see `zapc_syntax::parser::PRELUDE_SRC`);
//! `yaml._to_json` is the internal C transpiler (`zaprt.c`) that lowers the
//! supported YAML surface to a JSON string for the existing json tokenizer.
//! `@derive(Yaml)` (handled in the lowerer) adds `to_yaml`/`from_yaml`.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{assignable, expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "yaml",
    check,
};

/// Require a `str`/`bytes` argument (both share the `ZapStr` layout); infer it
/// regardless so it is still type-checked.
fn expect_strbytes(checker: &mut Checker, func: &str, arg: &Arg) {
    let t = checker.infer(&arg.value);
    if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
        checker.error(
            ErrorCode::ZAP4024,
            format!(
                "yaml.{func}() expects a str or bytes argument, found {}",
                t.display()
            ),
            expr_span(&arg.value),
        );
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // yaml.load(s: str|bytes) -> JsonValue (the shared dynamic DOM).
        "load" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!("yaml.load(s) takes 1 argument, found {}", args.len()),
                    span,
                );
            } else {
                expect_strbytes(checker, "load", &args[0]);
            }
            Some(Ty::Named("JsonValue".to_string()))
        }
        // yaml.dump(v: JsonValue) -> str (block-style serialization).
        "dump" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!("yaml.dump(v) takes 1 argument, found {}", args.len()),
                    span,
                );
            } else {
                let t = checker.infer(&args[0].value);
                if !assignable(&t, &Ty::Named("JsonValue".to_string())) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!("yaml.dump() expects a JsonValue, found {}", t.display()),
                        expr_span(&args[0].value),
                    );
                }
            }
            Some(Ty::Str)
        }
        // Internal: transpile the YAML subset to a JSON string for the json
        // tokenizer (driven by the prelude `__yaml_loads` builder).
        "_to_json" => {
            if args.len() == 1 {
                expect_strbytes(checker, "_to_json", &args[0]);
            }
            Some(Ty::Str)
        }
        _ => None,
    }
}
