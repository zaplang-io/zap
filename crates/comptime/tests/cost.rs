//! Comptime build-time cost controls — the `comptime for` unroll-breadth cap and
//! the evaluation cost telemetry (E7, #1617; epic #1248).
//!
//! The step cap (#1250) and depth cap already bound total work and recursion;
//! these tests document the E7 additions: a dedicated per-loop unroll-breadth cap
//! that fails fast with a clean `Budget` diagnostic (never a panic / explosion),
//! and the `steps_used` / `unrolled_iterations` telemetry (alongside the
//! registry's `MemoStats`, see `memoization.rs`) that make comptime cost an
//! observable, deterministic, comparable compile-time metric.
//!
//! Intent documentation: a normal derive-shaped unroll is orders of magnitude
//! under the default cap; the cap is per-loop (not the product of nested loops,
//! which the step cap bounds); the boundary is exact; a cap breach is a clean
//! `Budget` error at the loop's iterable; and the telemetry accumulates across
//! nested loops.

mod common;

use common::{parse_body, parse_expr, parse_structs};
use std::rc::Rc;
use zapc_comptime::{
    type_info_of_struct, unroll, CtErrorKind, Env, Evaluator, Value, DEFAULT_ITER_CAP,
};

/// Bind `name` to a comptime list of `n` `int64` zeros, so a test can unroll a
/// `comptime for` of an arbitrary, controlled breadth (the comptime fragment
/// itself cannot *generate* a large list — no range/comprehension/recursion —
/// so a test constructs the list value directly to exercise the cap).
fn list_env(name: &str, n: usize) -> Env {
    let mut env = Env::new();
    env.define(name, Value::list(vec![Value::Int(0); n]));
    env
}

// ---------------------------------------------------------------------------
// telemetry
// ---------------------------------------------------------------------------

#[test]
fn unroll_records_iteration_and_step_telemetry() {
    // A derive-shaped loop over a 3-field struct's `info.fields`.
    let defs = parse_structs("struct P:\n    int64 a\n    int64 b\n    int64 c\n");
    let info = type_info_of_struct(&defs[0]);
    let mut base = Env::new();
    base.define("info", Value::Type(Rc::new(info), None));
    let iterable = parse_expr("info.fields");
    let body = parse_body("emit(f.index)");

    let mut ev = Evaluator::new();
    let out = unroll("f", &iterable, &body, &base, &mut ev).expect("unroll ok");
    assert_eq!(out.len(), 3, "one body per field");
    assert_eq!(
        ev.unrolled_iterations(),
        3,
        "telemetry counts the three unrolled iterations",
    );
    assert!(
        ev.steps_used() > 0,
        "evaluating the iterable and the folded bodies charges steps",
    );
}

#[test]
fn telemetry_counts_zero_for_an_empty_loop() {
    let mut ev = Evaluator::new();
    let base = list_env("xs", 0);
    let out = unroll(
        "x",
        &parse_expr("xs"),
        &parse_body("emit(x)"),
        &base,
        &mut ev,
    )
    .expect("unroll ok");
    assert!(out.is_empty());
    assert_eq!(ev.unrolled_iterations(), 0);
}

// ---------------------------------------------------------------------------
// the unroll-breadth cap
// ---------------------------------------------------------------------------

#[test]
fn an_oversized_comptime_for_fails_fast_with_a_clean_budget_error() {
    let mut ev = Evaluator::new().with_iter_cap(2);
    let base = list_env("xs", 5);
    let err = unroll(
        "x",
        &parse_expr("xs"),
        &parse_body("emit(x)"),
        &base,
        &mut ev,
    )
    .expect_err("5 iterations over a cap of 2 must fail");
    assert_eq!(
        err.kind,
        CtErrorKind::Budget,
        "a cap breach is a Budget fault, not a panic or a generic value error",
    );
    assert!(
        err.message.contains("unroll 5 iterations")
            && err.message.contains("over the limit (2 iterations)"),
        "the diagnostic names the breadth and the cap: {}",
        err.message,
    );
}

#[test]
fn the_cap_boundary_is_exact() {
    // Exactly `cap` iterations is allowed; `cap + 1` is not.
    let at_cap = {
        let mut ev = Evaluator::new().with_iter_cap(4);
        let base = list_env("xs", 4);
        unroll(
            "x",
            &parse_expr("xs"),
            &parse_body("emit(x)"),
            &base,
            &mut ev,
        )
    };
    assert!(at_cap.is_ok(), "exactly at the cap unrolls");

    let over_cap = {
        let mut ev = Evaluator::new().with_iter_cap(4);
        let base = list_env("xs", 5);
        unroll(
            "x",
            &parse_expr("xs"),
            &parse_body("emit(x)"),
            &base,
            &mut ev,
        )
    };
    assert!(over_cap.is_err(), "one past the cap fails");
}

#[test]
fn the_default_cap_is_far_above_a_realistic_derive() {
    // A normal derive unrolls over a handful of fields/variants; the default cap
    // is a safety net real code never meets. Prove the real exported default
    // admits an unroll as wide as the cap itself (tracking the constant, not a
    // hard-coded number), so no realistic derive is ever refused.
    let n = DEFAULT_ITER_CAP as usize;
    let base = list_env("xs", n);
    let mut ev = Evaluator::new();
    assert!(
        unroll(
            "x",
            &parse_expr("xs"),
            &parse_body("emit(x)"),
            &base,
            &mut ev
        )
        .is_ok(),
        "a cap-sized unroll is admitted at the default cap",
    );
    assert_eq!(ev.unrolled_iterations(), DEFAULT_ITER_CAP);
}

// ---------------------------------------------------------------------------
// nested loops: the cap is per-loop, telemetry accumulates
// ---------------------------------------------------------------------------

#[test]
fn nested_loops_accumulate_telemetry_and_are_capped_per_loop() {
    // Outer over `outer` (N), body nests a `comptime for` over `inner` (M).
    // Total unrolled = N (outer) + N*M (inner unrolls once per outer iteration).
    let mut base = Env::new();
    base.define("outer", Value::list(vec![Value::Int(0); 3]));
    base.define("inner", Value::list(vec![Value::Int(0); 4]));
    let body = parse_body("comptime for y in inner:\n    emit(y)\n");

    // A cap of 3 admits each individual loop (outer=3, inner=4?) — no: inner is
    // 4, so set the cap to 4 so both the outer (3) and each inner (4) pass,
    // while the *product* of iterations (3 + 3*4 = 15) exceeds it. This proves
    // the cap bounds each loop's breadth, not the cumulative product.
    let mut ev = Evaluator::new().with_iter_cap(4);
    let out = unroll("x", &parse_expr("outer"), &body, &base, &mut ev)
        .expect("each loop is within the per-loop cap");
    assert_eq!(
        out.len(),
        3 * 4,
        "3 outer iterations, each emitting 4 inner"
    );
    assert_eq!(
        ev.unrolled_iterations(),
        3 + 3 * 4,
        "telemetry sums the outer and every nested inner unroll",
    );
}

#[test]
fn a_nested_loop_that_is_itself_too_wide_is_caught() {
    let mut base = Env::new();
    base.define("outer", Value::list(vec![Value::Int(0); 2]));
    base.define("inner", Value::list(vec![Value::Int(0); 10]));
    let body = parse_body("comptime for y in inner:\n    emit(y)\n");
    let mut ev = Evaluator::new().with_iter_cap(5);
    let err = unroll("x", &parse_expr("outer"), &body, &base, &mut ev)
        .expect_err("the inner loop (10) exceeds the cap (5)");
    assert_eq!(err.kind, CtErrorKind::Budget);
    assert!(
        err.message.contains("unroll 10 iterations"),
        "{}",
        err.message
    );
}
