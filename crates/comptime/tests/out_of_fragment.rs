//! Every out-of-fragment construct emits the exact `not available at comptime`
//! diagnostic, at the right span, never a panic (#1250).
//!
//! The evaluator is the physical soundness backstop (epic #1249, decision 2b):
//! it has no impure primitive, so reaching I/O, FFI, spawn/channels, global
//! mutation, time/RNG, dict/set iteration, recursion, `match`-over-comptime, or
//! a float/bigint/`bytes` value is a clean span-attributed error. Recursion is
//! covered in control.rs; the rest are here.

mod common;

use common::parse_fns;
use zapc_comptime::{CtError, Env, Evaluator};

/// The exact source wrapper `common::parse_expr` uses, so a returned span slices
/// back to the offending construct.
fn wrapped(expr_src: &str) -> String {
    format!("def __harness():\n    return {expr_src}\n")
}

/// Evaluate an expression expected to be out of the fragment; return the error
/// and the source text under its span.
#[track_caller]
fn out(expr_src: &str) -> (CtError, String) {
    let source = wrapped(expr_src);
    let e = common::eval(expr_src)
        .err()
        .unwrap_or_else(|| panic!("expected `{expr_src}` to be out of fragment"));
    let text = source[e.span.start as usize..e.span.end as usize].to_string();
    (e, text)
}

#[track_caller]
fn assert_out(expr_src: &str, span_contains: &str) {
    let (e, text) = out(expr_src);
    assert!(
        e.is_not_available(),
        "`{expr_src}` should be out of fragment, got {e:?}"
    );
    assert!(
        e.message.contains("not available at comptime"),
        "`{expr_src}` message: {}",
        e.message
    );
    assert!(
        text.contains(span_contains),
        "`{expr_src}` span text `{text}` should contain `{span_contains}`"
    );
}

// ---------------------------------------------------------------------------
// float / bigint / bytes / None values
// ---------------------------------------------------------------------------

#[test]
fn float_literal() {
    assert_out("3.14", "3.14");
}

#[test]
fn imaginary_literal() {
    assert_out("2j", "2j");
}

#[test]
fn bigint_literal() {
    // One past INT64_MAX — a bigint value, deferred to B2.
    assert_out("9223372036854775808", "9223372036854775808");
    assert_out("99999999999999999999999999", "99999999999999999999999999");
}

#[test]
fn bytes_literal() {
    assert_out("b\"hi\"", "b\"hi\"");
}

#[test]
fn none_value() {
    assert_out("None", "None");
}

// ---------------------------------------------------------------------------
// dict / set (construction and the nondeterministic-iteration leaf, #1066)
// ---------------------------------------------------------------------------

#[test]
fn dict_value() {
    assert_out("{1: 2}", "{1: 2}");
}

#[test]
fn set_value() {
    assert_out("{1, 2, 3}", "{1, 2, 3}");
}

// ---------------------------------------------------------------------------
// impure leaf calls (I/O, FFI, time, RNG, …) surface as unknown functions
// ---------------------------------------------------------------------------

#[test]
fn io_and_other_leaf_calls_are_unknown_functions() {
    // The evaluator has no impure primitive; an I/O / time / RNG call is simply
    // not a comptime-reducible function.
    for src in ["print(\"x\")", "now()", "random()", "read_file(\"p\")"] {
        let (e, _) = out(src);
        assert!(e.is_not_available(), "{src}: {e:?}");
    }
}

#[test]
fn channel_receive() {
    assert_out("<-ch", "<-ch");
}

// ---------------------------------------------------------------------------
// higher-order / dynamic forms
// ---------------------------------------------------------------------------

#[test]
fn lambda() {
    assert_out("lambda x: x + 1", "lambda x: x + 1");
}

#[test]
fn comprehension() {
    assert_out("[x for x in xs]", "[x for x in xs]");
}

#[test]
fn generator_expression() {
    assert_out("(x for x in xs)", "x for x in xs");
}

// ---------------------------------------------------------------------------
// statement-position constructs (via a function body)
// ---------------------------------------------------------------------------

/// Evaluate a one-statement function body (plus a trailing `return 0` so the
/// function is well-formed) and return the error from the offending statement.
#[track_caller]
fn out_stmt(stmt_src: &str) -> CtError {
    let module = format!("def __h():\n    {stmt_src}\n    return 0\n");
    let fns = parse_fns(&module);
    let mut ev = Evaluator::with_functions(&fns);
    ev.call("__h", vec![], common_sp())
        .err()
        .unwrap_or_else(|| panic!("expected `{stmt_src}` to be out of fragment"))
}

fn common_sp() -> zapc_syntax::span::Span {
    zapc_syntax::span::Span::new(0, 0)
}

#[test]
fn match_over_comptime_values() {
    let e = out_stmt("match x:\n        case 1:\n            return 1");
    assert!(e.is_not_available(), "{e:?}");
    assert!(e.message.contains("`match`"), "{}", e.message);
}

#[test]
fn for_loop_points_to_1251() {
    let e = out_stmt("for i in xs:\n        pass");
    assert!(e.is_not_available(), "{e:?}");
    assert!(e.message.contains("#1251"), "{}", e.message);
}

#[test]
fn while_loop() {
    let e = out_stmt("while True:\n        pass");
    assert!(e.is_not_available(), "{e:?}");
    assert!(e.message.contains("`while`"), "{}", e.message);
}

#[test]
fn assignment_is_out_of_fragment() {
    // Reassignment / global mutation: comptime uses immutable `let` bindings.
    let e = out_stmt("x = 5");
    assert!(e.is_not_available(), "{e:?}");
    assert!(e.message.contains("assignment"), "{}", e.message);
}

#[test]
fn spawn_and_scope() {
    assert!(out_stmt("spawn worker()").message.contains("`spawn`"));
}

#[test]
fn raise_statement() {
    assert!(out_stmt("raise Error()").message.contains("`raise`"));
}

#[test]
fn del_statement() {
    assert!(out_stmt("del xs[0]").message.contains("`del`"));
}

// ---------------------------------------------------------------------------
// budget: the fixed step cap fails fast with a clear diagnostic
// ---------------------------------------------------------------------------

#[test]
fn step_cap_is_a_clear_diagnostic() {
    // A tiny cap makes even a small expression trip the fixed budget.
    let e = common::parse_expr("1 + 1 + 1 + 1 + 1");
    let mut ev = Evaluator::new().with_step_cap(2);
    let err = ev.eval(&e, &mut Env::new()).unwrap_err();
    assert_eq!(err.kind, zapc_comptime::CtErrorKind::Budget);
    assert!(err.message.contains("step limit"), "{}", err.message);
}
