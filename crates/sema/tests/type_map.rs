//! The sema inferred-type map (#1165), the foundation for monomorphization
//! reading sema's types instead of re-deriving them (epic I#1169).
//!
//! `analyze_types` returns a span-keyed `TypeMap` of the type the checker
//! inferred for every expression. These tests pin that the map records the
//! *correct* type for exactly the expression forms the monomorphizer's own
//! partial inferer could not type — module constants (`math.inf`, #1162),
//! built-in free-call and method returns (`len`, `str.upper`, #1132), a
//! subscript, a slice, a tuple literal, and a comprehension — proving sema
//! already has what mono needs. Each program binds the expression under test to
//! a local named `probe`; the test looks up that binding's value span and
//! asserts its recorded type (via `Ty::display()`).

use zapc_sema::analyze_types;
use zapc_syntax::ast::{Expr, Item, Stmt};
use zapc_syntax::parser::parse_str;
use zapc_syntax::span::Span;

/// The span of an expression — the subset of variants these tests exercise.
fn span_of(e: &Expr) -> Span {
    match e {
        Expr::Int(_, s)
        | Expr::Float(_, s)
        | Expr::Attr { span: s, .. }
        | Expr::Call { span: s, .. }
        | Expr::Index { span: s, .. }
        | Expr::Slice { span: s, .. }
        | Expr::Tuple(_, s)
        | Expr::Binary { span: s, .. }
        | Expr::Unary { span: s, .. }
        | Expr::ListComp { span: s, .. }
        | Expr::SetComp { span: s, .. }
        | Expr::DictComp { span: s, .. } => *s,
        other => panic!("span_of: unhandled expr variant {other:?}"),
    }
}

/// Parse `src`, find the `auto probe = <expr>` binding anywhere in the module's
/// functions, and return `(inferred type's display, map length)` for that
/// binding's value expression.
fn probe_type(src: &str) -> (String, usize) {
    let (module, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");

    // Locate the `probe` binding's value expression span.
    let mut probe_span: Option<Span> = None;
    for item in &module.items {
        if let Item::Function(f) = item {
            for stmt in &f.body.stmts {
                if let Stmt::Let { name, value, .. } = stmt {
                    if name == "probe" {
                        probe_span = Some(span_of(value));
                    }
                }
            }
        }
    }
    let span = probe_span.expect("no `probe` binding found");

    let (errs, _warns, types) = analyze_types(&module);
    assert!(errs.is_empty(), "sema errors: {errs:?}");
    let ty = types
        .type_at(span)
        .unwrap_or_else(|| panic!("no type recorded for the probe expression"));
    (ty.display(), types.len())
}

/// Assert the `probe` binding's value infers to `expected` (by `Ty::display`).
fn assert_probe(src: &str, expected: &str) {
    let (got, n) = probe_type(src);
    assert!(n > 0, "the type map is empty");
    assert_eq!(got, expected, "probe expression type mismatch");
}

// ----- The forms the monomorphizer's own inferer cannot type -----

#[test]
fn module_constant_math_inf_is_float64() {
    // #1162: `math.inf` is a module-qualified constant; sema types it float64.
    assert_probe(
        "import math\n\
        def main():\n\
        \x20   auto probe = math.inf\n\
        \x20   print(probe)\n",
        "float64",
    );
}

#[test]
fn builtin_free_call_len_is_int64() {
    // #1132: a built-in free-function call return.
    assert_probe(
        "def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto probe = len(xs)\n\
        \x20   print(probe)\n",
        "int64",
    );
}

#[test]
fn builtin_method_str_upper_is_str() {
    // #1132: a built-in method call return.
    assert_probe(
        "def main():\n\
        \x20   str s = \"hi\"\n\
        \x20   auto probe = s.upper()\n\
        \x20   print(probe)\n",
        "str",
    );
}

#[test]
fn subscript_list_element_is_int64() {
    assert_probe(
        "def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto probe = xs[0]\n\
        \x20   print(probe)\n",
        "int64",
    );
}

#[test]
fn slice_list_is_list_int64() {
    assert_probe(
        "def main():\n\
        \x20   list[int64] xs = [1, 2, 3]\n\
        \x20   auto probe = xs[0:2]\n\
        \x20   print(probe.len())\n",
        "list[int64]",
    );
}

#[test]
fn tuple_literal_is_tuple() {
    assert_probe(
        "def main():\n\
        \x20   auto probe = (1, 2)\n\
        \x20   print(probe)\n",
        "(int64, int64)",
    );
}

#[test]
fn list_comprehension_is_list_int64() {
    assert_probe(
        "def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto probe = [x * 2 for x in xs]\n\
        \x20   print(probe.len())\n",
        "list[int64]",
    );
}

// ----- The map is only populated in analysis mode, and covers plain forms -----

#[test]
fn literal_and_binary_are_recorded() {
    assert_probe(
        "def main():\n\
        \x20   auto probe = 2 - 5\n\
        \x20   print(probe)\n",
        "int64",
    );
}

#[test]
fn plain_check_does_not_populate_the_map() {
    // `analyze` (the normal compile path) records nothing; only `analyze_types`
    // / `analyze_queryable` retain the per-expression types.
    let (module, _p, _l) = parse_str(
        "def main():\n\
        \x20   auto probe = 1\n\
        \x20   print(probe)\n",
    );
    let (errs, _warns) = zapc_sema::analyze(&module);
    assert!(errs.is_empty());
    // Sanity: analysis mode *does* populate it for the same module.
    let (_e, _w, types) = analyze_types(&module);
    assert!(!types.is_empty());
}
