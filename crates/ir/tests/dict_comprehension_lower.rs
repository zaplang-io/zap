//! Comprehensions iterating a `dict` directly (#1154).
//!
//! Iterating a `dict` yields its keys (Python semantics, #299): the statement
//! `for k in d` loop has always lowered and run, going through
//! `lower_for` → `lower_for_dict` (which materializes the keys via
//! `zap_dict_keys_*`). Comprehensions, however, lower their clause iteration
//! through the separate `Lowerer::iterate` protocol, which had no `Ty::Dict`
//! arm — so `[k for k in d]` / `{k for k in d}` / `(k for k in d)` fell through
//! to the catch-all and bailed with `value of type Dict(...) is not iterable
//! (D-0048)`, even though sema's `iterable_elem` already permits it
//! (`Dict(k, _) => Some(k)`). It was a sema-accepts / lowering-rejects mismatch
//! in the comprehension path alone.
//!
//! These host-only lowering checks (no LLVM) pin that every comprehension form
//! now lowers over a dict, across more than one key suffix, with a filter, and
//! nested inside an `Optional.Some(...)` (the shape that surfaced the gap under
//! #1131). They also pin that the unchanged iterables (the statement-`for` dict
//! path, and list / set / str / range comprehensions) still lower, so the new
//! `Dict` arm cannot silently regress a sibling. The end-to-end key *values*
//! (counts / sums over the materialized keys) are covered by the
//! `dict_comprehension_keys` golden on CI.

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

// ----- #1154: each comprehension form iterating a dict directly -----

#[test]
fn list_comprehension_over_dict_lowers() {
    // The issue's minimal repro: a list comprehension iterating a dict's keys.
    lower_ok(
        "def main():\n\
        \x20   dict[str, int64] d = {\"a\": 1, \"b\": 2}\n\
        \x20   list[str] ks = [k for k in d]\n\
        \x20   print(len(ks))\n",
    );
}

#[test]
fn set_comprehension_over_dict_lowers() {
    lower_ok(
        "def main():\n\
        \x20   dict[str, int64] d = {\"a\": 1, \"b\": 2}\n\
        \x20   set[str] ks = {k for k in d}\n\
        \x20   print(len(ks))\n",
    );
}

#[test]
fn generator_expression_over_dict_lowers() {
    // A bare generator expression over a dict, consumed by a `for` loop over
    // the resulting lazy iterator.
    lower_ok(
        "def main():\n\
        \x20   dict[str, int64] d = {\"a\": 1, \"b\": 2}\n\
        \x20   auto n = 0\n\
        \x20   for k in (k for k in d):\n\
        \x20       n = n + 1\n\
        \x20   print(n)\n",
    );
}

#[test]
fn dict_comprehension_over_dict_lowers() {
    // A dict comprehension whose source clause iterates a dict: `{k: k for k
    // in d}` rebuilds a dict keyed by the source keys.
    lower_ok(
        "def main():\n\
        \x20   dict[str, int64] d = {\"a\": 1, \"b\": 2}\n\
        \x20   dict[str, str] m = {k: k for k in d}\n\
        \x20   print(len(m))\n",
    );
}

// ----- key-suffix breadth: exercise more than one `zap_dict_keys_*` -----

#[test]
fn list_comprehension_over_int_keyed_dict_lowers() {
    // `dict[int64, _]` hits a different `zap_dict_keys_*` suffix than the
    // `str`-keyed cases above.
    lower_ok(
        "def main():\n\
        \x20   dict[int64, str] d = {1: \"a\", 2: \"b\"}\n\
        \x20   list[int64] ks = [k for k in d]\n\
        \x20   print(len(ks))\n",
    );
}

#[test]
fn set_comprehension_over_int_keyed_dict_lowers() {
    lower_ok(
        "def main():\n\
        \x20   dict[int64, str] d = {1: \"a\", 2: \"b\"}\n\
        \x20   set[int64] ks = {k for k in d}\n\
        \x20   print(len(ks))\n",
    );
}

// ----- edge cases: empty dict, a filter, and nested Optional.Some -----

#[test]
fn comprehension_over_empty_dict_lowers() {
    // An empty dict literal still has a concrete key type, so the comprehension
    // lowers (the materialized key list is simply empty at runtime).
    lower_ok(
        "def main():\n\
        \x20   dict[str, int64] d = {}\n\
        \x20   list[str] ks = [k for k in d]\n\
        \x20   print(len(ks))\n",
    );
}

#[test]
fn list_comprehension_over_dict_with_filter_lowers() {
    // A trailing `if` filter on the dict-key clause.
    lower_ok(
        "def main():\n\
        \x20   dict[int64, str] d = {1: \"a\", 2: \"b\", 3: \"c\"}\n\
        \x20   list[int64] ks = [k for k in d if k > 1]\n\
        \x20   print(len(ks))\n",
    );
}

#[test]
fn dict_comprehension_nested_in_optional_some_lowers() {
    // The exact shape that surfaced the gap under #1131: the comprehension's
    // inferred element type lands it inside an `Optional.Some(...)` payload.
    lower_ok(
        "def main():\n\
        \x20   dict[str, int64] d = {\"a\": 1, \"b\": 2}\n\
        \x20   Optional[list[str]] maybe = Optional.Some([k for k in d])\n\
        \x20   print(maybe.is_some())\n",
    );
}

// ----- unchanged: the statement-`for` dict path and sibling iterables -----

#[test]
fn statement_for_over_dict_still_lowers() {
    // The proven statement-`for` dict path (`lower_for_dict`) must be untouched.
    lower_ok(
        "def main():\n\
        \x20   dict[str, int64] d = {\"a\": 1, \"b\": 2}\n\
        \x20   for k in d:\n\
        \x20       print(k)\n",
    );
}

#[test]
fn list_comprehension_over_list_still_lowers() {
    lower_ok(
        "def main():\n\
        \x20   list[int64] xs = [1, 2, 3]\n\
        \x20   list[int64] ys = [x for x in xs]\n\
        \x20   print(len(ys))\n",
    );
}

#[test]
fn set_comprehension_over_set_still_lowers() {
    lower_ok(
        "def main():\n\
        \x20   set[int64] xs = {1, 2, 3}\n\
        \x20   set[int64] ys = {x for x in xs}\n\
        \x20   print(len(ys))\n",
    );
}

#[test]
fn comprehension_over_str_still_lowers() {
    lower_ok(
        "def main():\n\
        \x20   str s = \"abc\"\n\
        \x20   list[char] cs = [c for c in s]\n\
        \x20   print(len(cs))\n",
    );
}

#[test]
fn comprehension_over_range_still_lowers() {
    lower_ok(
        "def main():\n\
        \x20   list[int64] xs = [i for i in range(3)]\n\
        \x20   print(len(xs))\n",
    );
}
