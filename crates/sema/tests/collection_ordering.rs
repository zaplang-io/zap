//! Ordering comparisons on collections — sema (type-check) for #1334.
//!
//! `set` (subset/superset) and `deque` (lexicographic) have well-defined Python
//! orderings and type-check as `bool`. `dict`, `heap`, and `oset` have no
//! ordering, so `< <= > >=` on them is ZAP4012; `is` / `is not` identity stays
//! allowed. Host-only (parse + sema, no LLVM).

use zapc_sema::check_str;

#[track_caller]
fn errors(src: &str) -> Vec<(String, String)> {
    let (terrs, perrs, lerrs) = check_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    terrs
        .into_iter()
        .map(|e| (e.code.to_string(), e.message))
        .collect()
}

#[track_caller]
fn ok(src: &str) {
    let e = errors(src);
    assert!(e.is_empty(), "expected no sema errors, got: {e:?}");
}

#[track_caller]
fn code(src: &str, want: &str) {
    let e = errors(src);
    assert!(
        e.iter().any(|(c, _)| c == want),
        "expected a {want} error, got: {e:?}",
    );
}

// ---- accepted orderings --------------------------------------------------

#[test]
fn set_ordering_typechecks() {
    for op in ["<", "<=", ">", ">="] {
        ok(&format!(
            "def main():\n\
            \x20   set[int64] a = {{1}}\n\
            \x20   set[int64] b = {{1, 2}}\n\
            \x20   print(str(a {op} b))\n",
        ));
    }
}

#[test]
fn deque_ordering_typechecks() {
    for op in ["<", "<=", ">", ">="] {
        ok(&format!(
            "def main():\n\
            \x20   deque[int64] a = deque[int64]()\n\
            \x20   deque[int64] b = deque[int64]()\n\
            \x20   print(str(a {op} b))\n",
        ));
    }
}

// ---- rejected orderings (dict / heap / oset) -----------------------------

#[test]
fn dict_ordering_is_rejected() {
    for op in ["<", "<=", ">", ">="] {
        code(
            &format!(
                "def main():\n\
                \x20   dict[str, int64] a = {{\"x\": 1}}\n\
                \x20   dict[str, int64] b = {{\"y\": 2}}\n\
                \x20   print(str(a {op} b))\n",
            ),
            "ZAP4012",
        );
    }
}

#[test]
fn heap_ordering_is_rejected() {
    code(
        "def main():\n\
        \x20   heap[int64] a = heap[int64]()\n\
        \x20   heap[int64] b = heap[int64]()\n\
        \x20   print(str(a < b))\n",
        "ZAP4012",
    );
}

#[test]
fn oset_ordering_is_rejected() {
    code(
        "def main():\n\
        \x20   oset[int64] a = oset[int64]([1])\n\
        \x20   oset[int64] b = oset[int64]([2])\n\
        \x20   print(str(a >= b))\n",
        "ZAP4012",
    );
}

#[test]
fn dict_identity_still_allowed() {
    ok("def main():\n\
        \x20   dict[str, int64] a = {\"x\": 1}\n\
        \x20   dict[str, int64] b = {\"x\": 1}\n\
        \x20   print(str(a is b), str(a is not b))\n");
}
