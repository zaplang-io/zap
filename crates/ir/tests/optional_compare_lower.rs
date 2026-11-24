//! `==` / `!=` and ordering (`<` `<=` `>` `>=`) on `Optional[T]` (#1368).
//!
//! `Optional[T]` had structural equality/hash only as a dict key / set element
//! (#703/#704); the `==`/`!=`/ordering *operators* bailed at lowering with
//! `ZAP6001` ("type 'Optional$T' does not define '__eq__' ...", D-0047), while
//! only `== None` / `is None` were special-cased. The operators are now defined:
//!
//! - `==` / `!=` are structural: `None == None`, `Some(x) == Some(y)` iff
//!   `x == y`, and `Some` never equals `None`. Requires `T` to support `==`.
//! - ordering is a total order with **`None` before every `Some`**
//!   (`Option<T>`-style): `None < Some(_)`, and `Some(x) OP Some(y)` is
//!   `x OP y`. Requires `T` to support ordering.
//! - a bare payload on the right (`opt == 7`) is compared as `opt == Some(7)`.
//!
//! The Some/Some payload comparison recurses into the ordinary operator
//! lowering, so a payload type that does not support the operator yields the
//! same clean diagnostic it would for a direct comparison (pinned in the
//! negatives). Host-only (`lower_module`); runtime values are covered by the
//! `optional_compare` golden.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
}

fn lower_err_contains(src: &str, needle: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(!errs.is_empty(), "expected a lowering error, got none");
    let joined = errs
        .iter()
        .map(|e| format!("{e:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains(needle),
        "expected a lowering error containing {needle:?}, got:\n{joined}"
    );
}

// ----- Equality -----

#[test]
fn optional_int_eq_ne() {
    lower_ok(
        "def main():\n\
        \x20   Optional[int64] a = Optional.Some(7)\n\
        \x20   Optional[int64] b = Optional.Some(7)\n\
        \x20   print(a == b)\n\
        \x20   print(a != b)\n",
    );
}

#[test]
fn optional_int_eq_none_mix() {
    lower_ok(
        "def main():\n\
        \x20   Optional[int64] a = Optional.Some(7)\n\
        \x20   Optional[int64] n = None\n\
        \x20   print(a == n)\n\
        \x20   print(n == n)\n",
    );
}

#[test]
fn optional_str_eq() {
    lower_ok(
        "def main():\n\
        \x20   Optional[str] a = Optional.Some(\"hi\")\n\
        \x20   Optional[str] b = None\n\
        \x20   print(a == b)\n",
    );
}

#[test]
fn optional_eq_bare_payload() {
    // `opt == 7` compares as `opt == Some(7)`.
    lower_ok(
        "def main():\n\
        \x20   Optional[int64] a = Optional.Some(7)\n\
        \x20   print(a == 7)\n",
    );
}

// ----- Ordering (None-first total order) -----

#[test]
fn optional_int_ordering() {
    lower_ok(
        "def main():\n\
        \x20   Optional[int64] a = Optional.Some(1)\n\
        \x20   Optional[int64] b = Optional.Some(2)\n\
        \x20   print(a < b)\n\
        \x20   print(a <= b)\n\
        \x20   print(a > b)\n\
        \x20   print(a >= b)\n",
    );
}

#[test]
fn optional_ordering_none_first() {
    lower_ok(
        "def main():\n\
        \x20   Optional[int64] n = None\n\
        \x20   Optional[int64] s = Optional.Some(0)\n\
        \x20   print(n < s)\n\
        \x20   print(n <= n)\n\
        \x20   print(s > n)\n",
    );
}

#[test]
fn optional_str_ordering() {
    lower_ok(
        "def main():\n\
        \x20   Optional[str] a = Optional.Some(\"a\")\n\
        \x20   Optional[str] b = Optional.Some(\"b\")\n\
        \x20   print(a < b)\n",
    );
}

// ----- Negatives: payload must support the operator -----

#[test]
fn optional_eq_requires_payload_eq() {
    // A reference struct without `__eq__` cannot be compared, so neither can an
    // Optional of it — the Some/Some payload comparison surfaces the clean
    // diagnostic.
    lower_err_contains(
        "struct NoEq:\n\
        \x20   int64 v\n\
        def main():\n\
        \x20   Optional[NoEq] a = Optional.Some(NoEq(1))\n\
        \x20   Optional[NoEq] b = Optional.Some(NoEq(1))\n\
        \x20   print(a == b)\n",
        "__eq__",
    );
}

#[test]
fn optional_ordering_requires_payload_cmp() {
    lower_err_contains(
        "struct NoCmp:\n\
        \x20   int64 v\n\
        def main():\n\
        \x20   Optional[NoCmp] a = Optional.Some(NoCmp(1))\n\
        \x20   Optional[NoCmp] b = Optional.Some(NoCmp(2))\n\
        \x20   print(a < b)\n",
        "__cmp__",
    );
}
