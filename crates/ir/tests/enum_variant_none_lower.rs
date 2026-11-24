//! Type-contextual bare `None` lowering for an **enum-variant constructor
//! argument** `Enum.Variant(None)` (#1367).
//!
//! The shared type-contextual `None` / `Some`-wrap chokepoint is
//! `FnLower::expr_coerced` -> `expr_collection_context`
//! (`lower_none_expr` / `gen_opt_some`); struct-constructor fields already reach
//! it via `fill_struct_fields` (#1075), so `S(None)` for an `Optional[T]` field
//! lowers. But the enum-variant constructor lowered its payload argument with a
//! bare `self.expr(arg)` + `coerce` (`crates/ir/src/lower/enums.rs`, the niche
//! `Some` path and `build_variant`), so a bare `None` argument fell through to
//! the generic `ZAP6001` "Phase 1 slice" bail — even though sema typed it. This
//! blocked constructing a nested `Optional` (`Optional.Some(None)` for an
//! `Optional[Optional[T]]`) and any enum variant carrying an `Optional[T]`
//! (or other reference) field with a bare `None`.
//!
//! These host-only lowering checks (no LLVM) pin that every such position now
//! lowers (reaching `lower_none_expr`), and that a bare `None` for a
//! *non-nullable* (scalar) payload is still rejected with the clear
//! `None is not a valid ...` type error instead of the opaque `ZAP6001` bail.
//! End-to-end runtime values (outer-`Some` / inner-`None` round-trips) are
//! covered by the `enum_variant_none` golden on CI.

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

// ----- Nested Optional: outer Some wrapping an inner None -----

#[test]
fn nested_optional_some_none_scalar_inner() {
    // The issue's primary case: `Some(None)` where the payload is itself an
    // `Optional` — a valid outer-present / inner-absent value.
    lower_ok(
        "def main():\n\
        \x20   Optional[Optional[int64]] x = Optional.Some(None)\n\
        \x20   print(x.is_some())\n\
        \x20   print(x.unwrap().is_none())\n",
    );
}

#[test]
fn nested_optional_some_none_pointer_inner() {
    lower_ok(
        "def main():\n\
        \x20   Optional[Optional[str]] x = Optional.Some(None)\n\
        \x20   print(x.is_some())\n\
        \x20   print(x.unwrap().is_none())\n",
    );
}

// ----- Reference payload: a bare None is the payload's null (matches a struct
//        field of the same type, #1075) -----

#[test]
fn optional_str_some_none_lowers() {
    lower_ok(
        "def main():\n\
        \x20   Optional[str] x = Optional.Some(None)\n\
        \x20   print(x.is_some())\n",
    );
}

// ----- User enum variant with an Optional field -----

#[test]
fn user_enum_variant_optional_scalar_field_none() {
    lower_ok(
        "enum E:\n\
        \x20   V(Optional[int64] x)\n\
        \x20   Nil\n\
        def main():\n\
        \x20   auto e = E.V(None)\n\
        \x20   print(1)\n",
    );
}

#[test]
fn user_enum_variant_optional_pointer_field_none() {
    lower_ok(
        "enum E:\n\
        \x20   V(Optional[str] x)\n\
        \x20   Nil\n\
        def main():\n\
        \x20   auto e = E.V(None)\n\
        \x20   print(1)\n",
    );
}

#[test]
fn user_enum_variant_keyword_none() {
    // The keyword-argument form threads the same field type.
    lower_ok(
        "enum E:\n\
        \x20   V(Optional[int64] x)\n\
        \x20   Nil\n\
        def main():\n\
        \x20   auto e = E.V(x=None)\n\
        \x20   print(1)\n",
    );
}

// ----- Controls: the present forms already worked -----

#[test]
fn nested_optional_some_some_control() {
    lower_ok(
        "def main():\n\
        \x20   Optional[Optional[int64]] x = Optional.Some(Optional.Some(3))\n\
        \x20   print(x.unwrap().unwrap())\n",
    );
}

#[test]
fn user_enum_variant_some_control() {
    lower_ok(
        "enum E:\n\
        \x20   V(Optional[int64] x)\n\
        \x20   Nil\n\
        def main():\n\
        \x20   auto e = E.V(Optional.Some(5))\n\
        \x20   print(1)\n",
    );
}

// ----- Negatives: a bare None for a non-nullable payload is a clear type error
//        (not the opaque ZAP6001 Phase-1 bail) -----

#[test]
fn optional_scalar_some_none_rejected() {
    // `Optional[int64].Some(None)` — None is not a valid int64 payload.
    lower_err_contains(
        "def main():\n\
        \x20   Optional[int64] x = Optional.Some(None)\n\
        \x20   print(x.is_some())\n",
        "None is not a valid",
    );
}

#[test]
fn user_enum_variant_scalar_field_none_rejected() {
    lower_err_contains(
        "enum E:\n\
        \x20   V(int64 x)\n\
        \x20   Nil\n\
        def main():\n\
        \x20   auto e = E.V(None)\n\
        \x20   print(1)\n",
        "None is not a valid",
    );
}
