//! Value model round-trips (#1250): each Tier-B1 value type constructed and read
//! back, including empty and nested aggregates and a list of reflection records.
//!
//! Intent documentation: this pins the surface of [`Value`] so a later change
//! that drops a kind, mis-nests an aggregate, or breaks structural equality
//! fails here.

mod common;

use common::{ok, sp};
use std::rc::Rc;
use zapc_comptime::{FieldInfo, TypeInfo, TypeShape, Value};
use zapc_sema_types::Ty;

#[test]
fn int_roundtrip() {
    assert_eq!(ok("0"), Value::Int(0));
    assert_eq!(ok("42"), Value::Int(42));
    assert_eq!(ok("-7"), Value::Int(-7));
    assert_eq!(ok("9223372036854775807"), Value::Int(i64::MAX));
    assert_eq!(ok("-9223372036854775808"), Value::Int(i64::MIN));
}

#[test]
fn int_radix_and_separators() {
    assert_eq!(ok("0xFF"), Value::Int(255));
    assert_eq!(ok("0o17"), Value::Int(15));
    assert_eq!(ok("0b1010"), Value::Int(10));
    assert_eq!(ok("1_000_000"), Value::Int(1_000_000));
    assert_eq!(ok("0xDE_AD"), Value::Int(0xDEAD));
}

#[test]
fn bool_roundtrip() {
    assert_eq!(ok("True"), Value::Bool(true));
    assert_eq!(ok("False"), Value::Bool(false));
}

#[test]
fn str_roundtrip() {
    assert_eq!(ok("\"\""), Value::str(""));
    assert_eq!(ok("\"hello\""), Value::str("hello"));
    assert_eq!(ok("\"über\""), Value::str("über"));
}

#[test]
fn char_roundtrip_via_index_and_chr() {
    // No char literal surface — chars come from indexing a string or `chr`.
    assert_eq!(ok("\"abc\"[0]"), Value::Char('a'));
    assert_eq!(ok("\"abc\"[-1]"), Value::Char('c'));
    assert_eq!(ok("chr(65)"), Value::Char('A'));
    assert_eq!(ok("char(0x263A)"), Value::Char('☺'));
}

#[test]
fn empty_list_and_tuple() {
    assert_eq!(ok("[]"), Value::list(vec![]));
    assert_eq!(ok("()"), Value::tuple(vec![]));
}

#[test]
fn list_roundtrip() {
    assert_eq!(
        ok("[1, 2, 3]"),
        Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
    assert_eq!(
        ok("[\"a\", \"b\"]"),
        Value::list(vec![Value::str("a"), Value::str("b")])
    );
}

#[test]
fn tuple_heterogeneous_roundtrip() {
    assert_eq!(
        ok("(1, \"two\", True)"),
        Value::tuple(vec![Value::Int(1), Value::str("two"), Value::Bool(true)])
    );
}

#[test]
fn nested_tuples_and_lists() {
    assert_eq!(
        ok("((1, 2), (3, 4))"),
        Value::tuple(vec![
            Value::tuple(vec![Value::Int(1), Value::Int(2)]),
            Value::tuple(vec![Value::Int(3), Value::Int(4)]),
        ])
    );
    assert_eq!(
        ok("[[1], [], [2, 3]]"),
        Value::list(vec![
            Value::list(vec![Value::Int(1)]),
            Value::list(vec![]),
            Value::list(vec![Value::Int(2), Value::Int(3)]),
        ])
    );
}

#[test]
fn list_of_records_roundtrips_through_a_value() {
    // A `list` whose elements are E1 reflection records (built by hand here;
    // the resolver-driven path is exercised in reflection.rs).
    let field = |name: &str, index: usize| FieldInfo {
        name: name.to_string(),
        ty: Ty::Int64,
        decl_ty: zapc_syntax::ast::Type::Named {
            name: "int64".to_string(),
            args: vec![],
            span: sp(),
        },
        index,
        attrs: vec![],
        span: sp(),
    };
    let xs = Value::list(vec![
        Value::Field(Rc::new(field("a", 0))),
        Value::Field(Rc::new(field("b", 1))),
    ]);
    // A list of records is a first-class comptime value; its length and element
    // kinds are preserved.
    if let Value::List(items) = &xs {
        assert_eq!(items.len(), 2);
        assert!(matches!(items[0], Value::Field(_)));
    } else {
        panic!("expected a list");
    }
    // And it compares equal to itself (records by identity, list structurally).
    assert_eq!(xs, xs.clone());
    let _ = TypeInfo {
        shape: TypeShape::Tuple { elems: vec![] },
    };
}

#[test]
fn type_names_are_stable() {
    assert_eq!(ok("1").type_name(), "int64");
    assert_eq!(ok("True").type_name(), "bool");
    assert_eq!(ok("\"x\"").type_name(), "str");
    assert_eq!(ok("\"x\"[0]").type_name(), "char");
    assert_eq!(ok("[1]").type_name(), "list");
    assert_eq!(ok("(1, 2)").type_name(), "tuple");
}
