//! `comptime for` unroll engine (#1251): the body is cloned once per field or
//! variant, with comptime operands folded to literals, and — HQ3 — each
//! iteration stays an ordinary outlineable call rather than an inlined body.
//!
//! Intent documentation: unroll counts over 0/1/N, constant folding of
//! `f.index`/`f.name`, HQ3 (N calls, not N inlined bodies), enum `.variants`,
//! comptime user-function folding (the HQ9 `rename_all` path), `materialize`
//! round-trips, and the no-sema AST reflector's shape.

mod common;

use common::{parse_body, parse_enums, parse_expr, parse_fns, parse_structs, sp};
use std::rc::Rc;
use zapc_comptime::{
    materialize, type_info_of_enum, type_info_of_struct, unroll, Env, Evaluator, Value,
};
use zapc_syntax::ast::{render_expr, Stmt};

fn render_stmt(s: &Stmt) -> String {
    match s {
        Stmt::Expr(e) => render_expr(e),
        Stmt::Return { value: Some(e), .. } => format!("return {}", render_expr(e)),
        Stmt::Return { value: None, .. } => "return".to_string(),
        Stmt::Let { name, value, .. } => format!("let {name} = {}", render_expr(value)),
        Stmt::Pass(_) => "pass".to_string(),
        _ => "<stmt>".to_string(),
    }
}

/// Unroll `comptime for <loop_var> in info.fields:` over the first struct in
/// `struct_src`, returning the rendered specialized statements.
fn over_fields(struct_src: &str, loop_var: &str, body_src: &str) -> Vec<String> {
    let defs = parse_structs(struct_src);
    let info = type_info_of_struct(&defs[0]);
    let body = parse_body(body_src);
    let iterable = parse_expr("info.fields");
    let mut base = Env::new();
    base.define("info", Value::Type(Rc::new(info)));
    let mut ev = Evaluator::new();
    let stmts = unroll(loop_var, &iterable, &body, &base, &mut ev)
        .unwrap_or_else(|e| panic!("unroll failed: {e:?}"));
    stmts.iter().map(render_stmt).collect()
}

// ---------------------------------------------------------------------------
// unroll counts (0 / 1 / N) + exact body emission
// ---------------------------------------------------------------------------

#[test]
fn unroll_over_zero_fields_emits_nothing() {
    let out = over_fields("struct Empty:\n    pass\n", "f", "emit(f.index)");
    assert!(
        out.is_empty(),
        "empty struct must unroll to no statements: {out:?}"
    );
}

#[test]
fn unroll_over_one_field() {
    let out = over_fields("struct One:\n    int64 a\n", "f", "emit(f.index)");
    assert_eq!(out, vec!["emit(0)"]);
}

#[test]
fn unroll_over_n_fields_emits_body_exactly_n_times() {
    let out = over_fields(
        "struct P:\n    int64 a\n    int64 b\n    int64 c\n",
        "f",
        "emit(f.index)",
    );
    assert_eq!(out, vec!["emit(0)", "emit(1)", "emit(2)"]);
}

#[test]
fn folds_field_name_and_index() {
    let out = over_fields(
        "struct User:\n    int64 id\n    str name\n",
        "f",
        "send(value, f.name, f.index)",
    );
    assert_eq!(out.len(), 2);
    assert!(out[0].contains("id"), "{out:?}");
    assert!(out[0].contains('0'), "{out:?}");
    assert!(out[1].contains("name"), "{out:?}");
    assert!(out[1].contains('1'), "{out:?}");
    // The runtime operand `value` is preserved, not folded away.
    assert!(out[0].contains("value"), "{out:?}");
}

// ---------------------------------------------------------------------------
// HQ3: unroll ≠ forced inline
// ---------------------------------------------------------------------------

#[test]
fn hq3_each_iteration_is_an_outlineable_call_not_an_inlined_body() {
    // The body calls `emit`; after unrolling there are N *calls* to `emit`, and
    // `emit`'s definition is never inlined/duplicated (the engine only folds
    // comptime operands, it never touches function bodies).
    let defs = parse_structs("struct P:\n    int64 a\n    int64 b\n    int64 c\n");
    let info = type_info_of_struct(&defs[0]);
    let body = parse_body("emit(f.index)");
    let iterable = parse_expr("info.fields");
    let mut base = Env::new();
    base.define("info", Value::Type(Rc::new(info)));
    let mut ev = Evaluator::new();
    let stmts = unroll("f", &iterable, &body, &base, &mut ev).unwrap();
    assert_eq!(stmts.len(), 3);
    for st in &stmts {
        match st {
            Stmt::Expr(zapc_syntax::ast::Expr::Call { callee, args, .. }) => {
                // still a call to `emit`
                assert!(matches!(&**callee, zapc_syntax::ast::Expr::Name(n, _) if n == "emit"));
                // with a single folded integer-literal operand
                assert_eq!(args.len(), 1);
                assert!(matches!(&args[0].value, zapc_syntax::ast::Expr::Int(..)));
            }
            other => panic!("expected an emit() call, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// enum variants
// ---------------------------------------------------------------------------

#[test]
fn unroll_over_enum_variants() {
    let defs = parse_enums("enum Color:\n    RED\n    GREEN\n    BLUE\n");
    let info = type_info_of_enum(&defs[0]);
    let body = parse_body("register(v.name, v.index)");
    let iterable = parse_expr("info.variants");
    let mut base = Env::new();
    base.define("info", Value::Type(Rc::new(info)));
    let mut ev = Evaluator::new();
    let stmts = unroll("v", &iterable, &body, &base, &mut ev).unwrap();
    let rendered: Vec<String> = stmts.iter().map(render_stmt).collect();
    assert_eq!(rendered.len(), 3);
    assert!(
        rendered[0].contains("RED") && rendered[0].contains('0'),
        "{rendered:?}"
    );
    assert!(
        rendered[1].contains("GREEN") && rendered[1].contains('1'),
        "{rendered:?}"
    );
    assert!(
        rendered[2].contains("BLUE") && rendered[2].contains('2'),
        "{rendered:?}"
    );
}

// ---------------------------------------------------------------------------
// HQ9: a comptime user-function call folds to its constant result
// ---------------------------------------------------------------------------

#[test]
fn comptime_user_function_call_folds_to_constant() {
    // `serialized(f)` is reducible at comptime (f is a comptime field), so each
    // iteration folds it to the computed string constant — the `rename_all`
    // transform, computed at compile time, not an inlined runtime call.
    let fns = parse_fns("def serialized(str f):\n    return f.name.upper()\n");
    let defs = parse_structs("struct Row:\n    int64 id\n    str label\n");
    let info = type_info_of_struct(&defs[0]);
    let body = parse_body("emit(value, serialized(f))");
    let iterable = parse_expr("info.fields");
    let mut base = Env::new();
    base.define("info", Value::Type(Rc::new(info)));
    let mut ev = Evaluator::with_functions(&fns);
    let stmts = unroll("f", &iterable, &body, &base, &mut ev).unwrap();
    let rendered: Vec<String> = stmts.iter().map(render_stmt).collect();
    assert_eq!(rendered.len(), 2);
    assert!(rendered[0].contains("ID"), "{rendered:?}");
    assert!(rendered[1].contains("LABEL"), "{rendered:?}");
    // `value` stays a runtime operand; `serialized(...)` is gone (folded).
    assert!(rendered[0].contains("value"), "{rendered:?}");
    assert!(!rendered[0].contains("serialized"), "{rendered:?}");
}

// ---------------------------------------------------------------------------
// error paths
// ---------------------------------------------------------------------------

#[test]
fn non_list_iterable_is_an_error() {
    // `info.name` is a str, not a list.
    let defs = parse_structs("struct S:\n    int64 a\n");
    let info = type_info_of_struct(&defs[0]);
    let body = parse_body("emit(f)");
    let iterable = parse_expr("info.name");
    let mut base = Env::new();
    base.define("info", Value::Type(Rc::new(info)));
    let mut ev = Evaluator::new();
    let e = unroll("f", &iterable, &body, &base, &mut ev).unwrap_err();
    assert!(e.message.contains("comptime list"), "{}", e.message);
}

// ---------------------------------------------------------------------------
// materialize round-trips
// ---------------------------------------------------------------------------

#[test]
fn materialize_scalars() {
    assert_eq!(
        render_expr(&materialize(&Value::Int(42), sp()).unwrap()),
        "42"
    );
    assert_eq!(
        render_expr(&materialize(&Value::Int(-7), sp()).unwrap()),
        "-7"
    );
    assert_eq!(
        render_expr(&materialize(&Value::Bool(true), sp()).unwrap()),
        "True"
    );
    let s = materialize(&Value::str("hi"), sp()).unwrap();
    assert!(render_expr(&s).contains("hi"));
    // INT64_MIN round-trips as -<magnitude>.
    let m = materialize(&Value::Int(i64::MIN), sp()).unwrap();
    assert!(render_expr(&m).contains("9223372036854775808"));
}

#[test]
fn materialize_char_as_constructor() {
    let c = materialize(&Value::Char('A'), sp()).unwrap();
    // char(65) reconstructs 'A' (no char literal surface).
    assert_eq!(render_expr(&c), "char(65)");
}

#[test]
fn materialize_aggregates() {
    let list = Value::list(vec![Value::Int(1), Value::Int(2)]);
    assert_eq!(render_expr(&materialize(&list, sp()).unwrap()), "[1, 2]");
    let tup = Value::tuple(vec![Value::Int(1), Value::str("a")]);
    assert!(render_expr(&materialize(&tup, sp()).unwrap()).contains('1'));
}

#[test]
fn reflection_records_do_not_materialize() {
    use zapc_comptime::{TypeInfo, TypeShape};
    let ti = Value::Type(Rc::new(TypeInfo {
        shape: TypeShape::Tuple { elems: vec![] },
    }));
    assert!(materialize(&ti, sp()).is_none());
    assert!(materialize(&Value::Kind(zapc_comptime::TypeKind::Struct), sp()).is_none());
}

// ---------------------------------------------------------------------------
// the no-sema AST reflector's shape
// ---------------------------------------------------------------------------

#[test]
fn reflector_struct_shape() {
    let defs =
        parse_structs("struct Account:\n    @default(\"anon\")\n    str name\n    int64 age\n");
    let ti = type_info_of_struct(&defs[0]);
    assert_eq!(ti.name(), Some("Account"));
    assert_eq!(ti.kind(), zapc_comptime::TypeKind::Struct);
    let fields = ti.fields();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, "name");
    assert_eq!(fields[0].index, 0);
    assert_eq!(fields[0].attrs.len(), 1);
    assert_eq!(fields[0].attrs[0].name, "default");
    assert_eq!(fields[0].attrs[0].args[0].value, "anon");
    assert_eq!(fields[1].name, "age");
    assert_eq!(fields[1].index, 1);
    assert!(fields[1].attrs.is_empty());
}

#[test]
fn reflector_enum_shape() {
    let defs = parse_enums("enum Shape:\n    Circle(float64 r)\n    Point\n");
    let ti = type_info_of_enum(&defs[0]);
    assert_eq!(ti.name(), Some("Shape"));
    assert_eq!(ti.kind(), zapc_comptime::TypeKind::Enum);
    let vs = ti.variants();
    assert_eq!(vs.len(), 2);
    assert_eq!(vs[0].name, "Circle");
    assert_eq!(vs[0].index, 0);
    assert_eq!(vs[0].fields.len(), 1);
    assert_eq!(vs[0].fields[0].name, "r");
    assert_eq!(vs[1].name, "Point");
    assert!(vs[1].fields.is_empty());
}

// ---------------------------------------------------------------------------
// comptime-constant `if` pruning + `comptime.error` propagation (#1330)
// ---------------------------------------------------------------------------

/// Unroll over the first struct's fields, returning the raw `Result` so a
/// `comptime.error` abort (or any `CtError`) can be asserted.
fn try_over_fields(
    struct_src: &str,
    loop_var: &str,
    body_src: &str,
) -> Result<Vec<Stmt>, zapc_comptime::CtError> {
    let defs = parse_structs(struct_src);
    let info = type_info_of_struct(&defs[0]);
    let body = parse_body(body_src);
    let iterable = parse_expr("info.fields");
    let mut base = Env::new();
    base.define("info", Value::Type(Rc::new(info)));
    let mut ev = Evaluator::new();
    unroll(loop_var, &iterable, &body, &base, &mut ev)
}

#[test]
fn comptime_true_if_keeps_only_the_taken_branch() {
    // `if f.index == 0:` is a comptime-constant guard: it emits the body only for
    // field 0, and the `else` only for the others — the dead arm is dropped.
    let out = over_fields(
        "struct P:\n    int64 a\n    int64 b\n",
        "f",
        "if f.index == 0:\n    emit(f.name)\nelse:\n    skip(f.name)\n",
    );
    assert_eq!(out, vec!["emit(\"a\")", "skip(\"b\")"]);
}

#[test]
fn comptime_false_if_with_no_else_emits_nothing_for_that_field() {
    // Only the matching field emits; the guard prunes the rest to nothing (not to
    // a surviving runtime `if`).
    let out = over_fields(
        "struct P:\n    int64 a\n    int64 b\n    int64 c\n",
        "f",
        "if f.index == 1:\n    emit(f.name)\n",
    );
    assert_eq!(out, vec!["emit(\"b\")"], "only field 1 survives: {out:?}");
}

#[test]
fn a_runtime_if_in_a_comptime_for_body_is_left_intact() {
    // A condition that is not a comptime constant (`value.flag`) is a genuine
    // runtime `if` — it must survive unrolling, once per field, not be pruned.
    let out = over_fields(
        "struct P:\n    int64 a\n    int64 b\n",
        "f",
        "if value.flag:\n    emit(f.name)\n",
    );
    assert_eq!(out.len(), 2, "one runtime if per field: {out:?}");
    assert!(
        out.iter().all(|s| s == "<stmt>"),
        "kept as If stmts: {out:?}"
    );
}

#[test]
fn comptime_error_aborts_unrolling_with_the_authors_message() {
    // An unconditional `comptime.error` aborts, surfacing the author's message as
    // a `UserError` (#1256) rather than being emitted as runtime code.
    let err = try_over_fields(
        "struct P:\n    int64 a\n",
        "f",
        "comptime.error(f.span, \"Ser: unsupported\")\n",
    )
    .expect_err("comptime.error aborts");
    assert_eq!(err.kind, zapc_comptime::CtErrorKind::UserError);
    assert_eq!(err.message, "Ser: unsupported");
}

#[test]
fn a_guarded_comptime_error_fires_only_when_its_branch_is_taken() {
    // Guarded by a comptime-false condition over a single field → the abort is in
    // the dead arm, so unrolling succeeds and emits nothing.
    let ok = try_over_fields(
        "struct P:\n    int64 a\n",
        "f",
        "if f.index == 1:\n    comptime.error(f.span, \"nope\")\n",
    )
    .expect("the abort is in an untaken branch");
    assert!(ok.is_empty(), "nothing emitted: {ok:?}");

    // Over two fields the guard is true for field 1 → the abort fires.
    let err = try_over_fields(
        "struct P:\n    int64 a\n    int64 b\n",
        "f",
        "if f.index == 1:\n    comptime.error(f.span, \"nope\")\n",
    )
    .expect_err("field 1 takes the branch");
    assert_eq!(err.kind, zapc_comptime::CtErrorKind::UserError);
}
