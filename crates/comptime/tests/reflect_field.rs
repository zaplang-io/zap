//! Typed `reflect.field(value, comptime i)` access (#1252): the comptime index
//! resolves to the field at that declaration position and desugars to the
//! ordinary typed field access `value.<name>`, which the existing per-layout
//! lowering (value-struct / heap-struct / `@scoped`) compiles unchanged.
//!
//! Intent documentation: resolution for each field type, declaration-order
//! match with `type_info`, the out-of-range comptime error (never a runtime
//! OOB), desugaring across an expression, `field_decl_type`, and the
//! #1251+#1252 end-to-end (`comptime for` supplying the index).

mod common;

use common::{parse_body, parse_expr, parse_structs, sp};
use std::rc::Rc;
use zapc_comptime::{
    desugar_reflect_field, field_decl_type, reflect_field, type_info_of_struct, unroll, Env,
    Evaluator, Value,
};
use zapc_syntax::ast::{render_expr, Stmt, Type};

const SHAPES: &str = "\
struct Point:\n\
\x20   int64 x\n\
\x20   int64 y\n\
struct Rec:\n\
\x20   int64 n\n\
\x20   bool flag\n\
\x20   str name\n\
\x20   Point origin\n\
\x20   list[int64] xs\n";

fn rec_info() -> zapc_comptime::TypeInfo {
    let defs = parse_structs(SHAPES);
    let rec = defs.into_iter().find(|d| d.name == "Rec").unwrap();
    type_info_of_struct(&rec)
}

// ---------------------------------------------------------------------------
// index → value.<name>, for each field type
// ---------------------------------------------------------------------------

#[test]
fn resolves_each_field_by_index() {
    let info = rec_info();
    let v = parse_expr("value");
    let names = ["n", "flag", "name", "origin", "xs"];
    for (i, name) in names.iter().enumerate() {
        let acc = reflect_field(&v, i as i64, &info, sp()).unwrap();
        assert_eq!(render_expr(&acc), format!("value.{name}"));
    }
}

#[test]
fn declaration_order_matches_type_info() {
    let info = rec_info();
    // The i-th reflect.field names the i-th type_info field.
    for (i, f) in info.fields().iter().enumerate() {
        let acc = reflect_field(&parse_expr("v"), i as i64, &info, sp()).unwrap();
        assert_eq!(render_expr(&acc), format!("v.{}", f.name));
    }
}

#[test]
fn resolves_on_an_arbitrary_value_expression() {
    let info = rec_info();
    // The value operand can be any expression, not just a name.
    let acc = reflect_field(&parse_expr("make()"), 2, &info, sp()).unwrap();
    assert_eq!(render_expr(&acc), "make().name");
}

// ---------------------------------------------------------------------------
// out-of-range index → compile error at the comptime site
// ---------------------------------------------------------------------------

#[test]
fn out_of_range_index_is_a_compile_error() {
    let info = rec_info();
    let v = parse_expr("value");
    for bad in [5i64, 99, -1] {
        let e = reflect_field(&v, bad, &info, sp()).unwrap_err();
        assert!(e.message.contains("out of range"), "{bad}: {}", e.message);
    }
}

// ---------------------------------------------------------------------------
// field_decl_type — the field's static declared type
// ---------------------------------------------------------------------------

#[test]
fn field_declared_types() {
    let info = rec_info();
    let named = |i: i64| match field_decl_type(i, &info).unwrap() {
        Type::Named { name, .. } => name,
        other => panic!("expected a named type, got {other:?}"),
    };
    assert_eq!(named(0), "int64");
    assert_eq!(named(1), "bool");
    assert_eq!(named(2), "str");
    assert_eq!(named(3), "Point"); // nested struct
    assert_eq!(named(4), "list"); // list[int64]
    assert!(field_decl_type(9, &info).is_none());
}

// ---------------------------------------------------------------------------
// desugar across an expression
// ---------------------------------------------------------------------------

#[test]
fn desugars_reflect_field_calls_in_an_expression() {
    let info = rec_info();
    let e = parse_expr("emit(reflect.field(value, 0), reflect.field(value, 2))");
    let d = desugar_reflect_field(&e, &info).unwrap();
    assert_eq!(render_expr(&d), "emit(value.n, value.name)");
}

#[test]
fn desugar_leaves_non_reflect_calls_untouched() {
    let info = rec_info();
    let e = parse_expr("other(value, 0)");
    let d = desugar_reflect_field(&e, &info).unwrap();
    assert_eq!(render_expr(&d), "other(value, 0)");
}

#[test]
fn desugar_out_of_range_is_an_error() {
    let info = rec_info();
    let e = parse_expr("emit(reflect.field(value, 7))");
    let err = desugar_reflect_field(&e, &info).unwrap_err();
    assert!(err.message.contains("out of range"), "{}", err.message);
}

// ---------------------------------------------------------------------------
// #1251 + #1252 end-to-end: comptime for supplies the index
// ---------------------------------------------------------------------------

#[test]
fn comptime_for_with_reflect_field_access() {
    // `comptime for f in info.fields: emit(reflect.field(value, f.index))`
    // unrolls (#1251, folding f.index to a constant) then the reflect.field
    // desugars (#1252) to a typed field access per field.
    let info = rec_info();
    let body = parse_body("emit(reflect.field(value, f.index))");
    let iterable = parse_expr("info.fields");
    let mut base = Env::new();
    base.define("info", Value::Type(Rc::new(info.clone())));
    let mut ev = Evaluator::new();
    let unrolled = unroll("f", &iterable, &body, &base, &mut ev).unwrap();

    // Desugar reflect.field in each emitted statement.
    let rendered: Vec<String> = unrolled
        .iter()
        .map(|st| match st {
            Stmt::Expr(e) => render_expr(&desugar_reflect_field(e, &info).unwrap()),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        rendered,
        vec![
            "emit(value.n)",
            "emit(value.flag)",
            "emit(value.name)",
            "emit(value.origin)",
            "emit(value.xs)",
        ]
    );
}
