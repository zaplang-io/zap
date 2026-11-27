//! A collection display whose FIRST element is an empty/ambiguous collection
//! literal must take its element type from a non-empty sibling, exactly as sema
//! already joins it — not default off the empty element and then fail at
//! lowering (#1708).
//!
//! `[[], ["group", "switch"]]` has no type annotation, so lowering infers the
//! element kind. It used to read the kind off the FIRST element: the empty `[]`
//! defaults to `list[int64]`, so the sibling `list[str]` was rejected with
//! `error[ZAP6001]: list element expects i64, found ptr` — even though sema had
//! accepted the program and joined the siblings to `list[list[str]]`. Lowering
//! now consults sema's inferred type (the #1167 `TypeMap` the driver threads in)
//! for exactly this nested-collection case, so it agrees with sema instead of
//! accepting the expression and failing to store a sibling.
//!
//! This exercises the real integration path (`analyze_types` →
//! `lower_module_opts_with_types`), host-only (no LLVM): lowering runs the mono
//! pass. The end-to-end runtime behavior (the printed `0`/`2`, the inner `str`
//! elements, the dict-value counterpart) is pinned by the companion golden
//! `tests/golden/nested_empty_collection_infer.zap`.

use zapc_ir::{lower_module, lower_module_opts_with_types};
use zapc_sema::analyze_types;
use zapc_syntax::parser::parse_str;

/// `src` type-checks, and lowering it WITH sema's types infers the sibling's
/// element type (no lowering error). The leading-empty cases additionally fail
/// WITHOUT the types — proving sema's joined type, not a change to lowering's
/// own first-element heuristic, is what closes the gap (same contract as the
/// #1167 `sema_backfill` suite).
fn infers_with_sema_types(src: &str, fails_without_types: bool) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");

    let (errs, _warns, types) = analyze_types(&ast);
    assert!(
        errs.is_empty(),
        "sema should accept the program, got: {errs:?}"
    );

    let (_zir, with_errs) = lower_module_opts_with_types(&ast, false, &types);
    assert!(
        with_errs.is_empty(),
        "lowering WITH sema types should infer from the sibling, got: {with_errs:?}"
    );

    if fails_without_types {
        let (_zir, without_errs) = lower_module(&ast);
        assert!(
            without_errs
                .iter()
                .any(|e| e.message.contains("list element expects")
                    || e.message.contains("element kind")
                    || e.message.contains("representation")),
            "expected lowering WITHOUT sema types to still fail on the leading \
             empty element (the first-element heuristic); if this now passes, \
             the test no longer guards the #1708 inference path. errs: {without_errs:?}"
        );
    }
}

/// The exact issue reproduction: a leading empty list, then a `list[str]`.
#[test]
fn leading_empty_list_infers_from_str_sibling() {
    infers_with_sema_types(
        "def main():\n\
        \x20   for entry in [[], [\"group\", \"switch\"]]:\n\
        \x20       print(len(entry))\n",
        true,
    );
}

/// The other ordering — an empty member AFTER the non-empty sibling — already
/// lowered (the first element anchors the type), and must keep lowering.
#[test]
fn trailing_empty_list_still_infers() {
    infers_with_sema_types(
        "def main():\n\
        \x20   for entry in [[\"group\", \"switch\"], []]:\n\
        \x20       print(len(entry))\n",
        false,
    );
}

/// An empty member BETWEEN two non-empty siblings: the anchor is element 0, so
/// this already lowered, but it must stay correct under the new inference.
#[test]
fn empty_between_siblings_infers() {
    infers_with_sema_types(
        "def main():\n\
        \x20   for entry in [[\"a\"], [], [\"b\", \"c\", \"d\"]]:\n\
        \x20       print(len(entry))\n",
        false,
    );
}

/// Deep nesting — the leading empty is two levels down (`[[]]`). The outer
/// first element is itself a non-empty list, so the naive heuristic recursed
/// into its empty inner and still defaulted to `i64`; sema's join fixes it.
#[test]
fn deep_nested_leading_empty_infers() {
    infers_with_sema_types(
        "def main():\n\
        \x20   for row in [[[]], [[\"a\", \"b\"]]]:\n\
        \x20       print(len(row))\n",
        true,
    );
}

/// The dict counterpart: a leading empty list VALUE. The value kind used to
/// default off the empty `[]`, rejecting the `list[str]` sibling value.
#[test]
fn dict_leading_empty_list_value_infers() {
    infers_with_sema_types(
        "def main():\n\
        \x20   print(len({1: [], 2: [\"x\", \"y\"]}[1]))\n\
        \x20   print(len({1: [], 2: [\"x\", \"y\"]}[2]))\n",
        true,
    );
}

/// A nested-dict VALUE with a leading empty dict (`{}`), the dict-of-dict
/// counterpart of the list case.
#[test]
fn dict_leading_empty_dict_value_infers() {
    infers_with_sema_types(
        "def main():\n\
        \x20   print(len({\"a\": {}, \"b\": {\"k\": \"v\"}}[\"a\"]))\n\
        \x20   print(len({\"a\": {}, \"b\": {\"k\": \"v\"}}[\"b\"]))\n",
        true,
    );
}

/// A scalar-element display with a leading element that is already a usable
/// anchor is unaffected — it lowers the same with or without sema's types.
#[test]
fn scalar_list_is_unaffected() {
    infers_with_sema_types(
        "def main():\n\
        \x20   for n in [1, 2, 3]:\n\
        \x20       print(n)\n",
        false,
    );
}

/// An all-empty display (`[[], []]`) has no informative sibling: sema types it
/// `list[list[Unknown]]`, which does not bridge, so lowering keeps its default
/// (`list[list[int64]]`) consistently and still lowers — no regression, no
/// spurious error.
#[test]
fn all_empty_nested_display_lowers() {
    infers_with_sema_types(
        "def main():\n\
        \x20   for entry in [[], []]:\n\
        \x20       print(len(entry))\n",
        false,
    );
}
