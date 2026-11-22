//! Compile-time type introspection — `reflect.type_info(T)` (read-only).
//!
//! Issue #1257, epic #1248 phase **E1**; design resolved in Discussion #1243.
//! E1 is the read-only introspection layer: a `TypeInfo` value describing a
//! type's *shape* (`kind` / `name` / `fields` / `variants`), nominal for user
//! struct/enum and structural for the built-in containers. There is **no
//! comptime evaluator, no codegen, no runtime materialization** yet — those are
//! later phases (#1250/#1251/#1252/#1253). So these tests assert over the model
//! host-side, through [`zapc_sema::analyze_reflect`], the way the E2 evaluator
//! will eventually drive it.
//!
//! The suite is intent documentation, not a minimum: it pins the shape of every
//! kind, declaration-order preservation (#1066), field/variant metadata
//! (name / index / resolved type / declared type / attrs / span), nominal vs
//! structural naming, and the exact "not available for introspection"
//! diagnostic (with its span) for an unsupported type — so a later change that
//! silently drops a field, reorders fields, or panics on an opaque type fails
//! here. These run on the host (parse + sema), no LLVM.

use zapc_sema::{analyze_reflect, Reflector, Ty, TypeKind, TypeShape};
use zapc_syntax::ast::Type;
use zapc_syntax::span::Span;

/// Parse + analyze `src` into a [`Reflector`], asserting the program is free of
/// lex / parse / sema diagnostics (every fixture here is well-typed).
#[track_caller]
fn reflector(src: &str) -> Reflector {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    let (terrs, _warnings, r) = analyze_reflect(&module);
    assert!(
        terrs.is_empty(),
        "unexpected sema errors: {:?}",
        terrs
            .iter()
            .map(|e| (e.code.to_string(), e.message.clone()))
            .collect::<Vec<_>>()
    );
    r
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

#[test]
fn struct_zero_fields() {
    let r = reflector("struct Empty:\n    pass\n");
    let ti = r.type_info_by_name("Empty").expect("introspectable");
    assert_eq!(ti.kind(), TypeKind::Struct);
    assert_eq!(ti.name(), Some("Empty"));
    assert!(ti.fields().is_empty(), "a fieldless struct has no fields");
    assert!(ti.variants().is_empty());
}

#[test]
fn struct_one_field() {
    let r = reflector("struct Wrap:\n    int64 value\n");
    let ti = r.type_info_by_name("Wrap").unwrap();
    assert_eq!(ti.kind(), TypeKind::Struct);
    assert_eq!(ti.name(), Some("Wrap"));
    let f = ti.fields();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].name, "value");
    assert_eq!(f[0].index, 0);
    assert_eq!(f[0].ty, Ty::Int64);
    assert!(f[0].attrs.is_empty());
}

#[test]
fn struct_many_fields_declaration_order_not_alphabetical() {
    // Source order is z, a, m — deliberately not alphabetical and not sorted by
    // type — so the test fails if fields are ever reordered (#1066).
    let r = reflector("struct Rec:\n    int64 zeta\n    str alpha\n    bool mid\n");
    let ti = r.type_info_by_name("Rec").unwrap();
    let f = ti.fields();
    assert_eq!(f.len(), 3);
    assert_eq!(
        f.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
        ["zeta", "alpha", "mid"]
    );
    // Index is 0-based and contiguous, matching declaration order.
    assert_eq!(f.iter().map(|x| x.index).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(f[0].ty, Ty::Int64);
    assert_eq!(f[1].ty, Ty::Str);
    assert_eq!(f[2].ty, Ty::Bool);
}

#[test]
fn struct_field_each_primitive_type() {
    let r = reflector(
        "struct Prims:\n    int64 i\n    int32 small\n    uint8 ubyte\n    usize sz\n    \
         float64 f\n    float32 fsmall\n    bool b\n    str s\n    char c\n    bytes by\n    \
         bigint big\n",
    );
    let ti = r.type_info_by_name("Prims").unwrap();
    let got: Vec<(&str, &Ty)> = ti
        .fields()
        .iter()
        .map(|f| (f.name.as_str(), &f.ty))
        .collect();
    assert_eq!(
        got,
        vec![
            ("i", &Ty::Int64),
            ("small", &Ty::Int32),
            ("ubyte", &Ty::Uint8),
            ("sz", &Ty::Usize),
            ("f", &Ty::Float64),
            ("fsmall", &Ty::Float32),
            ("b", &Ty::Bool),
            ("s", &Ty::Str),
            ("c", &Ty::Char),
            ("by", &Ty::Bytes),
            ("big", &Ty::Bigint),
        ]
    );
}

#[test]
fn struct_declared_fields_only_methods_excluded() {
    // Methods are not data fields: only `x`/`y` are reported.
    let r = reflector(
        "struct Point:\n    int64 x\n    int64 y\n\n    def norm(self) -> int64:\n        return self.x\n",
    );
    let ti = r.type_info_by_name("Point").unwrap();
    assert_eq!(
        ti.fields()
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        ["x", "y"]
    );
}

#[test]
fn struct_value_struct_is_struct_kind() {
    // A `value struct` is still introspected as a nominal struct.
    let r = reflector("value struct V:\n    int64 a\n    int64 b\n");
    let ti = r.type_info_by_name("V").unwrap();
    assert_eq!(ti.kind(), TypeKind::Struct);
    assert_eq!(ti.name(), Some("V"));
    assert_eq!(ti.fields().len(), 2);
}

// ---------------------------------------------------------------------------
// Field attributes (HQ9) — captured as structured data, empty when absent
// ---------------------------------------------------------------------------

#[test]
fn struct_field_attributes_captured() {
    let r = reflector(
        "struct Account:\n    @default(100)\n    int64 balance\n    \
         @validate(max_len=8)\n    str name\n    int64 age\n",
    );
    let ti = r.type_info_by_name("Account").unwrap();
    let f = ti.fields();

    // `@default(100)` — one positional arg rendered as its lexeme.
    assert_eq!(f[0].name, "balance");
    assert_eq!(f[0].attrs.len(), 1);
    assert_eq!(f[0].attrs[0].name, "default");
    assert_eq!(f[0].attrs[0].args.len(), 1);
    assert_eq!(f[0].attrs[0].args[0].name, None);
    assert_eq!(f[0].attrs[0].args[0].value, "100");

    // `@validate(max_len=8)` — one keyword arg.
    assert_eq!(f[1].name, "name");
    assert_eq!(f[1].attrs.len(), 1);
    assert_eq!(f[1].attrs[0].name, "validate");
    assert_eq!(f[1].attrs[0].args[0].name.as_deref(), Some("max_len"));
    assert_eq!(f[1].attrs[0].args[0].value, "8");

    // A field with no decorators has an empty attr list.
    assert_eq!(f[2].name, "age");
    assert!(f[2].attrs.is_empty());
}

// ---------------------------------------------------------------------------
// Enums (HQ8)
// ---------------------------------------------------------------------------

#[test]
fn enum_unit_variants() {
    let r = reflector("enum Color:\n    RED\n    GREEN\n    BLUE\n");
    let ti = r.type_info_by_name("Color").unwrap();
    assert_eq!(ti.kind(), TypeKind::Enum);
    assert_eq!(ti.name(), Some("Color"));
    let v = ti.variants();
    assert_eq!(
        v.iter()
            .map(|x| (x.name.as_str(), x.index))
            .collect::<Vec<_>>(),
        [("RED", 0), ("GREEN", 1), ("BLUE", 2)]
    );
    assert!(
        v.iter().all(|x| x.fields.is_empty()),
        "unit variants carry no payload"
    );
    assert!(ti.fields().is_empty(), "an enum has no struct fields");
}

#[test]
fn enum_payload_variants() {
    let r = reflector("enum Shape:\n    Circle(float64 r)\n    Rect(float64 w, float64 h)\n");
    let ti = r.type_info_by_name("Shape").unwrap();
    let v = ti.variants();
    assert_eq!(v.len(), 2);

    assert_eq!(v[0].name, "Circle");
    assert_eq!(v[0].index, 0);
    assert_eq!(v[0].fields.len(), 1);
    assert_eq!(v[0].fields[0].name, "r");
    assert_eq!(v[0].fields[0].index, 0);
    assert_eq!(v[0].fields[0].ty, Ty::Float64);

    assert_eq!(v[1].name, "Rect");
    assert_eq!(v[1].index, 1);
    assert_eq!(
        v[1].fields
            .iter()
            .map(|f| (f.name.as_str(), f.index, &f.ty))
            .collect::<Vec<_>>(),
        [("w", 0, &Ty::Float64), ("h", 1, &Ty::Float64)]
    );
}

#[test]
fn enum_mixed_unit_and_payload_variants() {
    // A payload-carrying variant makes the whole enum a sum type; a unit variant
    // alongside it is an arity-0 payload (tag only).
    let r = reflector("enum Msg:\n    Close\n    Text(str body)\n    Ping\n");
    let ti = r.type_info_by_name("Msg").unwrap();
    let v = ti.variants();
    assert_eq!(
        v.iter()
            .map(|x| (x.name.as_str(), x.fields.len()))
            .collect::<Vec<_>>(),
        [("Close", 0), ("Text", 1), ("Ping", 0)]
    );
    assert_eq!(v[1].fields[0].name, "body");
    assert_eq!(v[1].fields[0].ty, Ty::Str);
}

// ---------------------------------------------------------------------------
// Tuples (HQ8) — positional, structural, no nominal name
// ---------------------------------------------------------------------------

/// Reflect a single-field probe struct's one declared field type. Lets a test
/// pin container / tuple / primitive kinds over a *real parsed* type expression
/// (with real spans), exactly as E2 will reflect a field's type.
#[track_caller]
fn field_type_info(field_decl: &str) -> zapc_sema::TypeInfo {
    let src = format!("struct Probe:\n    {field_decl} probe\n");
    let r = reflector(&src);
    let probe = r.type_info_by_name("Probe").unwrap();
    let f = probe.fields();
    assert_eq!(f.len(), 1);
    r.type_info(&f[0].decl_ty).unwrap_or_else(|e| {
        panic!(
            "`{field_decl}` should be introspectable, got: {}",
            e.message
        )
    })
}

#[test]
fn tuple_empty() {
    let ti = field_type_info("()");
    assert_eq!(ti.kind(), TypeKind::Tuple);
    assert_eq!(ti.name(), None, "a tuple is structural, no nominal name");
    match &ti.shape {
        TypeShape::Tuple { elems } => assert!(elems.is_empty()),
        other => panic!("expected tuple, got {other:?}"),
    }
}

#[test]
fn tuple_nested() {
    let ti = field_type_info("(int64, (str, bool))");
    match &ti.shape {
        TypeShape::Tuple { elems } => {
            assert_eq!(elems.len(), 2);
            assert_eq!(elems[0], Ty::Int64);
            assert_eq!(elems[1], Ty::Tuple(vec![Ty::Str, Ty::Bool]));
        }
        other => panic!("expected tuple, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

#[test]
fn primitive_by_name() {
    let r = reflector("struct S:\n    pass\n");
    for (name, canonical) in [
        ("int", "int64"),
        ("int64", "int64"),
        ("int32", "int32"),
        ("bool", "bool"),
        ("str", "str"),
        ("char", "char"),
        ("float", "float64"),
        ("float64", "float64"),
        ("bigint", "bigint"),
    ] {
        let ti = r
            .type_info_by_name(name)
            .unwrap_or_else(|e| panic!("{name}: {}", e.message));
        assert_eq!(ti.kind(), TypeKind::Primitive, "{name}");
        // A primitive carries its canonical spelling as a name.
        assert_eq!(ti.name(), Some(canonical), "{name}");
    }
}

// ---------------------------------------------------------------------------
// Containers — list / dict / optional (structural)
// ---------------------------------------------------------------------------

#[test]
fn list_of_primitive() {
    let ti = field_type_info("list[int64]");
    assert_eq!(ti.kind(), TypeKind::List);
    assert_eq!(ti.name(), None);
    match &ti.shape {
        TypeShape::List { elem } => assert_eq!(*elem, Ty::Int64),
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn dict_key_value() {
    let ti = field_type_info("dict[str, int64]");
    assert_eq!(ti.kind(), TypeKind::Dict);
    assert_eq!(ti.name(), None);
    match &ti.shape {
        TypeShape::Dict { key, value } => {
            assert_eq!(*key, Ty::Str);
            assert_eq!(*value, Ty::Int64);
        }
        other => panic!("expected dict, got {other:?}"),
    }
}

#[test]
fn optional_element_recovered() {
    // The resolved `Ty` erases `Optional`'s element; E1 recovers it from the
    // declared `Optional[T]` so the element type is reported correctly.
    let ti = field_type_info("Optional[int64]");
    assert_eq!(ti.kind(), TypeKind::Optional);
    assert_eq!(ti.name(), None);
    match &ti.shape {
        TypeShape::Optional { elem } => assert_eq!(*elem, Ty::Int64),
        other => panic!("expected optional, got {other:?}"),
    }
}

#[test]
fn nested_list_of_dict() {
    let ti = field_type_info("list[dict[str, int64]]");
    match &ti.shape {
        TypeShape::List { elem } => {
            assert_eq!(*elem, Ty::Dict(Box::new(Ty::Str), Box::new(Ty::Int64)));
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn list_of_user_struct() {
    // `list[Point]` where `Point` is a user struct: structural list whose element
    // is the nominal `Named("Point")`. Re-introspecting the element yields the
    // struct (one-level, lazily recursive — HQ1a).
    let src = "struct Point:\n    int64 x\n    int64 y\n\nstruct Poly:\n    list[Point] pts\n";
    let r = reflector(src);
    let poly = r.type_info_by_name("Poly").unwrap();
    let pts = &poly.fields()[0];
    let list_ti = r.type_info(&pts.decl_ty).unwrap();
    match &list_ti.shape {
        TypeShape::List { elem } => assert_eq!(*elem, Ty::Named("Point".into())),
        other => panic!("expected list, got {other:?}"),
    }
    // The element type is itself introspectable as the nominal struct.
    let point_ti = r.type_info_by_name("Point").unwrap();
    assert_eq!(point_ti.kind(), TypeKind::Struct);
    assert_eq!(point_ti.name(), Some("Point"));
    assert_eq!(point_ti.fields().len(), 2);
}

// ---------------------------------------------------------------------------
// Nominal vs structural
// ---------------------------------------------------------------------------

#[test]
fn nominal_kinds_carry_a_name_structural_do_not() {
    let r = reflector(
        "struct S:\n    int64 a\n\nenum E:\n    A\n    B\n\nstruct Box:\n    \
         list[int64] xs\n    dict[str, int64] m\n    Optional[str] o\n    (int64, str) t\n",
    );
    // Nominal: struct + enum carry their type name.
    assert_eq!(r.type_info_by_name("S").unwrap().name(), Some("S"));
    assert_eq!(r.type_info_by_name("E").unwrap().name(), Some("E"));

    // Structural: every container/tuple field type has no nominal name.
    let b = r.type_info_by_name("Box").unwrap();
    for f in b.fields() {
        let ti = r.type_info(&f.decl_ty).unwrap();
        assert_eq!(ti.name(), None, "`{}` should be structural", f.name);
    }
}

// ---------------------------------------------------------------------------
// Negative — unsupported / opaque types (never a panic)
// ---------------------------------------------------------------------------

#[test]
fn unsupported_container_is_a_diagnostic_not_a_panic() {
    // `set[int]` is a real container but outside E1's described kinds.
    let src = "struct Probe:\n    set[int64] s\n";
    let r = reflector(src);
    let probe = r.type_info_by_name("Probe").unwrap();
    let err = r
        .type_info(&probe.fields()[0].decl_ty)
        .expect_err("set[int] is not introspectable in E1");
    assert_eq!(
        err.message,
        "`set[int64]` is not available for introspection"
    );
}

#[test]
fn unsupported_unknown_type_name_is_a_diagnostic() {
    let r = reflector("struct S:\n    pass\n");
    let err = r
        .type_info_by_name("NoSuchType")
        .expect_err("an unknown type is not introspectable");
    assert!(
        err.message.contains("is not available for introspection"),
        "got: {}",
        err.message
    );
}

#[test]
fn unsupported_diagnostic_is_anchored_at_the_type_span() {
    // The diagnostic points at the type exactly as written, not a default span,
    // so a later phase can surface it at the user's source location.
    let r = reflector("struct S:\n    pass\n");
    let at = Span::new(42, 50);
    let err = r
        .type_info(&Type::Named {
            name: "Widget".to_string(),
            args: Vec::new(),
            span: at,
        })
        .expect_err("unknown type");
    assert_eq!(err.span, at);
    assert_eq!(err.message, "`Widget` is not available for introspection");
}

#[test]
fn unsupported_function_and_channel_types() {
    // A function-typed and a channel-typed field are both outside E1.
    let r = reflector("struct Probe:\n    channel[int64] ch\n");
    let probe = r.type_info_by_name("Probe").unwrap();
    assert!(r.type_info(&probe.fields()[0].decl_ty).is_err());

    // A bare function type, constructed directly.
    let fn_ty = Type::Func {
        params: vec![Type::Named {
            name: "int64".into(),
            args: vec![],
            span: Span::default(),
        }],
        ret: Some(Box::new(Type::Named {
            name: "int64".into(),
            args: vec![],
            span: Span::default(),
        })),
        span: Span::new(1, 2),
    };
    let err = reflector("struct S:\n    pass\n")
        .type_info(&fn_ty)
        .unwrap_err();
    assert_eq!(err.span, Span::new(1, 2));
}

// ---------------------------------------------------------------------------
// The declared type is retained alongside the resolved type
// ---------------------------------------------------------------------------

#[test]
fn field_retains_declared_and_resolved_type() {
    // `int` resolves to the canonical `Int64`, while the declared type keeps the
    // spelling the author wrote — both are available on the field.
    let r = reflector("struct S:\n    int count\n");
    let ti = r.type_info_by_name("S").unwrap();
    let f = &ti.fields()[0];
    assert_eq!(f.ty, Ty::Int64);
    match &f.decl_ty {
        Type::Named { name, .. } => assert_eq!(name, "int"),
        other => panic!("expected a named declared type, got {other:?}"),
    }
}
