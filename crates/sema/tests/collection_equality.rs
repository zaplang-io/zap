//! Structural collection equality — sema (type-check) behaviour for #1328.
//!
//! Host-only (parse + sema, no LLVM). `dict`/`set`/`list`/`deque`/`oset` all have
//! well-defined structural `==` / `!=` (value `bool`, accepted here; the actual
//! structural lowering is checked in `crates/ir/tests/collection_eq_lower.rs` and
//! end-to-end by the golden). `heap` has *no* value equality — its internal array
//! order is implementation-defined — so `==` / `!=` on a heap is rejected
//! (ZAP4012), while `is` / `is not` identity stays allowed.

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

// ---- accepted structural equality ----------------------------------------

#[test]
fn dict_equality_typechecks() {
    ok("def main():\n\
        \x20   dict[str, int64] a = {\"x\": 1}\n\
        \x20   dict[str, int64] b = {\"x\": 1}\n\
        \x20   bool eq = a == b\n\
        \x20   bool ne = a != b\n\
        \x20   print(str(eq), str(ne))\n");
}

#[test]
fn set_equality_typechecks() {
    ok("def main():\n\
        \x20   set[int64] a = {1, 2}\n\
        \x20   set[int64] b = {2, 1}\n\
        \x20   print(str(a == b), str(a != b))\n");
}

#[test]
fn deque_equality_typechecks() {
    ok("def main():\n\
        \x20   deque[int64] a = deque[int64]()\n\
        \x20   deque[int64] b = deque[int64]()\n\
        \x20   print(str(a == b))\n");
}

#[test]
fn oset_equality_typechecks() {
    ok("def main():\n\
        \x20   oset[int64] a = oset[int64]([1, 2])\n\
        \x20   oset[int64] b = oset[int64]([1, 2])\n\
        \x20   print(str(a == b))\n");
}

// ---- heap: equality rejected, identity allowed ---------------------------

#[test]
fn heap_eq_is_rejected() {
    code(
        "def main():\n\
        \x20   heap[int64] a = heap[int64]()\n\
        \x20   heap[int64] b = heap[int64]()\n\
        \x20   print(str(a == b))\n",
        "ZAP4012",
    );
}

#[test]
fn heap_ne_is_rejected() {
    code(
        "def main():\n\
        \x20   heap[int64] a = heap[int64]()\n\
        \x20   heap[int64] b = heap[int64]()\n\
        \x20   print(str(a != b))\n",
        "ZAP4012",
    );
}

#[test]
fn heap_identity_is_allowed() {
    // `is` / `is not` test reference identity and must stay accepted on a heap.
    ok("def main():\n\
        \x20   heap[int64] a = heap[int64]()\n\
        \x20   heap[int64] b = heap[int64]()\n\
        \x20   print(str(a is b), str(a is not b))\n");
}
