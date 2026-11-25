//! A function type as an explicit generic type argument (#1488).
//!
//! `run[fn(int64) -> int64](...)` used to fail to parse — a subscript element
//! was only ever a value expression, and a function type (`fn(A) -> B`) is not
//! one, so the parser reported `ZAP2001: expected ']'`. A bare function name or
//! lambda also cannot pin a generic type parameter by inference, so a function
//! value could only reach a generic `T` through an intermediate `fn`-typed
//! binding. The parser now accepts a `func_type` in type-argument position and
//! carries it as an `Expr::TypeArg`, which `mono`'s `expr_to_type` unwraps.
//!
//! These host-only checks (no LLVM) pin that every such call lowers; the
//! end-to-end values are covered by the `fn_type_type_arg` golden on CI.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
}

#[test]
fn free_fn_explicit_fn_type_arg_lambda() {
    // The issue's reducer: a one-parameter generic instantiated with a function
    // type, applied to a lambda.
    lower_ok(
        "def run[T](T fallback) -> T:\n    return fallback\n\
         def main():\n\
         \x20   auto f = run[fn(int64) -> int64](lambda x: x + 1)\n\
         \x20   print(f(41))\n",
    );
}

#[test]
fn free_fn_explicit_fn_type_arg_named_function() {
    // A bare function name reaches the generic `T` via the explicit type arg —
    // the case inference alone could not resolve.
    lower_ok(
        "def inc(int64 x) -> int64:\n    return x + 1\n\
         def run[T](T fallback) -> T:\n    return fallback\n\
         def main():\n\
         \x20   auto f = run[fn(int64) -> int64](inc)\n\
         \x20   print(f(41))\n",
    );
}

#[test]
fn method_explicit_fn_type_arg() {
    // The same on a generic method with explicit type args, `recv.m[fn(..)->..](..)`.
    lower_ok(
        "struct Box:\n    int64 unused\n\
         \x20   def run[T](self, T fallback) -> T:\n        return fallback\n\
         def main():\n    auto b = Box(0)\n\
         \x20   auto f = b.run[fn(int64) -> int64](lambda x: x * 2)\n\
         \x20   print(f(21))\n",
    );
}

#[test]
fn multi_param_fn_type_arg() {
    // A multi-parameter function type as the single type argument.
    lower_ok(
        "def add(int64 a, int64 b) -> int64:\n    return a + b\n\
         def run[T](T fallback) -> T:\n    return fallback\n\
         def main():\n\
         \x20   auto g = run[fn(int64, int64) -> int64](add)\n\
         \x20   print(g(2, 3))\n",
    );
}

#[test]
fn void_return_fn_type_arg() {
    // A function type with no return (`-> void`) as the type argument.
    lower_ok(
        "def shout(int64 n):\n    print(n)\n\
         def run[T](T fallback) -> T:\n    return fallback\n\
         def main():\n\
         \x20   auto g = run[fn(int64)](shout)\n\
         \x20   g(7)\n",
    );
}

// NOTE (scope): a function type *nested inside* a tuple type argument —
// `run[(fn(int64) -> int64, int64)]` — is still a parse error, because the
// tuple there is parsed as a parenthesized value expression and a function type
// is not a value expression. #1488 covers the direct form `run[fn(A) -> B]`;
// the nested-in-tuple shape would need the tuple-element grammar to admit
// function types and is left as a follow-up (the `fn`-typed-binding workaround
// still applies).
