//! The `comptime` / `reflect` parser surface — S1 (#1297, epic #1296).
//!
//! Parses the `comptime` keyword forms (locked decision 4 of epic #1249):
//! `comptime for <target> in <expr>:` (a compile-time-unrolled loop, the
//! distinct [`Stmt::ComptimeFor`] node) and `comptime <expr>` (the
//! [`Expr::Comptime`] force-eval prefix). This slice is **parse-only** — no
//! evaluation/unrolling (that is S2, #1298). The `comptime.<builtin>` qualified
//! call (`comptime.error(...)`, #1256) keeps parsing as an ordinary call.
//!
//! Intent documentation: the shapes, the round-trip through `render_expr` /
//! `zapc_fmt`, the `comptime.error` coexistence, and that `comptime` is now a
//! reserved word.

use zapc_syntax::ast::{render_expr, Block, Expr, Item, Stmt};
use zapc_syntax::parser::parse_str;

#[track_caller]
fn body_of(src: &str) -> Block {
    let (m, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    m.items
        .into_iter()
        .find_map(|i| match i {
            Item::Function(f) => Some(f.body),
            _ => None,
        })
        .expect("a function")
}

#[track_caller]
fn parse_errs(src: &str) -> usize {
    let (_m, perrs, _l) = parse_str(src);
    perrs.len()
}

// ---------------------------------------------------------------------------
// comptime for
// ---------------------------------------------------------------------------

#[test]
fn comptime_for_parses_to_its_own_node() {
    let b = body_of("def g():\n    comptime for f in xs:\n        pass\n");
    match &b.stmts[0] {
        Stmt::ComptimeFor {
            target, iter, body, ..
        } => {
            assert!(matches!(target, Expr::Name(n, _) if n == "f"));
            assert!(matches!(iter, Expr::Name(n, _) if n == "xs"));
            assert_eq!(body.stmts.len(), 1);
        }
        other => panic!("expected ComptimeFor, got {other:?}"),
    }
}

#[test]
fn comptime_for_over_a_reflect_call_parses() {
    // The generator shape: `comptime for f in reflect.type_info(T).fields:`.
    let b = body_of("def g():\n    comptime for f in reflect.type_info(T).fields:\n        pass\n");
    let Stmt::ComptimeFor { iter, .. } = &b.stmts[0] else {
        panic!("expected ComptimeFor");
    };
    // `reflect.type_info(T).fields` is an ordinary attribute access on a call.
    assert!(matches!(iter, Expr::Attr { name, .. } if name == "fields"));
}

#[test]
fn a_plain_for_is_still_a_runtime_for() {
    let b = body_of("def g():\n    for f in xs:\n        pass\n");
    assert!(matches!(b.stmts[0], Stmt::For { .. }));
}

#[test]
fn nested_comptime_for_parses() {
    let b = body_of(
        "def g():\n    comptime for a in xs:\n        comptime for b in ys:\n            pass\n",
    );
    let Stmt::ComptimeFor { body, .. } = &b.stmts[0] else {
        panic!("outer ComptimeFor");
    };
    assert!(matches!(body.stmts[0], Stmt::ComptimeFor { .. }));
}

// ---------------------------------------------------------------------------
// comptime <expr>
// ---------------------------------------------------------------------------

#[test]
fn comptime_expr_prefix_parses() {
    // As a call argument: `reflect.field(v, comptime f.index)`.
    let b = body_of("def g():\n    h(comptime f.index)\n");
    let Stmt::Expr(Expr::Call { args, .. }) = &b.stmts[0] else {
        panic!("expected a call statement, got {:?}", b.stmts[0]);
    };
    assert!(
        matches!(&args[0].value, Expr::Comptime { .. }),
        "arg should be a comptime expr, got {:?}",
        args[0].value
    );
}

#[test]
fn comptime_binding_uses_the_expr_prefix() {
    // `auto info = comptime reflect.type_info(T)` — the binding form (locked
    // decision 4's `comptime <binding>`) via the expression prefix.
    let b = body_of("def g():\n    auto info = comptime reflect.type_info(T)\n");
    let Stmt::Let { value, .. } = &b.stmts[0] else {
        panic!("expected a let, got {:?}", b.stmts[0]);
    };
    assert!(matches!(value, Expr::Comptime { .. }));
}

// ---------------------------------------------------------------------------
// comptime.<builtin> coexistence (#1256)
// ---------------------------------------------------------------------------

#[test]
fn comptime_dot_error_is_still_a_qualified_call_not_the_prefix() {
    let b = body_of("def g():\n    comptime.error(\"boom\")\n");
    let Stmt::Expr(Expr::Call { callee, .. }) = &b.stmts[0] else {
        panic!("expected a call, got {:?}", b.stmts[0]);
    };
    // callee is `comptime.error` — an Attr on the `comptime` name, NOT a
    // `comptime <expr>` prefix.
    match &**callee {
        Expr::Attr { base, name, .. } => {
            assert_eq!(name, "error");
            assert!(matches!(&**base, Expr::Name(n, _) if n == "comptime"));
        }
        other => panic!("expected comptime.error Attr, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// round-trip: render + fmt
// ---------------------------------------------------------------------------

#[test]
fn render_expr_round_trips_comptime() {
    let b = body_of("def g():\n    h(comptime f.index)\n");
    let Stmt::Expr(Expr::Call { args, .. }) = &b.stmts[0] else {
        panic!("call");
    };
    assert_eq!(render_expr(&args[0].value), "comptime f.index");
}

// (The `zapc_fmt` round-trip for this surface lives in the fmt crate's tests,
// since `zapc_syntax` cannot depend on `zapc_fmt`.)

// ---------------------------------------------------------------------------
// reserved word + malformed
// ---------------------------------------------------------------------------

#[test]
fn comptime_is_now_a_reserved_word() {
    // Using `comptime` as a plain binding name no longer parses.
    assert!(parse_errs("def g():\n    auto comptime = 1\n") > 0);
}

#[test]
fn bare_comptime_with_no_operand_is_a_parse_error() {
    assert!(parse_errs("def g():\n    comptime\n") > 0);
}
