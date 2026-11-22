//! Reflection-record access at comptime (#1250): field-access-by-index over the
//! E1 `TypeInfo`/`FieldInfo`/`VariantInfo`/`AttrInfo`/`AttrArg` records, and the
//! HQ9 unblock — a library function run at comptime to resolve a serialized
//! field name (the `rename` / `rename_all` pattern), the whole reason the
//! fragment must execute code, not just fixed primitives.
//!
//! Both halves of the E1→E2 path are exercised: records built by hand, and real
//! records produced by the E1 resolver (`analyze_reflect`) from parsed source.

mod common;

use common::sp;
use std::rc::Rc;
use zapc_comptime::{AttrArg, AttrInfo, Evaluator, FieldInfo, TypeInfo, TypeShape, Value};
use zapc_syntax::ast::Type;
use zapc_syntax::span::Span;

// --- fixture builders -------------------------------------------------------

fn named_ty(name: &str) -> Type {
    Type::Named {
        name: name.to_string(),
        args: vec![],
        span: sp(),
    }
}

fn field(name: &str, index: usize, attrs: Vec<AttrInfo>) -> FieldInfo {
    FieldInfo {
        name: name.to_string(),
        ty: zapc_sema_types::Ty::Int64,
        decl_ty: named_ty("int64"),
        index,
        attrs,
        span: sp(),
    }
}

fn kwarg(name: &str, value: &str) -> AttrArg {
    AttrArg {
        name: Some(name.to_string()),
        value: value.to_string(),
    }
}

fn attr(name: &str, args: Vec<AttrArg>) -> AttrInfo {
    AttrInfo {
        name: name.to_string(),
        args,
        span: sp(),
    }
}

fn struct_info(name: &str, fields: Vec<FieldInfo>) -> TypeInfo {
    TypeInfo {
        shape: TypeShape::Struct {
            name: name.to_string(),
            fields,
        },
    }
}

/// Evaluate `expr_src` with `info` pre-bound to a type record.
fn eval_info(info: TypeInfo, expr_src: &str) -> Value {
    common::eval_with(expr_src, vec![("info", Value::Type(Rc::new(info)))])
        .unwrap_or_else(|e| panic!("eval `{expr_src}` failed: {e:?}"))
}

// --- accessors (hand-built records) ----------------------------------------

#[test]
fn type_info_kind_and_name() {
    let ti = struct_info("Point", vec![field("x", 0, vec![]), field("y", 1, vec![])]);
    assert_eq!(
        eval_info(ti.clone(), "info.kind.name"),
        Value::str("struct")
    );
    assert_eq!(eval_info(ti, "info.name"), Value::str("Point"));
}

#[test]
fn field_count_and_by_index_access() {
    let ti = struct_info("Point", vec![field("x", 0, vec![]), field("y", 1, vec![])]);
    assert_eq!(eval_info(ti.clone(), "len(info.fields)"), Value::Int(2));
    assert_eq!(
        eval_info(ti.clone(), "info.fields[0].name"),
        Value::str("x")
    );
    assert_eq!(
        eval_info(ti.clone(), "info.fields[1].name"),
        Value::str("y")
    );
    assert_eq!(eval_info(ti.clone(), "info.fields[0].index"), Value::Int(0));
    assert_eq!(eval_info(ti, "info.fields[1].index"), Value::Int(1));
}

#[test]
fn field_attribute_access() {
    let ti = struct_info(
        "User",
        vec![
            field("id", 0, vec![attr("rename", vec![kwarg("to", "user_id")])]),
            field("name", 1, vec![]),
        ],
    );
    assert_eq!(
        eval_info(ti.clone(), "len(info.fields[0].attrs)"),
        Value::Int(1)
    );
    assert_eq!(
        eval_info(ti.clone(), "info.fields[0].attrs[0].name"),
        Value::str("rename")
    );
    assert_eq!(
        eval_info(ti.clone(), "info.fields[0].attrs[0].args[0].value"),
        Value::str("user_id")
    );
    assert_eq!(
        eval_info(ti.clone(), "info.fields[0].attrs[0].args[0].name"),
        Value::str("to")
    );
    assert_eq!(
        eval_info(ti.clone(), "info.fields[0].attrs[0].args[0].is_named"),
        Value::Bool(true)
    );
    assert_eq!(eval_info(ti, "len(info.fields[1].attrs)"), Value::Int(0));
}

#[test]
fn positional_attr_arg_has_no_name() {
    let ti = struct_info(
        "T",
        vec![field(
            "f",
            0,
            vec![attr(
                "default",
                vec![AttrArg {
                    name: None,
                    value: "100".to_string(),
                }],
            )],
        )],
    );
    assert_eq!(
        eval_info(ti.clone(), "info.fields[0].attrs[0].args[0].value"),
        Value::str("100")
    );
    assert_eq!(
        eval_info(ti.clone(), "info.fields[0].attrs[0].args[0].is_named"),
        Value::Bool(false)
    );
    // `.name` on a positional argument is a value fault, not a silent default.
    let e = common::eval_with(
        "info.fields[0].attrs[0].args[0].name",
        vec![("info", Value::Type(Rc::new(ti)))],
    )
    .unwrap_err();
    assert!(e.message.contains("positional"), "{}", e.message);
}

#[test]
fn field_type_recursion_is_deferred_to_1253() {
    let ti = struct_info("T", vec![field("f", 0, vec![])]);
    let e = common::eval_with(
        "info.fields[0].ty",
        vec![("info", Value::Type(Rc::new(ti)))],
    )
    .unwrap_err();
    assert!(e.is_not_available(), "{}", e.message);
    assert!(e.message.contains("#1253"), "{}", e.message);
}

// --- HQ9: resolve a serialized field name at comptime -----------------------

const RENAME_LIB: &str = "\
def serialized_name(str f):\n\
\x20   if len(f.attrs) > 0:\n\
\x20       return f.attrs[0].args[0].value\n\
\x20   return f.name.upper()\n";

#[test]
fn comptime_library_resolves_rename_override() {
    // A field carrying `@rename(to="user_id")` serializes under that name;
    // a bare field falls back to the uppercased declaration name. This is the
    // `rename` / `rename_all` logic a serde-style library runs at comptime.
    let fns = common::parse_fns(RENAME_LIB);
    let mut ev = Evaluator::with_functions(&fns);

    let renamed = Value::Field(Rc::new(field(
        "id",
        0,
        vec![attr("rename", vec![kwarg("to", "user_id")])],
    )));
    assert_eq!(
        ev.call("serialized_name", vec![renamed], Span::new(0, 0))
            .unwrap(),
        Value::str("user_id")
    );

    let plain = Value::Field(Rc::new(field("name", 1, vec![])));
    assert_eq!(
        ev.call("serialized_name", vec![plain], Span::new(0, 0))
            .unwrap(),
        Value::str("NAME")
    );
}

// --- resolver-driven: real TypeInfo from parsed source ----------------------

/// Reflect `type_name` out of `src` using the real E1 resolver.
fn reflect(src: &str, type_name: &str) -> TypeInfo {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (terrs, _warn, reflector) = zapc_sema::analyze_reflect(&module);
    assert!(terrs.is_empty(), "sema errors: {terrs:?}");
    reflector
        .type_info_by_name(type_name)
        .expect("introspectable type")
}

#[test]
fn resolver_produced_record_is_readable_at_comptime() {
    let ti = reflect(
        "struct Account:\n    @default(\"user_id\")\n    str id\n    str name\n    int64 age\n",
        "Account",
    );
    assert_eq!(eval_info(ti.clone(), "info.name"), Value::str("Account"));
    assert_eq!(
        eval_info(ti.clone(), "info.kind.name"),
        Value::str("struct")
    );
    assert_eq!(eval_info(ti.clone(), "len(info.fields)"), Value::Int(3));
    assert_eq!(
        eval_info(ti.clone(), "info.fields[0].name"),
        Value::str("id")
    );
    assert_eq!(eval_info(ti.clone(), "info.fields[2].index"), Value::Int(2));
    // Declaration order is preserved (#1066).
    assert_eq!(
        eval_info(ti.clone(), "info.fields[1].name"),
        Value::str("name")
    );
    // The `@default("user_id")` attribute's argument flows through to comptime.
    assert_eq!(
        eval_info(ti, "info.fields[0].attrs[0].args[0].value"),
        Value::str("user_id")
    );
}

#[test]
fn comptime_library_over_a_resolved_field() {
    // End to end: reflect a struct, then run the rename library over its first
    // field (carrying `@rename`) and a bare field.
    let ti = reflect(
        "struct Row:\n    @default(\"ID\")\n    str id\n    str label\n",
        "Row",
    );
    let fields: Vec<FieldInfo> = ti.fields().to_vec();
    let fns = common::parse_fns(RENAME_LIB);
    let mut ev = Evaluator::with_functions(&fns);

    let f0 = Value::Field(Rc::new(fields[0].clone()));
    assert_eq!(
        ev.call("serialized_name", vec![f0], Span::new(0, 0))
            .unwrap(),
        Value::str("ID")
    );
    let f1 = Value::Field(Rc::new(fields[1].clone()));
    assert_eq!(
        ev.call("serialized_name", vec![f1], Span::new(0, 0))
            .unwrap(),
        Value::str("LABEL")
    );
}
