//! A function type nested inside a tuple type argument (#1502, follow-up to
//! #1488).
//!
//! #1488 admitted a function type at the head of a type-argument element
//! (`run[fn(A) -> B]`). A function type *inside a tuple* type argument —
//! `run[(fn(A) -> B, C)]` — still failed to parse, because the tuple there is a
//! parenthesized value expression and a function type is not a value
//! expression. The parenthesized-tuple parser now admits a `fn(` function type
//! as a tuple element (carried as `Expr::TypeArg`), which `mono`'s
//! `expr_to_type` resolves inside the enclosing `Type::Tuple`.
//!
//! Boundary (accepted ambiguity, decided for #1502): `fn` is a legal
//! identifier, so a `fn(` element in a subscript/tuple is always read as a
//! function TYPE — a variable named `fn` cannot be *called* in that position.
//! That is a parse error, asserted in the grammar corpus, not here.

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
fn fn_type_as_tuple_type_arg_element_free_fn() {
    // The issue's reducer: a function type as the first element of a tuple type
    // argument, paired with a scalar.
    lower_ok(
        "def inc(int64 x) -> int64:\n    return x + 1\n\
         def run[T](T fallback) -> T:\n    return fallback\n\
         def main():\n\
         \x20   auto pair = run[(fn(int64) -> int64, int64)]((inc, 5))\n\
         \x20   auto f, n = pair\n\
         \x20   print(f(n))\n",
    );
}

#[test]
fn fn_type_as_tuple_type_arg_element_method() {
    lower_ok(
        "def inc(int64 x) -> int64:\n    return x + 1\n\
         struct Box:\n    int64 unused\n\
         \x20   def run[T](self, T fallback) -> T:\n        return fallback\n\
         def main():\n    auto b = Box(0)\n\
         \x20   auto pair = b.run[(fn(int64) -> int64, int64)]((inc, 9))\n\
         \x20   auto f, n = pair\n\
         \x20   print(f(n))\n",
    );
}

#[test]
fn grouped_single_fn_type_type_arg() {
    // A parenthesized single function type is the function type itself.
    lower_ok(
        "def inc(int64 x) -> int64:\n    return x + 1\n\
         def run[T](T fallback) -> T:\n    return fallback\n\
         def main():\n\
         \x20   auto f = run[(fn(int64) -> int64)](inc)\n\
         \x20   print(f(41))\n",
    );
}

#[test]
fn two_fn_types_in_a_tuple_type_arg() {
    // Two function-type elements in one tuple type argument.
    lower_ok(
        "def inc(int64 x) -> int64:\n    return x + 1\n\
         def dbl(int64 x) -> int64:\n    return x * 2\n\
         def run[T](T fallback) -> T:\n    return fallback\n\
         def main():\n\
         \x20   auto fns = run[(fn(int64) -> int64, fn(int64) -> int64)]((inc, dbl))\n\
         \x20   auto a, b = fns\n\
         \x20   print(a(10), b(10))\n",
    );
}

#[test]
fn fn_type_tuple_arg_with_void_return_element() {
    // A void-return function type as a tuple element.
    lower_ok(
        "def shout(int64 n):\n    print(n)\n\
         def run[T](T fallback) -> T:\n    return fallback\n\
         def main():\n\
         \x20   auto pair = run[(fn(int64), int64)]((shout, 3))\n\
         \x20   auto s, n = pair\n\
         \x20   s(n)\n",
    );
}
