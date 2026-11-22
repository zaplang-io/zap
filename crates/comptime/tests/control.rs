//! `let` bindings + scoping, `comptime if`, the ternary, boolean short-circuit,
//! and non-recursive user-function calls (#1250) — the HQ9 unblock: comptime
//! must run library code, not just fixed primitives.

mod common;

use common::{err, eval_in, ok, parse_fns};
use zapc_comptime::{CtErrorKind, Evaluator, Value};
use zapc_syntax::span::Span;

// ---------------------------------------------------------------------------
// let bindings + lexical scoping / shadowing
// ---------------------------------------------------------------------------

#[test]
fn let_binding_and_use() {
    let m = "def f():\n    int64 x = 10\n    int64 y = x + 5\n    return y\n";
    assert_eq!(eval_in(m, "f()").unwrap(), Value::Int(15));
}

#[test]
fn inner_block_shadows_then_restores() {
    // The inner `x` shadows within the `if` body; the outer `x` is restored
    // after the block ends.
    let m = "def f():\n    int64 x = 1\n    if True:\n        int64 x = 99\n        pass\n    return x\n";
    assert_eq!(eval_in(m, "f()").unwrap(), Value::Int(1));
}

#[test]
fn inner_block_binding_is_not_visible_outside() {
    let m = "def f():\n    if True:\n        int64 y = 2\n        pass\n    return y\n";
    let e = err_in(m, "f()");
    assert!(e.is_not_available(), "{}", e.message);
    assert!(e.message.contains("name `y`"), "{}", e.message);
}

#[test]
fn same_scope_rebinding_shadows() {
    let m = "def f():\n    int64 x = 1\n    int64 x = 2\n    return x\n";
    assert_eq!(eval_in(m, "f()").unwrap(), Value::Int(2));
}

#[test]
fn nested_block_sees_outer_binding() {
    let m = "def f():\n    int64 x = 7\n    if True:\n        return x + 1\n    return 0\n";
    assert_eq!(eval_in(m, "f()").unwrap(), Value::Int(8));
}

// ---------------------------------------------------------------------------
// comptime if
// ---------------------------------------------------------------------------

#[test]
fn comptime_if_elif_else() {
    let m = "def classify(int64 n):\n    if n < 0:\n        return \"neg\"\n    elif n == 0:\n        return \"zero\"\n    else:\n        return \"pos\"\n";
    assert_eq!(eval_in(m, "classify(-3)").unwrap(), Value::str("neg"));
    assert_eq!(eval_in(m, "classify(0)").unwrap(), Value::str("zero"));
    assert_eq!(eval_in(m, "classify(5)").unwrap(), Value::str("pos"));
}

#[test]
fn comptime_if_only_evaluates_the_taken_branch() {
    // The untaken branch contains a fault (`1 // 0`); it must not be evaluated.
    let m = "def guard(bool c):\n    if c:\n        return 1 // 0\n    return 42\n";
    assert_eq!(eval_in(m, "guard(False)").unwrap(), Value::Int(42));
    // Taking the faulting branch does surface the fault.
    assert_eq!(err_in(m, "guard(True)").kind, CtErrorKind::Value);
}

// ---------------------------------------------------------------------------
// ternary / boolean short-circuit
// ---------------------------------------------------------------------------

#[test]
fn ternary() {
    assert_eq!(ok("1 if True else 2"), Value::Int(1));
    assert_eq!(ok("1 if False else 2"), Value::Int(2));
    // The untaken arm is not evaluated.
    assert_eq!(ok("7 if True else (1 // 0)"), Value::Int(7));
    assert_eq!(ok("(1 // 0) if False else 9"), Value::Int(9));
}

#[test]
fn boolean_short_circuit() {
    // `and` does not evaluate its right operand when the left is false; `or`
    // does not when the left is true — observable because the right operand
    // would otherwise fault.
    assert_eq!(ok("False and (1 // 0 == 0)"), Value::Bool(false));
    assert_eq!(ok("True or (1 // 0 == 0)"), Value::Bool(true));
    // And they do reach the right operand when needed.
    assert_eq!(ok("True and (2 > 1)"), Value::Bool(true));
    assert_eq!(ok("False or (2 > 3)"), Value::Bool(false));
    assert_eq!(err("True and (1 // 0 == 0)").kind, CtErrorKind::Value);
}

#[test]
fn not_operator() {
    assert_eq!(ok("not True"), Value::Bool(false));
    assert_eq!(ok("not (1 > 2)"), Value::Bool(true));
}

// ---------------------------------------------------------------------------
// non-recursive user function calls
// ---------------------------------------------------------------------------

#[test]
fn multi_arg_call() {
    let m = "def add(int64 a, int64 b):\n    return a + b\n";
    assert_eq!(eval_in(m, "add(3, 4)").unwrap(), Value::Int(7));
}

#[test]
fn call_returning_tuple_and_list() {
    let m = "def pair(int64 a, int64 b):\n    return (a, b)\ndef seq(int64 n):\n    return [n, n + 1, n + 2]\n";
    assert_eq!(
        eval_in(m, "pair(1, 2)").unwrap(),
        Value::tuple(vec![Value::Int(1), Value::Int(2)])
    );
    assert_eq!(
        eval_in(m, "seq(5)").unwrap(),
        Value::list(vec![Value::Int(5), Value::Int(6), Value::Int(7)])
    );
}

#[test]
fn transitive_call_chain() {
    let m = "def c(int64 x):\n    return x * 2\ndef b(int64 x):\n    return c(x) + 1\ndef a(int64 x):\n    return b(x) + b(x)\n";
    // a(3) = (c(3)+1) + (c(3)+1) = 7 + 7 = 14
    assert_eq!(eval_in(m, "a(3)").unwrap(), Value::Int(14));
}

#[test]
fn default_argument() {
    let m = "def greet(str name, str suffix = \"!\"):\n    return name + suffix\n";
    assert_eq!(eval_in(m, "greet(\"hi\")").unwrap(), Value::str("hi!"));
    assert_eq!(
        eval_in(m, "greet(\"hi\", \"?\")").unwrap(),
        Value::str("hi?")
    );
}

#[test]
fn missing_argument_is_an_error() {
    let m = "def add(int64 a, int64 b):\n    return a + b\n";
    let e = err_in(m, "add(1)");
    assert_eq!(e.kind, CtErrorKind::Value);
    assert!(e.message.contains("missing argument"), "{}", e.message);
}

#[test]
fn call_api_with_constructed_args() {
    // The public `Evaluator::call` entry, driven with already-built values.
    let fns = parse_fns("def mul(int64 a, int64 b):\n    return a * b\n");
    let mut ev = Evaluator::with_functions(&fns);
    let v = ev
        .call("mul", vec![Value::Int(6), Value::Int(7)], Span::new(0, 0))
        .unwrap();
    assert_eq!(v, Value::Int(42));
}

#[test]
fn unknown_function_is_out_of_fragment() {
    let e = err("mystery(1, 2)");
    assert!(e.is_not_available(), "{}", e.message);
}

// ---------------------------------------------------------------------------
// recursion is out of the fragment
// ---------------------------------------------------------------------------

#[test]
fn direct_recursion_is_rejected() {
    let m = "def fact(int64 n):\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n";
    let e = err_in(m, "fact(5)");
    assert!(e.is_not_available(), "{}", e.message);
    assert!(e.message.contains("recursion"), "{}", e.message);
}

#[test]
fn mutual_recursion_is_rejected() {
    let m = "def is_even(int64 n):\n    if n == 0:\n        return True\n    return is_odd(n - 1)\ndef is_odd(int64 n):\n    if n == 0:\n        return False\n    return is_even(n - 1)\n";
    let e = err_in(m, "is_even(4)");
    assert!(e.is_not_available(), "{}", e.message);
    assert!(e.message.contains("recursion"), "{}", e.message);
}

#[test]
fn function_without_return_is_an_error() {
    let m = "def noret(int64 x):\n    pass\n";
    let e = err_in(m, "noret(1)");
    assert_eq!(e.kind, CtErrorKind::Value);
    assert!(e.message.contains("did not return"), "{}", e.message);
}

/// Like `err`, but evaluates `expr_src` against a module's functions.
#[track_caller]
fn err_in(module_src: &str, expr_src: &str) -> zapc_comptime::CtError {
    match eval_in(module_src, expr_src) {
        Ok(v) => panic!("eval `{expr_src}` unexpectedly succeeded with {v:?}"),
        Err(e) => e,
    }
}
