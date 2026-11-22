//! `comptime for` unroll wired to mono (#1251): mono reflects a concrete
//! `struct`/`enum` declaration straight off the AST (the no-sema reflector) and
//! unrolls a body once per field/variant. These exercise the mono-side entry
//! points over constructed AST; the parser `comptime for` surface and the
//! `rewrite_stmt` auto-detection that calls them are the deferred follow-up.

use zapc_syntax::ast::{render_expr, Block, EnumDef, Function, Item, Stmt, StructDef};

fn parse(src: &str) -> Vec<Item> {
    let (m, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    m.items
}

fn first_struct(src: &str) -> StructDef {
    parse(src)
        .into_iter()
        .find_map(|i| match i {
            Item::Struct(s) => Some(s),
            _ => None,
        })
        .expect("a struct")
}

fn first_enum(src: &str) -> EnumDef {
    parse(src)
        .into_iter()
        .find_map(|i| match i {
            Item::Enum(e) => Some(e),
            _ => None,
        })
        .expect("an enum")
}

fn funcs(src: &str) -> Vec<Function> {
    parse(src)
        .into_iter()
        .filter_map(|i| match i {
            Item::Function(f) => Some(f),
            _ => None,
        })
        .collect()
}

/// A loop body: the indented statements of a `def` wrapper.
fn body(body_src: &str) -> Block {
    let indented: String = body_src.lines().map(|l| format!("    {l}\n")).collect();
    let src = format!("def __h():\n{indented}");
    funcs(&src).into_iter().next().unwrap().body
}

fn render(stmts: &[Stmt]) -> Vec<String> {
    stmts
        .iter()
        .map(|s| match s {
            Stmt::Expr(e) => render_expr(e),
            Stmt::Return { value: Some(e), .. } => format!("return {}", render_expr(e)),
            other => format!("{other:?}"),
        })
        .collect()
}

#[test]
fn unroll_struct_fields_over_concrete_declaration() {
    let def = first_struct("struct P:\n    int64 a\n    int64 b\n    int64 c\n");
    let b = body("emit(f.index)");
    let stmts = zapc_mono::unroll_struct_fields(&def, "f", &b, &[]).unwrap();
    assert_eq!(render(&stmts), vec!["emit(0)", "emit(1)", "emit(2)"]);
}

#[test]
fn unroll_empty_struct_emits_nothing() {
    let def = first_struct("struct Empty:\n    pass\n");
    let b = body("emit(f.index)");
    let stmts = zapc_mono::unroll_struct_fields(&def, "f", &b, &[]).unwrap();
    assert!(stmts.is_empty());
}

#[test]
fn unroll_struct_folds_field_names() {
    let def = first_struct("struct User:\n    int64 id\n    str name\n");
    let b = body("put(value, f.name)");
    let stmts = zapc_mono::unroll_struct_fields(&def, "f", &b, &[]).unwrap();
    let r = render(&stmts);
    assert_eq!(r.len(), 2);
    assert!(r[0].contains("id") && r[0].contains("value"), "{r:?}");
    assert!(r[1].contains("name"), "{r:?}");
}

#[test]
fn unroll_enum_variants_over_concrete_declaration() {
    let def = first_enum("enum Color:\n    RED\n    GREEN\n    BLUE\n");
    let b = body("register(v.name, v.index)");
    let stmts = zapc_mono::unroll_enum_variants(&def, "v", &b, &[]).unwrap();
    let r = render(&stmts);
    assert_eq!(r.len(), 3);
    assert!(r[0].contains("RED") && r[0].contains('0'), "{r:?}");
    assert!(r[2].contains("BLUE") && r[2].contains('2'), "{r:?}");
}

// --- #1252: typed reflect.field access ---------------------------------------

#[test]
fn reflect_field_access_resolves_to_typed_field() {
    let def = first_struct("struct Rec:\n    int64 n\n    str name\n    bool flag\n");
    let value =
        zapc_syntax::ast::Expr::Name("value".to_string(), zapc_syntax::span::Span::new(0, 0));
    for (i, name) in ["n", "name", "flag"].iter().enumerate() {
        let acc = zapc_mono::reflect_field_access(&def, &value, i as i64).unwrap();
        assert_eq!(render_expr(&acc), format!("value.{name}"));
    }
}

#[test]
fn reflect_field_access_out_of_range_errors() {
    let def = first_struct("struct S:\n    int64 a\n");
    let value = zapc_syntax::ast::Expr::Name("v".to_string(), zapc_syntax::span::Span::new(0, 0));
    assert!(zapc_mono::reflect_field_access(&def, &value, 3).is_err());
}

#[test]
fn unroll_with_reflect_field_access_end_to_end() {
    // The combined #1251 + #1252 pipeline a derive codec drives.
    let def = first_struct("struct Rec:\n    int64 n\n    str name\n");
    let b = body("emit(reflect.field(value, f.index))");
    let stmts = zapc_mono::unroll_struct_fields_with_access(&def, "f", &b, &[]).unwrap();
    assert_eq!(render(&stmts), vec!["emit(value.n)", "emit(value.name)"]);
}

// --- #1253: kind-switch registry built from a module -------------------------

#[test]
fn type_registry_resolves_container_recursion() {
    let src = "struct P:\n    int64 x\nstruct Holder:\n    list[P] ps\n    dict[str, int64] m\n";
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(lerrs.is_empty() && perrs.is_empty(), "{lerrs:?} {perrs:?}");
    let reg = zapc_mono::type_registry(&module);

    // Holder.ps : list[P] -> elem is the struct P.
    let info = reg
        .resolve_type(
            &zapc_syntax::ast::Type::Named {
                name: "Holder".to_string(),
                args: vec![],
                span: zapc_syntax::span::Span::new(0, 0),
            },
            zapc_syntax::span::Span::new(0, 0),
        )
        .unwrap();
    assert_eq!(info.kind(), zapc_comptime::TypeKind::Struct);
    // ps field's type is a list whose element resolves back to struct P.
    let ps_ty = reg
        .resolve_type(
            &info.fields()[0].decl_ty,
            zapc_syntax::span::Span::new(0, 0),
        )
        .unwrap();
    assert_eq!(ps_ty.kind(), zapc_comptime::TypeKind::List);
    if let zapc_comptime::TypeShape::List { elem } = &ps_ty.shape {
        let p = reg
            .resolve_ty(elem, zapc_syntax::span::Span::new(0, 0))
            .unwrap();
        assert_eq!(p.kind(), zapc_comptime::TypeKind::Struct);
        assert_eq!(p.name(), Some("P"));
    } else {
        panic!("expected list");
    }
}

#[test]
fn unroll_with_comptime_library_function() {
    // HQ9: a registered comptime function folds per field to its constant.
    let def = first_struct("struct Row:\n    int64 id\n    str label\n");
    let fs = funcs("def serialized(str f):\n    return f.name.upper()\n");
    let b = body("emit(serialized(f))");
    let stmts = zapc_mono::unroll_struct_fields(&def, "f", &b, &fs).unwrap();
    let r = render(&stmts);
    assert_eq!(r.len(), 2);
    assert!(r[0].contains("ID"), "{r:?}");
    assert!(r[1].contains("LABEL"), "{r:?}");
    assert!(
        !r[0].contains("serialized"),
        "folded, not a runtime call: {r:?}"
    );
}
