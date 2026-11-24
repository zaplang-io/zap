//! Comptime / `@derive` diagnostics (#1256, HQ4): the `comptime.error(at, msg)`
//! primitive, the `.span` accessors that feed it, the stable `ZAP7xxx` code
//! mapping, and the rendered derive/instantiation backtrace — consumer-grade
//! "cannot derive X for T" errors and dual-span generator faults, never a panic.
//!
//! Intent documentation: these tests are the walled garden around the promise
//! that a derive that cannot apply points at the *user's own field* with a real
//! diagnostic, and that a generator fault is a clean compile error citing both
//! the generator and the instantiation — not a crash or an internal symbol.

mod common;

use std::rc::Rc;

use common::{parse_expr, parse_structs};
use zapc_comptime::{
    error_code, render_ct_error, render_derive_error, render_generator_fault, type_info_of_struct,
    unroll, AttrArg, AttrInfo, CtError, CtErrorKind, DeriveBacktrace, Env, ErrorCode, Evaluator,
    FieldInfo, Frame, TypeInfo, TypeRegistry, Value, VariantInfo,
};
use zapc_syntax::ast::Type;
use zapc_syntax::span::Span;

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn sp() -> Span {
    Span::new(0, 0)
}

fn named_ty(name: &str) -> Type {
    Type::Named {
        name: name.to_string(),
        args: vec![],
        span: sp(),
    }
}

/// A `FieldInfo` with an explicit span (so a test can assert the error lands on
/// exactly this span).
fn field_at(name: &str, index: usize, span: Span) -> FieldInfo {
    FieldInfo {
        name: name.to_string(),
        ty: zapc_sema_types::Ty::Int64,
        decl_ty: named_ty("int64"),
        index,
        attrs: vec![],
        span,
    }
}

/// Evaluate `expr_src` with `bindings` pre-seeded (used to inject a record whose
/// span the test controls), returning the raw result.
fn eval_with(expr_src: &str, bindings: Vec<(&str, Value)>) -> Result<Value, CtError> {
    let e = parse_expr(expr_src);
    let mut env = Env::new();
    for (name, value) in bindings {
        env.define(name, value);
    }
    let mut ev = Evaluator::new();
    ev.eval(&e, &mut env)
}

// ===========================================================================
// 1. `comptime.error(at, msg)` primitive
// ===========================================================================

#[test]
fn comptime_error_two_arg_surfaces_at_the_given_span_with_the_exact_message() {
    let field_span = Span::new(42, 50);
    let f = Value::Field(Rc::new(field_at("tags", 1, field_span)));
    let err = eval_with(
        "comptime.error(f.span, \"serde: unsupported field type\")",
        vec![("f", f)],
    )
    .expect_err("comptime.error always aborts");
    assert_eq!(err.kind, CtErrorKind::UserError);
    assert!(err.is_user());
    assert_eq!(err.span, field_span, "error points at the field's span");
    assert_eq!(err.message, "serde: unsupported field type");
}

#[test]
fn comptime_error_one_arg_points_at_the_call_site() {
    // No span argument: the abort points at the `comptime.error(...)` call span.
    let e = parse_expr("comptime.error(\"boom\")");
    let mut ev = Evaluator::new();
    let err = ev
        .eval(&e, &mut Env::new())
        .expect_err("comptime.error always aborts");
    assert_eq!(err.kind, CtErrorKind::UserError);
    assert_eq!(err.message, "boom");
    // The call-site span is the whole `comptime.error(...)` expression.
    assert_eq!(err.span, zapc_comptime_expr_span(&e));
}

/// Mirror of the crate-private `expr_span` for asserting the call-site span.
fn zapc_comptime_expr_span(e: &zapc_syntax::ast::Expr) -> Span {
    match e {
        zapc_syntax::ast::Expr::Call { span, .. } => *span,
        other => panic!("expected a call expression, got {other:?}"),
    }
}

#[test]
fn comptime_error_span_is_faithful_to_a_parsed_source_field() {
    // A real struct: `comptime.error(info.fields[1].span, ...)` must report the
    // actual source span of the second field, not a synthesized one.
    let src = "struct User:\n    int64 id\n    str name\n";
    let s = parse_structs(src).into_iter().next().unwrap();
    let info = type_info_of_struct(&s);
    let name_field_span = info.fields()[1].span;

    let err = eval_with(
        "comptime.error(info.fields[1].span, \"nope\")",
        vec![("info", Value::Type(Rc::new(info.clone()), None))],
    )
    .expect_err("aborts");
    assert_eq!(err.span, name_field_span);
    assert_eq!(err.message, "nope");
    // Sanity: the span really covers the `name` field in the source.
    let slice = &src[name_field_span.start as usize..name_field_span.end as usize];
    assert!(
        slice.contains("name"),
        "span covers the field, got {slice:?}"
    );
}

#[test]
fn comptime_error_rejects_a_bad_argument_shape() {
    // Single non-string.
    let e1 = eval_with("comptime.error(123)", vec![]).unwrap_err();
    assert_eq!(e1.kind, CtErrorKind::Value);
    assert!(e1.message.contains("message"), "{}", e1.message);

    // First argument not a span.
    let e2 = eval_with("comptime.error(1, \"m\")", vec![]).unwrap_err();
    assert_eq!(e2.kind, CtErrorKind::Value);
    assert!(e2.message.contains("Span"), "{}", e2.message);

    // Span only, no message.
    let f = Value::Field(Rc::new(field_at("x", 0, Span::new(1, 2))));
    let e3 = eval_with("comptime.error(f.span)", vec![("f", f)]).unwrap_err();
    assert_eq!(e3.kind, CtErrorKind::Value);

    // Too many arguments.
    let e4 = eval_with("comptime.error(1, 2, 3)", vec![]).unwrap_err();
    assert_eq!(e4.kind, CtErrorKind::Value);
}

#[test]
fn an_unknown_comptime_builtin_is_out_of_fragment() {
    let err = eval_with("comptime.frobnicate(1)", vec![]).unwrap_err();
    assert_eq!(err.kind, CtErrorKind::NotAvailable);
    assert!(
        err.message.contains("comptime.frobnicate"),
        "{}",
        err.message
    );
}

// ===========================================================================
// 2. `.span` accessors feed `comptime.error`
// ===========================================================================

#[test]
fn field_variant_attr_span_accessors_return_the_record_span() {
    let f = Value::Field(Rc::new(field_at("f", 0, Span::new(3, 7))));
    assert_eq!(
        eval_with("f.span", vec![("f", f)]).unwrap(),
        Value::Span(Span::new(3, 7))
    );

    let v = Value::Variant(Rc::new(VariantInfo {
        name: "V".to_string(),
        index: 0,
        fields: vec![],
        attrs: vec![],
        span: Span::new(10, 15),
    }));
    assert_eq!(
        eval_with("v.span", vec![("v", v)]).unwrap(),
        Value::Span(Span::new(10, 15))
    );

    let a = Value::Attr(Rc::new(AttrInfo {
        name: "serialize".to_string(),
        args: vec![AttrArg {
            name: Some("rename".to_string()),
            value: "id".to_string(),
        }],
        span: Span::new(20, 33),
    }));
    assert_eq!(
        eval_with("a.span", vec![("a", a)]).unwrap(),
        Value::Span(Span::new(20, 33))
    );
}

#[test]
fn a_span_value_has_no_runtime_representation() {
    // It never folds into emitted code (like the other reflection records).
    assert!(zapc_comptime::materialize(&Value::Span(Span::new(0, 4)), sp()).is_none());
}

// ===========================================================================
// 3. stable ZAP code mapping
// ===========================================================================

#[test]
fn error_code_maps_each_kind_to_its_stable_code() {
    assert_eq!(
        error_code(&CtError::not_available("x", sp())),
        ErrorCode::ZAP7001
    );
    assert_eq!(error_code(&CtError::value("x", sp())), ErrorCode::ZAP7001);
    assert_eq!(error_code(&CtError::budget("x", sp())), ErrorCode::ZAP7001);
    assert_eq!(error_code(&CtError::user("x", sp())), ErrorCode::ZAP7002);
}

// ===========================================================================
// 4. render_ct_error + instantiation backtrace
// ===========================================================================

/// Byte offset of the (first) occurrence of `needle` in `src`, as a span.
fn span_of(src: &str, needle: &str) -> Span {
    let start = src.find(needle).expect("needle present") as u32;
    Span::new(start, start + needle.len() as u32)
}

const DERIVE_SRC: &str = "\
@derive(Json)
struct Outer:
    list[Widget] ws
    Widget elem
";

#[test]
fn render_ct_error_puts_a_user_error_at_its_span_with_the_code() {
    let at = span_of(DERIVE_SRC, "elem");
    let err = CtError::user("`Widget` does not implement `Serialize`", at);
    let out = render_ct_error(DERIVE_SRC, "outer.zap", &err, &DeriveBacktrace::new());

    assert!(out.contains("error[ZAP7002]"), "{out}");
    assert!(
        out.contains("`Widget` does not implement `Serialize`"),
        "{out}"
    );
    // Located: the source line and a caret.
    assert!(out.contains("Widget elem"), "{out}");
    assert!(out.contains('^'), "{out}");
    // No instantiation context, so no backtrace notes.
    assert!(!out.contains("required because of"), "{out}");
}

#[test]
fn a_multi_level_backtrace_renders_the_chain_in_order() {
    // Leaf failure at the element type; contexts above it: the field, then the
    // @derive consumer. Frames are pushed in descent order (outermost first).
    let leaf = CtError::user(
        "`Widget` does not implement `Serialize`",
        span_of(DERIVE_SRC, "Widget elem"),
    );
    let backtrace = DeriveBacktrace::new()
        .with(Frame::new(
            "`@derive(Json)` on `Outer`",
            span_of(DERIVE_SRC, "@derive(Json)"),
        ))
        .with(Frame::new(
            "field `ws` of `Outer`",
            span_of(DERIVE_SRC, "list[Widget] ws"),
        ));

    let out = render_ct_error(DERIVE_SRC, "outer.zap", &leaf, &backtrace);

    let primary = out.find("error[ZAP7002]").expect("primary present");
    let field_note = out
        .find("required because of field `ws` of `Outer`")
        .expect("field note present");
    let derive_note = out
        .find("required because of `@derive(Json)` on `Outer`")
        .expect("derive note present");

    // Notes follow the primary, inner-first: field (nearer the failure) then
    // the derive consumer (outermost).
    assert!(primary < field_note, "{out}");
    assert!(field_note < derive_note, "{out}");
    // Each note is located (its own source line echoed).
    assert!(out.contains("list[Widget] ws"), "{out}");
    assert!(out.contains("@derive(Json)"), "{out}");
    assert_eq!(backtrace.len(), 2);
}

#[test]
fn an_empty_backtrace_renders_the_primary_error_alone() {
    let err = CtError::value("division by zero", span_of(DERIVE_SRC, "Widget elem"));
    let out = render_ct_error(DERIVE_SRC, "outer.zap", &err, &DeriveBacktrace::new());
    assert!(out.contains("error[ZAP7001]"), "{out}");
    assert!(!out.contains("required because of"), "{out}");
    assert!(DeriveBacktrace::new().is_empty());
}

// ===========================================================================
// 5. consumer-grade derive errors (the non-negotiable)
// ===========================================================================

#[test]
fn an_inapplicable_derive_points_at_the_users_field_not_an_internal_symbol() {
    // The ordinary programmer wrote `@derive(Json)` on a struct with a field the
    // codec can't handle. The error must name THEIR field and source line.
    let src = "struct User:\n    int64 id\n    Widget w\n";
    let s = parse_structs(src).into_iter().next().unwrap();
    let info = type_info_of_struct(&s);
    let w_span = info.fields()[1].span;

    // The internal cause mentions a comptime-internal symbol; it must NOT leak
    // into the consumer header.
    let cause = CtError::user("`Widget` does not implement `Serialize`", w_span);
    let out = render_derive_error(
        src,
        "user.zap",
        "Json",
        "User",
        w_span,
        &cause,
        &DeriveBacktrace::new(),
    );

    // Consumer-grade header, with the dedicated code, pointing at the field.
    assert!(out.contains("error[ZAP7003]"), "{out}");
    assert!(out.contains("cannot derive `Json` for `User`"), "{out}");
    assert!(out.contains("Widget w"), "source line echoed: {out}");
    assert!(out.contains('^'), "caret present: {out}");

    // The header line (everything before the cause note) names the user's type,
    // never an internal symbol like a generator function or crate path.
    let header = out.split("note:").next().unwrap();
    assert!(header.contains("User"), "{header}");
    for internal in ["_derive_", "serialize_struct", "zapc_", "__", "Evaluator"] {
        assert!(
            !header.contains(internal),
            "header leaked internal symbol {internal:?}: {header}"
        );
    }
    // The cause is still shown, as a note, for the library author.
    assert!(out.contains("the generator reported"), "{out}");
    assert!(out.contains("does not implement `Serialize`"), "{out}");
}

#[test]
fn a_derive_error_threads_the_backtrace_and_cause() {
    let src = "struct User:\n    int64 id\n    list[Widget] ws\n";
    let s = parse_structs(src).into_iter().next().unwrap();
    let info = type_info_of_struct(&s);
    let ws_span = info.fields()[1].span;

    let cause = CtError::user("`Widget` does not implement `Serialize`", ws_span);
    let backtrace = DeriveBacktrace::new().with(Frame::new("field `ws` of `User`", ws_span));
    let out = render_derive_error(src, "user.zap", "Json", "User", ws_span, &cause, &backtrace);
    assert!(out.contains("cannot derive `Json` for `User`"), "{out}");
    assert!(
        out.contains("required because of field `ws` of `User`"),
        "{out}"
    );
    assert!(out.contains("the generator reported"), "{out}");
}

// ===========================================================================
// 6. dual-span generator faults, and the never-panic guarantee
// ===========================================================================

const GEN_SRC: &str = "\
def _derive(value):
    return value // 0
auto u = pack(Thing())
";

#[test]
fn a_generator_fault_cites_both_the_generator_and_the_instantiation() {
    let gen_span = span_of(GEN_SRC, "value // 0");
    let inst_span = span_of(GEN_SRC, "pack(Thing())");
    let err = CtError::value("division by zero at comptime", gen_span);

    let out = render_generator_fault(GEN_SRC, "gen.zap", &err, gen_span, inst_span);

    assert!(out.contains("error[ZAP7001]"), "{out}");
    assert!(out.contains("error in comptime generator"), "{out}");
    assert!(out.contains("division by zero"), "{out}");
    assert!(out.contains("value // 0"), "generator line echoed: {out}");
    assert!(out.contains("instantiated here"), "{out}");
    assert!(
        out.contains("pack(Thing())"),
        "instantiation line echoed: {out}"
    );
}

#[test]
fn a_faulty_generator_is_a_diagnostic_never_a_panic() {
    // A generator whose body reaches an impure leaf (`print`) must fail as a
    // clean CtError the evaluator returns — never a panic — and render cleanly.
    let fns = common::parse_fns("def _derive(int64 value):\n    print(value)\n    return value\n");
    let mut ev = Evaluator::with_functions(&fns);
    let err = ev
        .call("_derive", vec![Value::Int(1)], sp())
        .expect_err("impure leaf is out of fragment");
    assert_eq!(err.kind, CtErrorKind::NotAvailable);

    // Rendering the fault never panics and names both places.
    let out = render_generator_fault("x", "gen.zap", &err, Span::new(0, 1), Span::new(0, 1));
    assert!(out.contains("error in comptime generator"), "{out}");
}

#[test]
fn rendering_tolerates_out_of_range_spans_without_panicking() {
    // A span past the end of the source clamps rather than panicking.
    let err = CtError::user("boom", Span::new(9000, 9100));
    let out = render_ct_error("short", "x.zap", &err, &DeriveBacktrace::new());
    assert!(out.contains("error[ZAP7002]"), "{out}");
}

// ===========================================================================
// 7. span fidelity through kind-switch recursion and unrolling
// ===========================================================================

const SHAPES_SRC: &str = "\
struct Inner:
    int64 a
struct Holder:
    int64 n
    Inner nested
";

fn holder() -> (TypeRegistry, TypeInfo) {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(SHAPES_SRC);
    assert!(lerrs.is_empty() && perrs.is_empty());
    let reg = TypeRegistry::from_module(&module);
    let s = parse_structs(SHAPES_SRC)
        .into_iter()
        .find(|d| d.name == "Holder")
        .unwrap();
    (reg, type_info_of_struct(&s))
}

#[test]
fn span_survives_a_kind_switch_recursion_into_a_field_type() {
    // A comptime generator descends into a field's type (a kind-switch via
    // `.ty.kind`, #1253) and, for the non-primitive field, aborts pointing at
    // the field's own span. The span must be the field's real source span,
    // threaded through the `.ty` recursion unchanged.
    let (reg, info) = holder();
    let nested_span = info.fields()[1].span; // `Inner nested`

    // classify(info): if the second field's type is not Primitive, abort at it.
    let module = "\
def classify(str info):
    return check(info.fields[1])
def check(str f):
    if f.ty.kind == Primitive:
        return 0
    return comptime.error(f.span, \"serde: cannot serialize a nested struct field\")
";
    let fns = common::parse_fns(module);
    let e = parse_expr("classify(info)");
    let mut env = Env::new();
    env.define("info", Value::Type(Rc::new(info.clone()), None));
    zapc_comptime::seed_kind_constants(&mut env);
    let mut ev = Evaluator::with_functions(&fns).with_registry(&reg);
    let err = ev
        .eval(&e, &mut env)
        .expect_err("nested struct field aborts");

    assert_eq!(err.kind, CtErrorKind::UserError);
    assert_eq!(
        err.span, nested_span,
        "span threaded through `.ty` kind-switch"
    );
    assert!(
        err.message.contains("nested struct field"),
        "{}",
        err.message
    );

    // And it renders at the user's `Inner nested` line.
    let out = render_ct_error(SHAPES_SRC, "holder.zap", &err, &DeriveBacktrace::new());
    assert!(out.contains("Inner nested"), "{out}");
}

#[test]
fn field_spans_are_faithful_across_comptime_for_unrolling() {
    // The values a `comptime for` iterates (`info.fields`) carry the real source
    // spans, and the loop binds each in turn — so a per-field diagnostic raised
    // from the unrolled body would land on that field. We assert both: the
    // unroller produces one body-instance per field, and each field's `.span`
    // equals its parsed source span.
    let (_reg, info) = holder();
    let field_spans: Vec<Span> = info.fields().iter().map(|f| f.span).collect();
    assert_eq!(field_spans.len(), 2);

    // Unroll `comptime for f in info.fields:` over a trivial body; one instance
    // per field, with the body span preserved (not rewritten to a synthetic).
    let body = common::parse_body("let idx = f.index\n");
    let iterable = parse_expr("info.fields");
    let mut base = Env::new();
    base.define("info", Value::Type(Rc::new(info.clone()), None));
    let mut ev = Evaluator::new();
    let stmts = unroll("f", &iterable, &body, &base, &mut ev).expect("unrolls");
    assert_eq!(stmts.len(), 2, "one `let idx = ...` per field");

    // Each iterated field, accessed as the loop would, reports its real span,
    // and a `comptime.error(f.span, ...)` on it lands exactly there.
    for (i, want) in field_spans.iter().enumerate() {
        let f = Value::Field(Rc::new(info.fields()[i].clone()));
        let got = eval_with("f.span", vec![("f", f.clone())]).unwrap();
        assert_eq!(got, Value::Span(*want));
        let err = eval_with("comptime.error(f.span, \"x\")", vec![("f", f)]).unwrap_err();
        assert_eq!(err.span, *want);
    }
}
