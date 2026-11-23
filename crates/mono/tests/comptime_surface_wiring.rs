//! `comptime for` / `reflect.*` wired to the engine through monomorphization —
//! S2 (#1298, epic #1296). Mono detects a parsed `comptime for f in
//! reflect.type_info(T).fields:` whose `T` resolves to a concrete declaration,
//! and splices the unrolled + `reflect.field`-desugared statements (the E2
//! bridge: unroll #1251 + field desugar #1252). Host-tested over the
//! monomorphized AST; the end-to-end LLVM run is the CI gate.

use zapc_mono::monomorphize;
use zapc_syntax::ast::{Block, Expr, Item, Stmt};
use zapc_syntax::parser::parse_str;

#[track_caller]
fn mono(src: &str) -> Vec<Item> {
    let (m, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex: {lerrs:?}");
    assert!(perrs.is_empty(), "parse: {perrs:?}");
    monomorphize(&m).expect("monomorphize ok").items
}

fn fn_body(items: &[Item], name: &str) -> Block {
    items
        .iter()
        .find_map(|i| match i {
            Item::Function(f) if f.name == name => Some(f.body.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no fn `{name}`"))
}

const SRC: &str = "\
struct Point:
    int64 x
    int64 y
def emit(int64 v):
    pass
def dump(Point p):
    comptime for f in reflect.type_info(Point).fields:
        emit(reflect.field(p, comptime f.index))
";

#[test]
fn comptime_for_over_struct_fields_unrolls_and_desugars_reflect_field() {
    let items = mono(SRC);
    let body = fn_body(&items, "dump");

    // The `comptime for` node is gone — replaced by one statement per field.
    assert!(
        !body
            .stmts
            .iter()
            .any(|s| matches!(s, Stmt::ComptimeFor { .. })),
        "comptime for should be unrolled away"
    );
    assert_eq!(body.stmts.len(), 2, "one emit per field: {:?}", body.stmts);

    // Each is `emit(p.<field>)` — reflect.field desugared to a typed field access.
    let field_of = |s: &Stmt| -> String {
        let Stmt::Expr(Expr::Call { args, .. }) = s else {
            panic!("expected emit(...) call, got {s:?}");
        };
        let Expr::Attr { name, .. } = &args[0].value else {
            panic!("expected p.<field>, got {:?}", args[0].value);
        };
        name.clone()
    };
    assert_eq!(field_of(&body.stmts[0]), "x");
    assert_eq!(field_of(&body.stmts[1]), "y");
}

#[test]
fn a_plain_struct_program_is_unchanged_by_the_wiring() {
    // No `comptime for` → mono leaves the body alone.
    let items =
        mono("struct P:\n    int64 x\ndef f(P p):\n    emit(p.x)\ndef emit(int64 v):\n    pass\n");
    let body = fn_body(&items, "f");
    assert_eq!(body.stmts.len(), 1);
    assert!(!body
        .stmts
        .iter()
        .any(|s| matches!(s, Stmt::ComptimeFor { .. })));
}

#[test]
fn comptime_for_over_enum_variants_unrolls() {
    let src = "\
enum Color:
    Red
    Green
    Blue
def note(int64 i):
    pass
def each():
    comptime for v in reflect.type_info(Color).variants:
        note(comptime v.index)
";
    let items = mono(src);
    let body = fn_body(&items, "each");
    assert!(!body
        .stmts
        .iter()
        .any(|s| matches!(s, Stmt::ComptimeFor { .. })));
    assert_eq!(body.stmts.len(), 3, "one per variant: {:?}", body.stmts);
}

#[test]
fn an_unresolvable_comptime_for_type_is_left_for_the_backstop() {
    // `reflect.type_info(Unknown)` — no such declaration. Mono leaves the node
    // (the lowering backstop / sema deferral reports it); mono itself does not
    // panic or mis-splice.
    let src = "def g():\n    comptime for f in reflect.type_info(Nope).fields:\n        pass\n";
    let items = mono(src);
    let body = fn_body(&items, "g");
    assert!(
        body.stmts
            .iter()
            .any(|s| matches!(s, Stmt::ComptimeFor { .. })),
        "unresolvable comptime for is left intact: {:?}",
        body.stmts
    );
}
