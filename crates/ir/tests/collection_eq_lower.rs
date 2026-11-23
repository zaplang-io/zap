//! Structural collection equality — lowering (ZIR) checks for #1328.
//!
//! The dict counterpart to the list-equality fix (#1239), widened across the
//! core collections. Before the fix, `==` / `!=` on `dict`/`set`/`oset`/`deque`
//! fell through to the generic numeric/reference path, which `ptrtoint`-compared
//! the two container headers: an identity test, so two separately allocated but
//! equal containers reported unequal. These are host-only lowering checks (no
//! LLVM): they lower a program to ZIR and assert that equality dispatches to a
//! *structural* comparison (reads lengths, walks entries/elements/members) rather
//! than a bare pointer compare, and that `heap` equality is rejected. The
//! end-to-end runtime *values* are covered by the `collection_structural_eq`
//! golden on the LLVM CI matrix.

use zapc_ir::{lower_module, Inst, Module};
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting no lex/parse/lower errors, returning the ZIR.
fn lower_ok(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    module
}

/// Parse + lower `src`, asserting lowering *fails* with an error whose text
/// contains `needle`.
fn lower_err_contains(src: &str, needle: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(
        errs.iter().any(|e| format!("{e:?}").contains(needle)),
        "expected a lowering error containing {needle:?}; got {errs:?}"
    );
}

/// Every runtime symbol named by a `Call`/`Invoke` anywhere in the module.
fn called_symbols(m: &Module) -> Vec<String> {
    let mut out = Vec::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                match inst {
                    Inst::Call { func, .. } | Inst::Invoke { func, .. } => out.push(func.clone()),
                    _ => {}
                }
            }
        }
    }
    out
}

fn has(m: &Module, sym: &str) -> bool {
    called_symbols(m).iter().any(|s| s == sym)
}

fn has_prefix(m: &Module, prefix: &str) -> bool {
    called_symbols(m).iter().any(|s| s.starts_with(prefix))
}

// ---------------------------------------------------------------- dict --------

#[test]
fn dict_eq_is_structural_str_keys() {
    // The #1328 reproduction: equal dicts built in a different key order. Must
    // read both lengths, materialize keys, probe membership, and fetch values —
    // not compare the two dict pointers.
    let m = lower_ok(
        "def main():\n\
        \x20   dict[str, int64] a = {\"x\": 1, \"y\": 2}\n\
        \x20   dict[str, int64] b = {\"y\": 2, \"x\": 1}\n\
        \x20   print(str(a == b))\n",
    );
    assert!(has(&m, "zap_dict_len"), "dict `==` must read lengths");
    assert!(
        has_prefix(&m, "zap_dict_keys_"),
        "dict `==` must materialize the left keys"
    );
    assert!(
        has_prefix(&m, "zap_dict_contains_"),
        "dict `==` must probe the right dict for each key"
    );
    assert!(
        has_prefix(&m, "zap_dict_get_"),
        "dict `==` must fetch both values to compare them"
    );
}

#[test]
fn dict_eq_int_keys_structural() {
    let m = lower_ok(
        "def main():\n\
        \x20   dict[int64, int64] a = {1: 10, 2: 20}\n\
        \x20   dict[int64, int64] b = {2: 20, 1: 10}\n\
        \x20   print(str(a == b))\n",
    );
    assert!(has(&m, "zap_dict_len"));
    assert!(has(&m, "zap_dict_keys_i64"));
    assert!(has(&m, "zap_dict_contains_i64"));
}

#[test]
fn dict_eq_str_values_compare_by_contents() {
    // A `str` value must compare through `zap_str_eq`, not a pointer identity on
    // each value.
    let m = lower_ok(
        "def main():\n\
        \x20   dict[str, str] a = {\"k\": \"v\"}\n\
        \x20   dict[str, str] b = {\"k\": \"v\"}\n\
        \x20   print(str(a == b))\n",
    );
    assert!(has(&m, "zap_dict_get_str_ptr"));
    assert!(
        has(&m, "zap_str_eq"),
        "str values must compare by contents (zap_str_eq)"
    );
}

#[test]
fn dict_eq_nested_list_values_recurse() {
    // A `list` value type supplies its own structural equality (#1239), so the
    // dict compare must recurse into it: a nested `zap_list_len` must appear.
    let m = lower_ok(
        "def main():\n\
        \x20   dict[str, list[int64]] a = {\"k\": [1, 2]}\n\
        \x20   dict[str, list[int64]] b = {\"k\": [1, 2]}\n\
        \x20   print(str(a == b))\n",
    );
    assert!(has(&m, "zap_dict_get_str_ptr"));
    assert!(
        has(&m, "zap_list_len"),
        "a nested list value must recurse into structural list equality"
    );
}

#[test]
fn dict_ne_is_structural_too() {
    let m = lower_ok(
        "def main():\n\
        \x20   dict[str, int64] a = {\"x\": 1}\n\
        \x20   dict[str, int64] b = {\"x\": 2}\n\
        \x20   print(str(a != b))\n",
    );
    assert!(has(&m, "zap_dict_len"), "dict `!=` must also be structural");
}

#[test]
fn dict_identity_still_pointer_compare() {
    // `is` / `is not` must NOT become structural — they remain a pointer compare,
    // so no dict traversal symbols are emitted for the identity test itself.
    let m = lower_ok(
        "def main():\n\
        \x20   dict[str, int64] a = {\"x\": 1}\n\
        \x20   dict[str, int64] b = {\"x\": 1}\n\
        \x20   print(str(a is not b))\n",
    );
    // No keys/contains traversal is emitted for identity (only the two literals).
    assert!(
        !has_prefix(&m, "zap_dict_keys_") && !has_prefix(&m, "zap_dict_contains_"),
        "`is not` on dicts must stay a pointer identity test, not structural"
    );
}

// ----------------------------------------------------------------- set --------

#[test]
fn set_eq_is_membership_structural() {
    let m = lower_ok(
        "def main():\n\
        \x20   set[int64] a = {1, 2, 3}\n\
        \x20   set[int64] b = {3, 2, 1}\n\
        \x20   print(str(a == b))\n",
    );
    assert!(has(&m, "zap_set_len"), "set `==` must read lengths");
    assert!(
        has(&m, "zap_set_items"),
        "set `==` must materialize the left members"
    );
    assert!(
        has_prefix(&m, "zap_set_contains_"),
        "set `==` must probe membership in the right set"
    );
}

#[test]
fn set_ne_is_structural_too() {
    let m = lower_ok(
        "def main():\n\
        \x20   set[str] a = {\"a\"}\n\
        \x20   set[str] b = {\"b\"}\n\
        \x20   print(str(a != b))\n",
    );
    assert!(has(&m, "zap_set_len"));
    assert!(has(&m, "zap_set_contains_str"));
}

// ---------------------------------------------------------------- deque -------

#[test]
fn deque_eq_is_order_sensitive_structural() {
    let m = lower_ok(
        "def main():\n\
        \x20   deque[int64] a = deque[int64]()\n\
        \x20   a.append(1)\n\
        \x20   a.append(2)\n\
        \x20   deque[int64] b = deque[int64]()\n\
        \x20   b.append(1)\n\
        \x20   b.append(2)\n\
        \x20   print(str(a == b))\n",
    );
    assert!(has(&m, "zap_deque_len"), "deque `==` must read lengths");
    assert!(
        has(&m, "zap_deque_get_i64"),
        "deque `==` must walk elements positionally"
    );
}

// ----------------------------------------------------------------- oset -------

#[test]
fn oset_eq_is_order_sensitive_structural() {
    let m = lower_ok(
        "def main():\n\
        \x20   oset[int64] a = oset[int64]([1, 2, 3])\n\
        \x20   oset[int64] b = oset[int64]([1, 2, 3])\n\
        \x20   print(str(a == b))\n",
    );
    assert!(has(&m, "zap_oset_len"), "oset `==` must read lengths");
    assert!(
        has(&m, "zap_oset_get_i64"),
        "oset `==` must walk elements positionally (order-sensitive)"
    );
}

// ----------------------------------------------------------------- heap -------

#[test]
fn heap_eq_is_rejected() {
    // A heap's internal order is implementation-defined, so `==` is undefined and
    // must be rejected (lowering backstop; sema reports it in the normal pipeline).
    lower_err_contains(
        "def main():\n\
        \x20   heap[int64] a = heap[int64]()\n\
        \x20   a.push(1)\n\
        \x20   heap[int64] b = heap[int64]()\n\
        \x20   b.push(1)\n\
        \x20   print(str(a == b))\n",
        "value equality",
    );
}
