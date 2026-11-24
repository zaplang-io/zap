//! An `Optional[U]` value must not be silently accepted where an `Optional[T]`
//! with a *different* payload type is expected, and then mis-compiled (#1407).
//!
//! Sema erases the `Optional` payload (it tracks every `Optional[T]` as opaque
//! `Named("Optional")`), so the #1265 bare-payload widening and the top-level
//! `assignable` identity are payload-agnostic: `Optional[U]` is "assignable" to
//! `Optional[T]` for *any* `U`, `T`. So an explicit `Optional.Some(v)` whose
//! argument monomorphizes a different payload — `Optional.Some(<bigint value>)`,
//! `Optional.Some(x)` for an `x: int32`, or for a `str` — reaches IR lowering as
//! `Optional$U`, and with no `Optional`-to-`Optional` arm in `coerce` its
//! `Optional$U` pointer is stored raw into the `Optional$T` slot. The two
//! monomorphizations disagree on both physical layout (a niche pointer-payload
//! `Optional$bigint`/`str` vs a tagged `Optional$int64`, or a narrower tagged
//! payload) and method dispatch, so a later `is_some()`/`unwrap()` reads garbage
//! or raises — a memory-unsafe miscompile in a checked program.
//!
//! This is the sibling of the bare-`bigint` case #1379 (PR #1403) fixed: that
//! caught a bare `bigint` *literal* flowing into a fixed-width-numeric payload
//! (lowering sees `bigint` -> `int64` and rejects it), but a value whose
//! *Optional* has already monomorphized `Optional$U` slips the same check. The
//! fix is payload-aware `Optional`-to-`Optional` assignability in lowering
//! (`check_optional_payload`): both payloads must be identical, at the shared
//! `expr_coerced` chokepoint (typed binding, reassignment, call argument, return,
//! constructor field) and the `Optional`-wrap arm of `expr_collection_context`
//! (collection elements). A bare payload is untouched — the #1265 bare-payload
//! wrap still widens a *matching* numeric payload (`Optional[int64] x = n` for an
//! `int32 n`).
//!
//! Host-only (`lower_module`): the rejection is in lowering, so it surfaces here.

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

// ---------------------------------------------------------------------------
// Rejected: Optional.Some(<value of a different payload type>) into Optional[T]
// ---------------------------------------------------------------------------

#[test]
fn some_bigint_value_into_optional_int64_rejected() {
    // The issue's exact reproduction: a bigint *variable* (so `Optional.Some(g)`
    // monomorphizes `Optional$bigint`), not the bare `bigint` literal #1379
    // already rejects.
    lower_err_contains(
        "def main():\n\
        \x20   bigint g = 9223372036854775808\n\
        \x20   Optional[int64] x = Optional.Some(g)\n\
        \x20   print(x.is_some())\n",
        "Optional",
    );
}

#[test]
fn some_int32_value_into_optional_int64_rejected() {
    // A narrower fixed-width payload is still a layout mismatch (tagged
    // `Optional$int32` vs tagged `Optional$int64`): reject, don't relabel.
    lower_err_contains(
        "def main():\n\
        \x20   int32 n = 5\n\
        \x20   Optional[int64] x = Optional.Some(n)\n\
        \x20   print(x.is_some())\n",
        "Optional",
    );
}

#[test]
fn some_int64_value_into_optional_int32_rejected() {
    // ...and the reverse narrowing direction.
    lower_err_contains(
        "def main():\n\
        \x20   int64 n = 5\n\
        \x20   Optional[int32] x = Optional.Some(n)\n\
        \x20   print(x.is_some())\n",
        "Optional",
    );
}

#[test]
fn some_str_value_into_optional_int64_rejected() {
    // A pointer payload (`str`, niche) into a tagged numeric `Optional` — the
    // worst case: a `str` heap pointer stored where an `int64` is read back.
    lower_err_contains(
        "def main():\n\
        \x20   str s = \"hi\"\n\
        \x20   Optional[int64] x = Optional.Some(s)\n\
        \x20   print(x.is_some())\n",
        "Optional",
    );
}

#[test]
fn some_int64_value_into_optional_str_rejected() {
    // ...and a scalar payload into a niche pointer `Optional`.
    lower_err_contains(
        "def main():\n\
        \x20   int64 n = 5\n\
        \x20   Optional[str] x = Optional.Some(n)\n\
        \x20   print(x.is_some())\n",
        "Optional",
    );
}

#[test]
fn some_float64_value_into_optional_int64_rejected() {
    lower_err_contains(
        "def main():\n\
        \x20   float64 f = 1.5\n\
        \x20   Optional[int64] x = Optional.Some(f)\n\
        \x20   print(x.is_some())\n",
        "Optional",
    );
}

// ----- the same mismatch in every type-contextual position -----

#[test]
fn reassign_optional_mismatch_rejected() {
    lower_err_contains(
        "def main():\n\
        \x20   bigint g = 9223372036854775808\n\
        \x20   Optional[int64] b = None\n\
        \x20   b = Optional.Some(g)\n\
        \x20   print(b.unwrap())\n",
        "Optional",
    );
}

#[test]
fn call_arg_optional_mismatch_rejected() {
    lower_err_contains(
        "def f(Optional[int64] o):\n\
        \x20   print(o.is_some())\n\
        def main():\n\
        \x20   bigint g = 9223372036854775808\n\
        \x20   f(Optional.Some(g))\n",
        "Optional",
    );
}

#[test]
fn return_optional_mismatch_rejected() {
    lower_err_contains(
        "def f() -> Optional[int64]:\n\
        \x20   bigint g = 9223372036854775808\n\
        \x20   return Optional.Some(g)\n\
        def main():\n\
        \x20   print(f().is_some())\n",
        "Optional",
    );
}

#[test]
fn struct_optional_field_mismatch_rejected() {
    lower_err_contains(
        "struct Box:\n\
        \x20   Optional[int64] f = None\n\
        def main():\n\
        \x20   bigint g = 9223372036854775808\n\
        \x20   Box b = Box(Optional.Some(g))\n\
        \x20   print(b.f.is_some())\n",
        "Optional",
    );
}

#[test]
fn list_element_optional_mismatch_rejected() {
    // Reaches `expr_collection_context` directly, not via `expr_coerced`.
    lower_err_contains(
        "def main():\n\
        \x20   bigint g = 9223372036854775808\n\
        \x20   list[Optional[int64]] xs = [Optional.Some(g)]\n\
        \x20   print(xs[0].is_some())\n",
        "Optional",
    );
}

#[test]
fn dict_value_optional_mismatch_rejected() {
    lower_err_contains(
        "def main():\n\
        \x20   bigint g = 9223372036854775808\n\
        \x20   dict[str, Optional[int64]] d = {\"k\": Optional.Some(g)}\n\
        \x20   print(d[\"k\"].is_some())\n",
        "Optional",
    );
}

#[test]
fn append_optional_mismatch_rejected() {
    lower_err_contains(
        "def main():\n\
        \x20   bigint g = 9223372036854775808\n\
        \x20   list[Optional[int64]] xs = []\n\
        \x20   xs.append(Optional.Some(g))\n\
        \x20   print(len(xs))\n",
        "Optional",
    );
}

// ---------------------------------------------------------------------------
// Accepted: a genuine matching payload still lowers (no over-rejection)
// ---------------------------------------------------------------------------

#[test]
fn genuine_optional_bigint_accepted() {
    // The partner of the first rejected case: a matching `Optional[bigint]`.
    lower_ok(
        "def main():\n\
        \x20   bigint g = 9223372036854775808\n\
        \x20   Optional[bigint] x = Optional.Some(g)\n\
        \x20   print(x.is_some())\n",
    );
}

#[test]
fn optional_int64_matching_payload_accepted() {
    lower_ok(
        "def main():\n\
        \x20   int64 n = 5\n\
        \x20   Optional[int64] x = Optional.Some(n)\n\
        \x20   print(x.unwrap())\n",
    );
}

#[test]
fn optional_str_matching_payload_accepted() {
    lower_ok(
        "def main():\n\
        \x20   str s = \"hi\"\n\
        \x20   Optional[str] x = Optional.Some(s)\n\
        \x20   print(x.unwrap())\n",
    );
}

#[test]
fn optional_to_optional_binding_same_payload_accepted() {
    // A whole `Optional[int64]` value copied into another `Optional[int64]`
    // binding: identical payload, so never rejected.
    lower_ok(
        "def main():\n\
        \x20   Optional[int64] a = Optional.Some(5)\n\
        \x20   Optional[int64] b = a\n\
        \x20   print(b.is_some())\n",
    );
}

#[test]
fn bare_payload_numeric_widening_still_accepted() {
    // The #1265 bare-payload wrap must be untouched: a bare `int32` widens into
    // an `Optional[int64]` (the value is NOT itself an `Optional`, so the
    // payload-variance check never fires; `opt_payload_source` widens it).
    lower_ok(
        "def main():\n\
        \x20   int32 n = 5\n\
        \x20   Optional[int64] x = n\n\
        \x20   print(x.unwrap())\n",
    );
}

#[test]
fn bare_none_into_optional_still_accepted() {
    // `None` is lowered by `lower_none_expr` before any coercion, so the check
    // never sees it.
    lower_ok(
        "def main():\n\
        \x20   Optional[int64] x = None\n\
        \x20   print(x.is_none())\n",
    );
}

#[test]
fn optional_list_matching_element_accepted() {
    lower_ok(
        "def main():\n\
        \x20   int64 n = 5\n\
        \x20   list[Optional[int64]] xs = [Optional.Some(n)]\n\
        \x20   print(xs[0].unwrap())\n",
    );
}

#[test]
fn nested_optional_matching_payload_accepted() {
    // `Optional[Optional[int64]]` on both sides: the top-level payloads match.
    lower_ok(
        "def main():\n\
        \x20   Optional[int64] inner = Optional.Some(5)\n\
        \x20   Optional[Optional[int64]] x = Optional.Some(inner)\n\
        \x20   print(x.is_some())\n",
    );
}
