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
fn a_generic_generator_unrolls_per_instantiation_via_cur_subst() {
    // The real `@derive` shape: a generator generic over `T` with a
    // `comptime for … in reflect.type_info(T).fields:` body, called at a concrete
    // type. Mono specializes `ser[Point]`, and `T` in `reflect.type_info(T)`
    // resolves through the active substitution to `Point`, so the loop unrolls
    // per instantiation (#1291, E3b generic-T resolution).
    let src = "\
struct Point:
    int64 x
    int64 y
def emit(int64 v):
    pass
def ser[T](T p):
    comptime for f in reflect.type_info(T).fields:
        emit(reflect.field(p, comptime f.index))
def main():
    ser(Point(1, 2))
";
    let items = mono(src);
    // The specialized `ser[Point]` body must be unrolled (no ComptimeFor left),
    // with one `emit(p.<field>)` per field. Find it among the functions.
    let specialized = items.iter().find_map(|i| match i {
        Item::Function(f) if f.name.contains("ser") && f.name != "main" => Some(f.body.clone()),
        _ => None,
    });
    let body = specialized.expect("a specialized `ser` instantiation");
    assert!(
        !body
            .stmts
            .iter()
            .any(|s| matches!(s, Stmt::ComptimeFor { .. })),
        "generic generator's comptime for should be unrolled: {:?}",
        body.stmts
    );
    assert_eq!(body.stmts.len(), 2, "one emit per field: {:?}", body.stmts);
    for (i, want) in ["x", "y"].iter().enumerate() {
        let Stmt::Expr(Expr::Call { args, .. }) = &body.stmts[i] else {
            panic!("expected emit(...) call, got {:?}", body.stmts[i]);
        };
        let Expr::Attr { name, .. } = &args[0].value else {
            panic!("expected p.<field>, got {:?}", args[0].value);
        };
        assert_eq!(name, want);
    }
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

// --- #1324: a generic free function parameter named `self` is an ordinary
// parameter (not a method receiver) — its type is substituted per instantiation,
// and type-argument inference matches the call argument against it. ---

#[test]
fn a_generic_free_fn_self_param_is_substituted_and_inferred() {
    // `g[T](T self)` called as `g(Point(..))` must specialize with `self: Point`
    // (T substituted) and infer T from the argument — not keep `self: T` or fail
    // with "cannot infer type argument". Reflect is not involved; this isolates
    // the self-param handling in `rewrite_fn_inner` + `infer_fn_targs`.
    let src = "\
struct Point:
    int64 x
    int64 y
def g[T](T self) -> T:
    return self
def main():
    auto p = g(Point(1, 2))
";
    let items = mono(src);
    // The specialized `g$Point` must have its single parameter typed `Point`.
    let f = items
        .iter()
        .find_map(|i| match i {
            Item::Function(f) if f.name.contains("g") && f.name != "main" => Some(f),
            _ => None,
        })
        .expect("a specialized `g` instantiation");
    assert_eq!(f.params.len(), 1);
    assert_eq!(f.params[0].name, "self");
    match f.params[0].ty.as_ref().expect("self has a type") {
        zapc_syntax::ast::Type::Named { name, .. } => {
            assert_eq!(name, "Point", "self's type is substituted T -> Point");
        }
        other => panic!("expected Named Point, got {other:?}"),
    }
}

// --- #1686: when a comptime-`if` is folded to a taken branch that
// unconditionally diverges (an early `return`/`raise`), the statements written
// after the `if` ran only on the fall-through path and are now statically
// unreachable in this specialization — they must NOT be spliced into the taken
// path. A branch that can fall through (e.g. the empty false-specialization, or
// a plain assignment) keeps them. ---

/// Find the single specialized instantiation of `base` (`base$...`), skipping
/// `main` and the still-generic template.
#[track_caller]
fn specialization(items: &[Item], base: &str) -> Block {
    let prefix = format!("{base}$");
    items
        .iter()
        .find_map(|i| match i {
            Item::Function(f) if f.name.starts_with(&prefix) => Some(f.body.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no specialization of `{base}`"))
}

#[test]
fn folded_early_return_drops_a_trailing_raise() {
    // The issue's exact shape: the supported-type specialization folds the
    // `if` to `return value`; the `raise` that followed it must be gone, so the
    // body is a lone `return` (not `return` then a dead `raise` the backend
    // would otherwise lower into the taken path).
    let src = "\
def resolve[T](T value) -> T:
    if reflect.type_info(T).name == \"str\":
        return value
    raise ValueError(\"unsupported\")
def main():
    auto a = resolve(\"ok\")
";
    let body = specialization(&mono(src), "resolve");
    assert_eq!(
        body.stmts.len(),
        1,
        "only the folded `return` survives: {:?}",
        body.stmts
    );
    assert!(
        matches!(body.stmts[0], Stmt::Return { value: Some(_), .. }),
        "the sole statement is the early `return`: {:?}",
        body.stmts[0]
    );
}

#[test]
fn folded_early_return_drops_a_trailing_side_effect() {
    // Same ordering hazard with a side-effecting statement (not a `raise`) after
    // the early return: it is also unreachable and must be dropped.
    let src = "\
def log(str m):
    pass
def resolve[T](T value) -> T:
    if reflect.type_info(T).name == \"str\":
        return value
    log(\"fallback\")
    raise ValueError(\"unsupported\")
def main():
    auto a = resolve(\"ok\")
";
    let body = specialization(&mono(src), "resolve");
    assert_eq!(
        body.stmts.len(),
        1,
        "the trailing `log(...)` and `raise` are both unreachable: {:?}",
        body.stmts
    );
    assert!(matches!(body.stmts[0], Stmt::Return { .. }));
}

#[test]
fn false_specialization_still_reaches_the_fallback() {
    // The unsupported-type specialization folds the `if` away to nothing (empty
    // false branch, which can fall through), so the fallback `raise` after it
    // MUST remain — dropping trailing statements only applies to a diverging
    // taken branch.
    let src = "\
def resolve[T](T value) -> T:
    if reflect.type_info(T).name == \"str\":
        return value
    raise ValueError(\"unsupported\")
def main():
    auto b = resolve(5)
";
    let body = specialization(&mono(src), "resolve");
    assert_eq!(
        body.stmts.len(),
        1,
        "the if folds to nothing; only the fallback `raise` is left: {:?}",
        body.stmts
    );
    assert!(
        matches!(body.stmts[0], Stmt::Raise { .. }),
        "the fallback `raise` survives: {:?}",
        body.stmts[0]
    );
}

#[test]
fn both_specializations_coexist_with_correct_control_flow() {
    // The true and false specializations are generated from one template in the
    // same program: the supported one returns, the unsupported one raises.
    let src = "\
def resolve[T](T value) -> T:
    if reflect.type_info(T).name == \"str\":
        return value
    raise ValueError(\"unsupported\")
def main():
    auto a = resolve(\"ok\")
    auto b = resolve(5)
";
    let items = mono(src);
    let str_fn = items.iter().find_map(|i| match i {
        Item::Function(f) if f.name == "resolve$str" => Some(f.body.clone()),
        _ => None,
    });
    let int_fn = items.iter().find_map(|i| match i {
        Item::Function(f) if f.name == "resolve$int64" => Some(f.body.clone()),
        _ => None,
    });
    assert!(matches!(
        str_fn.expect("resolve$str").stmts.as_slice(),
        [Stmt::Return { .. }]
    ));
    assert!(matches!(
        int_fn.expect("resolve$int64").stmts.as_slice(),
        [Stmt::Raise { .. }]
    ));
}

#[test]
fn a_non_diverging_folded_branch_keeps_trailing_statements() {
    // The taken branch does not diverge (a plain assignment), so control falls
    // through it and the statement after the `if` is still reachable — it must
    // be kept. This guards the fix against over-eager dropping.
    let src = "\
def resolve[T](T value) -> int64:
    int64 n = 0
    if reflect.type_info(T).name == \"str\":
        n = 1
    return n
def main():
    auto a = resolve(\"ok\")
";
    let body = specialization(&mono(src), "resolve");
    // `int64 n = 0`, folded `n = 1` (branch body), then `return n`.
    assert_eq!(
        body.stmts.len(),
        3,
        "fall-through branch keeps the trailing `return`: {:?}",
        body.stmts
    );
    assert!(
        matches!(body.stmts.last(), Some(Stmt::Return { .. })),
        "the trailing `return` survives a non-diverging folded branch: {:?}",
        body.stmts
    );
}

#[test]
fn an_if_else_whose_arms_all_diverge_drops_trailing_statements() {
    // A total `if/else` where both arms diverge is itself diverging: anything
    // after it is unreachable and must be dropped once the chain is folded in.
    let src = "\
def resolve[T](T value) -> T:
    if reflect.type_info(T).name == \"str\":
        return value
    else:
        raise ValueError(\"x\")
    raise ValueError(\"unreachable\")
def main():
    auto a = resolve(\"ok\")
";
    let body = specialization(&mono(src), "resolve");
    assert_eq!(
        body.stmts.len(),
        1,
        "the trailing unreachable `raise` is dropped: {:?}",
        body.stmts
    );
    assert!(matches!(body.stmts[0], Stmt::Return { .. }));
}

#[test]
fn a_runtime_if_in_a_specialization_is_left_intact() {
    // Negative control: a non-comptime condition inside a specialization is NOT
    // folded, so it stays a runtime `if` and no trailing statement is dropped.
    let src = "\
def resolve[T](T value, bool flag) -> T:
    if flag:
        return value
    raise ValueError(\"unsupported\")
def main():
    auto a = resolve(\"ok\", true)
";
    let body = specialization(&mono(src), "resolve");
    assert_eq!(
        body.stmts.len(),
        2,
        "runtime `if` is preserved with its fallthrough `raise`: {:?}",
        body.stmts
    );
    assert!(matches!(body.stmts[0], Stmt::If { .. }));
    assert!(matches!(body.stmts[1], Stmt::Raise { .. }));
}

#[test]
fn a_non_generic_fn_is_never_pruned() {
    // Negative control: with no active substitution (an ordinary, non-generic
    // function), the comptime-`if` pruning path is gated off entirely, so even a
    // literal-true `if` stays a runtime `if` and nothing is dropped.
    let src = "\
def f() -> int64:
    if true:
        return 1
    return 2
";
    let body = fn_body(&mono(src), "f");
    assert_eq!(
        body.stmts.len(),
        2,
        "non-generic body is untouched by #1394/#1686 pruning: {:?}",
        body.stmts
    );
    assert!(matches!(body.stmts[0], Stmt::If { .. }));
}
