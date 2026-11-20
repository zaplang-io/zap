//! Generic type-argument inference from subscript, slice, tuple-literal, and
//! comprehension argument expressions (#1131).
//!
//! Monomorphization (`zapc_mono`) runs a small best-effort type inferer over a
//! generic constructor's / generic call's argument expressions and unifies the
//! inferred types against the generic's type parameters (I#154). Siblings of
//! #1117 (unary) and #1128 (binary): the inferer typed a `list[int64]` literal,
//! but typed a *subscript* `xs[0]`, a *slice* `xs[0:2]`, a *tuple literal*
//! `(1, 2)`, and a *comprehension* `[x for x in xs]` all as "unknown". So
//! `Optional.Some(xs[0])` could not pin `T` and bailed with `ZAP6001`, and an
//! untyped `auto e = xs[0]` binding carried that unknown into every later use.
//!
//! The fix types each arm from information it already has, mirroring sema:
//! a subscript yields the base container's element/value type (`list[T]`→`T`,
//! `dict[K, V]`→`V`, `str`→`char`, `bytes`/`range`→`int64`); a slice yields the
//! base's own type (`list[T]`/`str`/`bytes`); a tuple literal yields
//! `tuple[...]` of its element types; and a list/set/dict comprehension yields
//! `list`/`set`/`dict` of its element/key-value type, with the loop variable
//! now typed from its iterable. An operand the inferer cannot type stays
//! "unknown", so no previously-inferring site changes behaviour and a genuinely
//! unresolvable parameter is still rejected. A bare generator expression keeps
//! "unknown" on purpose (its type is a non-nominal iterator).
//!
//! These are host-only lowering checks (no LLVM): `lower_module` runs the mono
//! pass first, so a `ZAP6001` from inference surfaces here. The runtime *value*
//! and *inferred-type* round-trips are pinned by the `index_infer_optional`
//! golden, which executes on CI.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting no lex/parse/lower errors (the program is
/// accepted by the whole frontend, monomorphization included).
fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
}

/// Parse + lower `src`, asserting lowering fails with a message containing
/// `needle` (parse/lex must be clean so the failure is the lowering one we mean
/// to pin).
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

// ===== A2 — subscript `base[i]` =====

#[test]
fn subscript_list_element_infers() {
    // The issue's A2 reproduction: `xs[0]` on a `list[int64]` is `int64`.
    lower_ok(
        "def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto o = Optional.Some(xs[0])\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn subscript_list_via_auto_binding_infers() {
    // The `auto`-binding propagation path: the binding must carry `int64`.
    lower_ok(
        "def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto e = xs[0]\n\
        \x20   auto o = Optional.Some(e)\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn subscript_str_element_infers_char() {
    // Indexing a `str` yields a `char`.
    lower_ok(
        "def main():\n\
        \x20   str s = \"hi\"\n\
        \x20   auto o = Optional.Some(s[0])\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn subscript_dict_value_infers() {
    // Indexing a `dict[str, int64]` by key yields the value type `int64`.
    lower_ok(
        "def main():\n\
        \x20   dict[str, int64] d = {\"k\": 1}\n\
        \x20   auto o = Optional.Some(d[\"k\"])\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn subscript_bytes_element_infers_int() {
    // Indexing `bytes` yields the byte value as an `int64` (I#337).
    lower_ok(
        "def main():\n\
        \x20   auto data = b\"abc\"\n\
        \x20   auto o = Optional.Some(data[0])\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn subscript_in_user_generic_struct() {
    lower_ok(
        "struct Box[T]:\n\
        \x20   T item\n\
        def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto b = Box(xs[0])\n\
        \x20   print(b.item)\n",
    );
}

#[test]
fn subscript_in_generic_fn() {
    lower_ok(
        "def wrap[T](T x) -> Optional[T]:\n\
        \x20   return Optional.Some(x)\n\
        def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   print(wrap(xs[0]).unwrap())\n",
    );
}

// ===== A3 — tuple literal `(a, b)` =====

#[test]
fn tuple_literal_infers() {
    // The issue's A3 reproduction: `(1, 2)` is `tuple[int64, int64]`.
    lower_ok(
        "def main():\n\
        \x20   auto o = Optional.Some((1, 2))\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn tuple_literal_via_auto_binding_infers() {
    lower_ok(
        "def main():\n\
        \x20   auto t = (1, 2)\n\
        \x20   auto o = Optional.Some(t)\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn tuple_literal_mixed_element_types_infers() {
    // A heterogeneous but fully-typed tuple: `tuple[int64, str]`.
    lower_ok(
        "def main():\n\
        \x20   auto o = Optional.Some((1, \"a\"))\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn tuple_literal_in_user_generic_struct() {
    lower_ok(
        "struct Box[T]:\n\
        \x20   T item\n\
        def main():\n\
        \x20   auto b = Box((1, 2))\n\
        \x20   print(b.item)\n",
    );
}

// ===== A4 — slice `base[lo:hi]` =====

#[test]
fn slice_list_infers_list() {
    // The issue's A4 reproduction: slicing a `list[int64]` yields `list[int64]`.
    lower_ok(
        "def main():\n\
        \x20   list[int64] xs = [1, 2, 3]\n\
        \x20   auto o = Optional.Some(xs[0:2])\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn slice_str_infers_str() {
    lower_ok(
        "def main():\n\
        \x20   str s = \"hello\"\n\
        \x20   auto o = Optional.Some(s[0:2])\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn slice_via_auto_binding_infers() {
    lower_ok(
        "def main():\n\
        \x20   list[int64] xs = [1, 2, 3]\n\
        \x20   auto part = xs[0:2]\n\
        \x20   auto o = Optional.Some(part)\n\
        \x20   print(o.is_some())\n",
    );
}

// ===== A5 — comprehensions =====

#[test]
fn list_comprehension_infers() {
    // The issue's A5 reproduction: `[x for x in xs]` is `list[int64]`.
    lower_ok(
        "def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto o = Optional.Some([x for x in xs])\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn set_comprehension_infers() {
    lower_ok(
        "def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto o = Optional.Some({x for x in xs})\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn dict_comprehension_infers() {
    lower_ok(
        "def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto o = Optional.Some({x: x for x in xs})\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn comprehension_over_str_iterates_chars() {
    lower_ok(
        "def main():\n\
        \x20   auto o = Optional.Some([c for c in \"hi\"])\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn comprehension_over_range_stays_unknown() {
    // Boundary (the companion #1132): the iterable `range(3)` is a built-in
    // call whose return type the inferer does not model, so the loop variable —
    // and thus the comprehension's element — stays unknown and inference still
    // fails. (When #1132 teaches the inferer built-in returns, this becomes an
    // `lower_ok`.)
    lower_err_contains(
        "def main():\n\
        \x20   auto o = Optional.Some([i for i in range(3)])\n\
        \x20   print(o.is_some())\n",
        "cannot infer type argument",
    );
}

#[test]
fn comprehension_via_auto_binding_infers() {
    lower_ok(
        "def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto ys = [x for x in xs]\n\
        \x20   auto o = Optional.Some(ys)\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn comprehension_in_generic_fn() {
    lower_ok(
        "def wrap[T](T x) -> Optional[T]:\n\
        \x20   return Optional.Some(x)\n\
        def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   print(wrap([x for x in xs]).is_some())\n",
    );
}

// ===== Baselines preserved =====

#[test]
fn list_literal_still_infers() {
    // The existing list-literal element inference must be untouched.
    lower_ok(
        "def main():\n\
        \x20   auto o = Optional.Some([1, 2])\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn explicit_sink_still_infers() {
    // The explicit-type annotation rescue path still works.
    lower_ok(
        "def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   Optional[int64] o = Optional.Some(xs[0])\n\
        \x20   print(o.is_some())\n",
    );
}

// ===== Don't over-accept: fail-closed boundaries =====

#[test]
fn subscript_does_not_mask_unresolvable_param() {
    // `Pair[A, B]` with only `a = xs[0]`: the subscript pins `A`, but `B` is
    // unconstrained, so inference must still fail — and specifically on `B`.
    lower_err_contains(
        "struct Pair[A, B]:\n\
        \x20   A a\n\
        \x20   B b\n\
        def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto p = Pair(a=xs[0])\n\
        \x20   print(p.a)\n",
        "type argument 'B'",
    );
}

#[test]
fn tuple_subscript_stays_unknown() {
    // A tuple is heterogeneous: its element type needs a compile-time-constant
    // index, which the inferer does not model, so `t[0]` stays unresolved and
    // still fails to infer (rather than mis-inferring).
    lower_err_contains(
        "def main():\n\
        \x20   auto t = (1, \"a\")\n\
        \x20   auto o = Optional.Some(t[0])\n\
        \x20   print(o.is_some())\n",
        "cannot infer type argument",
    );
}

#[test]
fn generator_expression_stays_unknown() {
    // A bare generator expression's type is a non-nominal iterator the inferer
    // cannot name, so it is deliberately left unresolved and still fails.
    lower_err_contains(
        "def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto o = Optional.Some((x for x in xs))\n\
        \x20   print(o.is_some())\n",
        "cannot infer type argument",
    );
}
