//! Type-contextual bare `None` lowering (#1075, #1080).
//!
//! A bare `None` literal in a position whose expected type is `Optional[T]`
//! used to bail at IR lowering with `ZAP6001` ("this expression is not
//! supported by codegen yet (Phase 1 slice)") whenever the `None` itself
//! carried no type context — specifically in a struct-constructor field and as
//! a `list[Optional[T]]` element — even though sema typed it correctly. These
//! host-only lowering checks (no LLVM) pin that every such position now lowers,
//! and that a bare `None` against a *non*-nullable destination is still
//! rejected with a clear type error rather than silently lowered. The
//! end-to-end runtime *values* (`.is_none()`/`.is_some()`/`.unwrap()`) are
//! covered by the `optional_none_construct` / `optional_none_list` goldens on
//! CI, since the tagged-vs-null `Optional` representation only matters once the
//! native collector runs.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting no lex/parse/lower errors.
fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
}

/// Parse + lower `src`, asserting lowering fails with a message containing
/// `needle` (parse/lex must be clean so the failure is the lowering one we
/// mean to pin).
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

// ----- #1075: bare None in an Optional struct-constructor field -----

#[test]
fn ctor_bare_none_positional_scalar_payload() {
    // The issue's minimal repro.
    lower_ok(
        "struct Box:\n\
        \x20   Optional[int] item\n\
        def main():\n\
        \x20   auto box = Box(None)\n\
        \x20   print(box.item.is_none())\n",
    );
}

#[test]
fn ctor_bare_none_keyword_scalar_payload() {
    lower_ok(
        "struct Box:\n\
        \x20   Optional[int] item\n\
        def main():\n\
        \x20   auto box = Box(item=None)\n\
        \x20   print(box.item.is_none())\n",
    );
}

#[test]
fn ctor_bare_none_reference_payload() {
    // A reference payload (`Optional[Item]`) uses the niche null; the field must
    // still lower from a bare `None`, positionally and by keyword.
    lower_ok(
        "struct Item:\n\
        \x20   int value\n\
        struct Box:\n\
        \x20   Optional[Item] slot\n\
        def main():\n\
        \x20   auto a = Box(None)\n\
        \x20   auto b = Box(slot=None)\n\
        \x20   print(a.slot.is_none())\n\
        \x20   print(b.slot.is_none())\n",
    );
}

#[test]
fn ctor_bare_none_mixed_with_some() {
    // A bare `None` and an `Optional.Some(...)` in the same constructor, across
    // both a scalar and a reference payload, positional and keyword.
    lower_ok(
        "struct Item:\n\
        \x20   int value\n\
        struct Box:\n\
        \x20   Optional[int] scalar\n\
        \x20   Optional[Item] slot\n\
        def main():\n\
        \x20   auto c = Box(Optional.Some(7), None)\n\
        \x20   auto d = Box(scalar=None, slot=Optional.Some(Item(42)))\n\
        \x20   print(c.scalar.is_some())\n\
        \x20   print(d.slot.is_some())\n",
    );
}

#[test]
fn ctor_bare_none_field_default() {
    // An omitted field whose declared default is `None` lowers the default in
    // the same type-contextual way (the adjacent gap noted under #564).
    lower_ok(
        "struct Item:\n\
        \x20   int value\n\
        struct Defaulted:\n\
        \x20   Optional[int] scalar = None\n\
        \x20   Optional[Item] slot = None\n\
        def main():\n\
        \x20   auto e = Defaulted()\n\
        \x20   print(e.scalar.is_none())\n\
        \x20   print(e.slot.is_none())\n",
    );
}

// ----- #1080: bare None in a list[Optional[T]] element -----

#[test]
fn list_literal_bare_none_element() {
    lower_ok(
        "struct Item:\n\
        \x20   int value\n\
        def main():\n\
        \x20   list[Optional[Item]] items = [None]\n\
        \x20   print(len(items))\n",
    );
}

#[test]
fn list_comprehension_bare_none_element() {
    lower_ok(
        "struct Item:\n\
        \x20   int value\n\
        def main():\n\
        \x20   list[Optional[Item]] items = [None for index in range(3)]\n\
        \x20   print(len(items))\n",
    );
}

#[test]
fn list_mixed_none_and_some_literal() {
    lower_ok(
        "struct Item:\n\
        \x20   int value\n\
        def main():\n\
        \x20   list[Optional[Item]] xs = [None, Optional.Some(Item(1)), None]\n\
        \x20   print(xs[0].is_none())\n",
    );
}

#[test]
fn list_append_bare_none() {
    // A bare `None` passed to `.append()` lands in the same type-contextual
    // element position, so it must lower too.
    lower_ok(
        "struct Item:\n\
        \x20   int value\n\
        def main():\n\
        \x20   list[Optional[Item]] xs = [None]\n\
        \x20   xs.append(None)\n\
        \x20   xs.append(Optional.Some(Item(1)))\n\
        \x20   print(len(xs))\n",
    );
}

#[test]
fn list_scalar_optional_element() {
    // The scalar-payload list element (`Optional[int64]`) takes the tagged
    // `None`, mixed with an `Optional.Some(...)` in the same literal. (Spelled
    // `int64` so the `Some` literal's representation matches the declared
    // element exactly — an `int`-vs-`int64` list-element repr mismatch is a
    // separate, pre-existing strictness unrelated to bare-`None` lowering.)
    lower_ok(
        "def main():\n\
        \x20   list[Optional[int64]] xs = [None, Optional.Some(5)]\n\
        \x20   print(xs[0].is_none())\n",
    );
}

// ----- #1195: bare None in a tuple-literal element against a declared tuple -----

#[test]
fn tuple_return_two_missing_pointer_payload() {
    // The issue's minimal repro: a function returning `(Optional[bytes],
    // Optional[bytes])` as `(None, None)` must lower (the declared tuple type
    // supplies each `None`'s `Optional[bytes]` context).
    lower_ok(
        "def get() -> (Optional[bytes], Optional[bytes]):\n\
        \x20   return (None, None)\n\
        def main():\n\
        \x20   auto value, cas = get()\n\
        \x20   print(value.is_none(), cas.is_none())\n",
    );
}

#[test]
fn tuple_return_one_missing_pointer_payload() {
    // A present + missing mix: the `Some` element stays unchanged while the
    // bare `None` takes its declared `Optional[bytes]` context.
    lower_ok(
        "def get() -> (Optional[bytes], Optional[bytes]):\n\
        \x20   return (Optional.Some(b\"x\"), None)\n\
        def main():\n\
        \x20   auto a, b = get()\n\
        \x20   print(a.is_some(), b.is_none())\n",
    );
}

#[test]
fn tuple_return_all_present_pointer_payload_unchanged() {
    // The all-present control must keep lowering — the fix must not disturb it.
    lower_ok(
        "def get() -> (Optional[bytes], Optional[bytes]):\n\
        \x20   return (Optional.Some(b\"value\"), Optional.Some(b\"cas\"))\n\
        def main():\n\
        \x20   auto a, b = get()\n\
        \x20   print(a.unwrap(), b.unwrap())\n",
    );
}

#[test]
fn tuple_return_str_payload() {
    lower_ok(
        "def get() -> (Optional[str], Optional[str]):\n\
        \x20   return (None, None)\n\
        def main():\n\
        \x20   auto a, b = get()\n\
        \x20   print(a.is_none(), b.is_none())\n",
    );
}

#[test]
fn tuple_return_reference_struct_payload() {
    // A reference-struct payload uses the niche null; the bare `None` tuple
    // element must still lower, both missing and mixed with a `Some`.
    lower_ok(
        "struct Item:\n\
        \x20   int value\n\
        def miss() -> (Optional[Item], Optional[Item]):\n\
        \x20   return (None, None)\n\
        def mixed() -> (Optional[Item], Optional[Item]):\n\
        \x20   return (None, Optional.Some(Item(42)))\n\
        def main():\n\
        \x20   auto a, b = miss()\n\
        \x20   auto c, d = mixed()\n\
        \x20   print(a.is_none(), b.is_none())\n\
        \x20   print(c.is_none(), d.unwrap().value)\n",
    );
}

#[test]
fn tuple_return_scalar_payloads() {
    // Scalar payloads (`int64`, `bool`) use the tagged discriminant `None`, a
    // different representation from the pointer-payload null — both must lower.
    lower_ok(
        "def ints() -> (Optional[int64], Optional[int64]):\n\
        \x20   return (None, None)\n\
        def ints_mixed() -> (Optional[int64], Optional[int64]):\n\
        \x20   return (Optional.Some(7), None)\n\
        def bools() -> (Optional[bool], Optional[bool]):\n\
        \x20   return (None, None)\n\
        def main():\n\
        \x20   auto a, b = ints()\n\
        \x20   auto c, d = ints_mixed()\n\
        \x20   auto e, f = bools()\n\
        \x20   print(a.is_none(), b.is_none())\n\
        \x20   print(c.unwrap(), d.is_none())\n\
        \x20   print(e.is_none(), f.is_none())\n",
    );
}

#[test]
fn tuple_typed_binding_bare_none() {
    // A typed tuple binding (not a return) threads the same per-element context.
    lower_ok(
        "def main():\n\
        \x20   (Optional[bytes], Optional[bytes]) pair = (None, None)\n\
        \x20   print(pair[0].is_none(), pair[1].is_none())\n",
    );
}

#[test]
fn tuple_return_rejects_bare_none_for_non_optional_element() {
    // A bare `None` tuple element against a non-nullable declared element type
    // is still a clear type error, not a silent lowering.
    lower_err_contains(
        "def get() -> (int, int):\n\
        \x20   return (None, None)\n\
        def main():\n\
        \x20   auto a, b = get()\n\
        \x20   print(a, b)\n",
        "None is not a valid",
    );
}

// ----- Negative: a bare None against a non-nullable destination -----

#[test]
fn ctor_bare_none_rejected_for_non_optional_field() {
    lower_err_contains(
        "struct Box:\n\
        \x20   int item\n\
        def main():\n\
        \x20   auto box = Box(None)\n\
        \x20   print(box.item)\n",
        "None is not a valid",
    );
}

#[test]
fn list_bare_none_rejected_for_non_optional_element() {
    lower_err_contains(
        "def main():\n\
        \x20   list[int] items = [None]\n\
        \x20   print(len(items))\n",
        "None is not a valid",
    );
}

#[test]
fn list_comprehension_bare_none_rejected_for_non_optional_element() {
    lower_err_contains(
        "def main():\n\
        \x20   list[int] items = [None for index in range(3)]\n\
        \x20   print(len(items))\n",
        "None is not a valid",
    );
}
