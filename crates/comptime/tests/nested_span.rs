//! Nested-type **use-site spans** (#1346): a container's element/key/value/
//! inner/tuple-element type, reached through `field.ty.elem` (etc.), carries the
//! source span of *that nested reference as written in the field's declaration*
//! — the `Widget` in `list[Widget] items`, not a declaration of `Widget`
//! elsewhere. This is what lets a `@derive` generator point `comptime.error` at
//! the offending nested level, and what the multi-level `required because of`
//! chain (E3d #1293) anchors its intermediate frames on.
//!
//! Intent documentation: each container accessor's `.span`; the field type span;
//! arbitrary-depth nesting resolving to the innermost use-site; primitives; and
//! the deliberate absence of a span on a reference with no declared `Type` in
//! hand (a top-level `type_info(T)` / a `Ty`-resolved reference).

mod common;

use common::parse_structs;
use std::rc::Rc;
use zapc_comptime::{type_info_of_struct, Env, Evaluator, TypeInfo, TypeRegistry, Value};
use zapc_syntax::span::Span;

const MODULE: &str = "\
struct Widget:\n\
\x20   int64 w\n\
struct Bag:\n\
\x20   int64 n\n\
\x20   list[Widget] items\n\
\x20   dict[str, Widget] by_name\n\
\x20   Optional[Widget] maybe\n\
\x20   (int64, Widget) pair\n\
\x20   list[list[Widget]] grid\n\
\x20   dict[str, list[Widget]] buckets\n";

fn registry() -> TypeRegistry {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(MODULE);
    assert!(lerrs.is_empty() && perrs.is_empty(), "{lerrs:?} {perrs:?}");
    TypeRegistry::from_module(&module)
}

fn bag_info() -> TypeInfo {
    let s = parse_structs(MODULE)
        .into_iter()
        .find(|d| d.name == "Bag")
        .unwrap();
    type_info_of_struct(&s)
}

/// Evaluate `expr_src` with `info` = `Bag` and the registry attached, expecting
/// success.
fn eval_ok(expr_src: &str) -> Value {
    let (reg, info) = (registry(), bag_info());
    let e = common::parse_expr(expr_src);
    let mut env = Env::new();
    env.define("info", Value::Type(Rc::new(info), None));
    let mut ev = Evaluator::new().with_registry(&reg);
    ev.eval(&e, &mut env)
        .unwrap_or_else(|err| panic!("eval `{expr_src}` failed: {err:?}"))
}

/// Evaluate `expr_src`, expecting a comptime error.
fn eval_err(expr_src: &str) -> zapc_comptime::CtError {
    let (reg, info) = (registry(), bag_info());
    let e = common::parse_expr(expr_src);
    let mut env = Env::new();
    env.define("info", Value::Type(Rc::new(info), None));
    let mut ev = Evaluator::new().with_registry(&reg);
    match ev.eval(&e, &mut env) {
        Ok(v) => panic!("eval `{expr_src}` unexpectedly succeeded with {v:?}"),
        Err(e) => e,
    }
}

/// The source text a `.span` value points at — the use-site substring.
fn span_text(expr_src: &str) -> String {
    let v = eval_ok(expr_src);
    let s = v
        .as_span()
        .unwrap_or_else(|| panic!("`{expr_src}` did not evaluate to a Span (got {v:?})"));
    MODULE[s.start as usize..s.end as usize].to_string()
}

fn span_of(expr_src: &str) -> Span {
    eval_ok(expr_src).as_span().expect("a Span")
}

// ---------------------------------------------------------------------------
// each container accessor's nested use-site span
// ---------------------------------------------------------------------------

#[test]
fn field_type_span_is_the_declared_type() {
    // n:int64 -> "int64"; items:list[Widget] -> "list[Widget]".
    assert_eq!(span_text("info.fields[0].ty.span"), "int64");
    assert_eq!(span_text("info.fields[1].ty.span"), "list[Widget]");
}

#[test]
fn list_element_span_is_the_use_site() {
    // items: list[Widget] -> .elem.span is the `Widget` *in this declaration*.
    assert_eq!(span_text("info.fields[1].ty.elem.span"), "Widget");
}

#[test]
fn dict_key_and_value_spans_are_use_sites() {
    // by_name: dict[str, Widget].
    assert_eq!(span_text("info.fields[2].ty.key.span"), "str");
    assert_eq!(span_text("info.fields[2].ty.value.span"), "Widget");
}

#[test]
fn optional_inner_span_is_the_use_site() {
    // maybe: Optional[Widget].
    assert_eq!(span_text("info.fields[3].ty.inner.span"), "Widget");
}

#[test]
fn tuple_element_spans_are_use_sites() {
    // pair: (int64, Widget).
    assert_eq!(span_text("info.fields[4].ty.elems[0].span"), "int64");
    assert_eq!(span_text("info.fields[4].ty.elems[1].span"), "Widget");
}

// ---------------------------------------------------------------------------
// arbitrary-depth nesting resolves to the innermost use-site
// ---------------------------------------------------------------------------

#[test]
fn nested_list_descends_to_innermost_use_site() {
    // grid: list[list[Widget]] — .elem is `list[Widget]`, .elem.elem is `Widget`.
    assert_eq!(span_text("info.fields[5].ty.elem.span"), "list[Widget]");
    assert_eq!(span_text("info.fields[5].ty.elem.elem.span"), "Widget");
}

#[test]
fn dict_value_list_element_descends_to_use_site() {
    // buckets: dict[str, list[Widget]] — .value is `list[Widget]`, .value.elem
    // is `Widget`.
    assert_eq!(span_text("info.fields[6].ty.value.span"), "list[Widget]");
    assert_eq!(span_text("info.fields[6].ty.value.elem.span"), "Widget");
}

// ---------------------------------------------------------------------------
// the nested span is distinct from, and enclosed by, the field's declaration
// ---------------------------------------------------------------------------

#[test]
fn element_span_is_enclosed_by_field_type_and_distinct_from_field() {
    let field_span = {
        // `info.fields[1].span` is the field itself.
        eval_ok("info.fields[1].span").as_span().expect("span")
    };
    let field_ty_span = span_of("info.fields[1].ty.span");
    let elem_span = span_of("info.fields[1].ty.elem.span");
    // The element is a strict sub-span of the declared field type.
    assert!(
        field_ty_span.start <= elem_span.start && elem_span.end <= field_ty_span.end,
        "elem {elem_span:?} not within field type {field_ty_span:?}"
    );
    assert!(
        elem_span.start > field_ty_span.start,
        "element use-site should start after the `list[` prefix"
    );
    // And it is not the whole field (the single-frame #1335 fallback span).
    assert_ne!(elem_span, field_span);
}

// ---------------------------------------------------------------------------
// a reference with no declared `Type` in hand has no use-site span
// ---------------------------------------------------------------------------

#[test]
fn top_level_type_reference_has_no_span() {
    // `info` itself was bound with no use-site (a top-level `type_info(T)`
    // carries none), so `.span` is a clean comptime error, not a bogus span.
    let e = eval_err("info.span");
    assert!(
        e.message.contains("no source span"),
        "unexpected message: {}",
        e.message
    );
}
