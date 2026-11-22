//! Comptime purity enforcement (#1254): the static leaf-table reject-lint
//! (layer a), the interpreter backstop (layer b, #1250) consulting the same
//! table, and the fixed step/recursion cap.
//!
//! Intent documentation: a leaf in an unconditional / conditional / provably-
//! dead branch is rejected at the definition site; an unknown callee is deferred
//! by the lint and caught by the interpreter; transitive propagation across 1
//! and 2+ call levels; `dict`/`set` iteration flagged `Nondet`; allocation-heavy
//! pure code is NOT rejected; the step and recursion caps trip with a
//! diagnostic; and the shared-table invariant (lint-rejects ⊇ interpreter-
//! refuses) over the detectable leaves.

mod common;

use common::{err, parse_fns};
use zapc_comptime::{lint_comptime_callable, CtErrorKind, Env, Evaluator};

/// Lint the first function in `src` (the rest are its callees).
fn lint(src: &str) -> Vec<zapc_comptime::CtError> {
    let fns = parse_fns(src);
    let (first, rest) = fns.split_first().expect("at least one function");
    lint_comptime_callable(first, rest)
}

// ---------------------------------------------------------------------------
// a leaf is rejected wherever it is mentioned (join over branches)
// ---------------------------------------------------------------------------

#[test]
fn leaf_in_unconditional_path_is_rejected() {
    let errs = lint("def f():\n    print(\"x\")\n    return 0\n");
    assert_eq!(errs.len(), 1);
    assert!(errs[0].is_not_available());
    assert!(errs[0].message.contains("I/O"), "{}", errs[0].message);
}

#[test]
fn leaf_in_conditional_branch_is_rejected() {
    let errs = lint("def f(bool c):\n    if c:\n        print(\"x\")\n    return 0\n");
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].message.contains("I/O"));
}

#[test]
fn leaf_in_provably_dead_branch_is_rejected() {
    // Fail-closed: a dead `if False:` branch still rejects — the lint does not
    // evaluate conditions, it joins over every branch.
    let errs = lint("def f():\n    if False:\n        print(\"x\")\n    return 0\n");
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].message.contains("I/O"));
}

#[test]
fn pure_function_is_not_rejected() {
    let errs = lint("def f(int64 a, int64 b):\n    int64 s = a + b\n    return s * 2\n");
    assert!(errs.is_empty(), "{errs:?}");
}

// ---------------------------------------------------------------------------
// defer on unknown callee; interpreter catches it
// ---------------------------------------------------------------------------

#[test]
fn unknown_callee_is_deferred_by_the_lint() {
    // `mystery` is not a known leaf and not a known comptime function: the lint
    // must NOT reject it (it only rejects what it can resolve).
    let errs = lint("def f():\n    return mystery(1, 2)\n");
    assert!(errs.is_empty(), "lint must defer unknown callees: {errs:?}");
    // The interpreter is the backstop: evaluating the call errors.
    assert!(err("mystery(1, 2)").is_not_available());
}

// ---------------------------------------------------------------------------
// transitive propagation
// ---------------------------------------------------------------------------

#[test]
fn transitive_one_level() {
    let errs = lint("def a():\n    return b()\ndef b():\n    print(\"x\")\n    return 0\n");
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].message.contains("I/O"));
}

#[test]
fn transitive_two_levels() {
    let src = "def a():\n    return b()\ndef b():\n    return c()\ndef c():\n    print(\"x\")\n    return 0\n";
    let errs = lint(src);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].message.contains("I/O"));
}

#[test]
fn transitive_recursion_does_not_loop() {
    // Mutually recursive callees must not hang the lint (cycle guard).
    let src = "def a():\n    return b()\ndef b():\n    return a()\n";
    let errs = lint(src);
    assert!(errs.is_empty(), "{errs:?}");
}

// ---------------------------------------------------------------------------
// dict / set iteration -> Nondet
// ---------------------------------------------------------------------------

#[test]
fn dict_and_set_iteration_flagged_nondet() {
    let d = lint("def f():\n    for k in {1: 2}:\n        pass\n    return 0\n");
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].message.contains("nondeterministic"),
        "{}",
        d[0].message
    );

    let s = lint("def f():\n    for x in {1, 2, 3}:\n        pass\n    return 0\n");
    assert_eq!(s.len(), 1, "{s:?}");
    assert!(s[0].message.contains("nondeterministic"));
}

// ---------------------------------------------------------------------------
// allocation-heavy pure code is NOT a leaf
// ---------------------------------------------------------------------------

#[test]
fn allocation_heavy_pure_code_is_allowed() {
    // Building strings and lists allocates, but allocation is not tagged.
    let src = "def f(str a):\n    str s = a + a + a\n    auto xs = [1, 2, 3, 4, 5]\n    return s + str(len(xs))\n";
    assert!(lint(src).is_empty());
}

// ---------------------------------------------------------------------------
// caps
// ---------------------------------------------------------------------------

#[test]
fn step_cap_trips_with_a_diagnostic() {
    let e = common::parse_expr("1 + 1 + 1 + 1 + 1 + 1");
    let mut ev = Evaluator::new().with_step_cap(2);
    let err = ev.eval(&e, &mut Env::new()).unwrap_err();
    assert_eq!(err.kind, CtErrorKind::Budget);
    assert!(err.message.contains("step limit"), "{}", err.message);
}

#[test]
fn recursion_depth_cap_trips_with_a_diagnostic() {
    // A non-recursive 3-deep chain a -> b -> c with a depth cap of 2 trips the
    // call-depth limit (recursion itself is separately rejected; this bounds a
    // deep-but-finite chain).
    let fns =
        parse_fns("def a():\n    return b()\ndef b():\n    return c()\ndef c():\n    return 1\n");
    let mut ev = Evaluator::with_functions(&fns).with_depth_cap(2);
    let err = ev
        .call("a", vec![], zapc_syntax::span::Span::new(0, 0))
        .unwrap_err();
    assert_eq!(err.kind, CtErrorKind::Budget);
    assert!(err.message.contains("depth"), "{}", err.message);
}

// ---------------------------------------------------------------------------
// shared-table invariant: lint-rejects ⊇ interpreter-refuses
// ---------------------------------------------------------------------------

#[test]
fn shared_table_invariant_over_detectable_leaves() {
    // For each leaf the lint can detect statically, assert BOTH layers refuse:
    // the lint rejects a function mentioning it, AND the interpreter refuses
    // evaluating it. (Ffi / GlobalMutation are interpreter-only in v1.)
    struct Case {
        // The leaf mention, as a function body statement.
        stmt: &'static str,
        // The same mention as an expression the interpreter evaluates.
        expr: &'static str,
    }
    let cases = [
        Case {
            stmt: "print(\"x\")",
            expr: "print(\"x\")",
        }, // Io
        Case {
            stmt: "auto v = time.now()",
            expr: "time.now()",
        }, // Time
        Case {
            stmt: "auto v = random.rand()",
            expr: "random.rand()",
        }, // Rng
        Case {
            stmt: "auto v = <-ch",
            expr: "<-ch",
        }, // Channel
    ];
    for c in cases {
        let src = format!("def f():\n    {}\n    return 0\n", c.stmt);
        let errs = lint(&src);
        assert!(!errs.is_empty(), "lint must reject `{}`", c.stmt);
        assert!(errs.iter().all(|e| e.is_not_available()));
        // Interpreter refuses the same op.
        assert!(
            err(c.expr).is_not_available(),
            "interpreter must refuse `{}`",
            c.expr
        );
    }
}

#[test]
fn spawn_is_rejected_by_both_layers() {
    let errs = lint("def f():\n    spawn worker()\n    return 0\n");
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].message.contains("spawn"));
    // Interpreter: a spawn statement in a comptime function is out of fragment.
    let fns = parse_fns("def f():\n    spawn worker()\n    return 0\n");
    let mut ev = Evaluator::with_functions(&fns);
    let e = ev
        .call("f", vec![], zapc_syntax::span::Span::new(0, 0))
        .unwrap_err();
    assert!(e.is_not_available());
}
