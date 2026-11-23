//! Ordering comparisons on collections — lowering (ZIR) checks for #1334.
//!
//! `< <= > >=` on `dict`/`set`/`heap`/`oset`/`deque` used to fall through
//! `binop`'s generic numeric path, which `ptrtoint`-compares the two container
//! headers (a meaningless allocation-address compare). The fix: `set` gets
//! subset/superset semantics, `deque` gets lexicographic ordering, and
//! `dict`/`heap`/`oset` are rejected (they have no ordering). These host-only
//! lowering checks (no LLVM) assert the structural lowering / rejection; the
//! end-to-end values are covered by the `collection_ordering` golden on the LLVM
//! CI matrix.

use zapc_ir::{lower_module, Inst, Module};
use zapc_syntax::parser::parse_str;

fn lower_ok(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    module
}

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

// ----------------------------------------------------------------- set --------

#[test]
fn set_subset_is_membership_structural() {
    // `a <= b` (subset) must walk a's members and probe membership in b, not
    // pointer-compare the two set headers.
    let m = lower_ok(
        "def main():\n\
        \x20   set[int64] a = {1, 2}\n\
        \x20   set[int64] b = {1, 2, 3}\n\
        \x20   print(str(a <= b))\n",
    );
    assert!(has(&m, "zap_set_len"), "set `<=` must read lengths");
    assert!(
        has(&m, "zap_set_items"),
        "set `<=` must materialize the left members"
    );
    assert!(
        has_prefix(&m, "zap_set_contains_"),
        "set `<=` must probe membership"
    );
}

#[test]
fn set_strict_subset_and_superset_lower() {
    for (op, _why) in [
        ("<", "proper subset"),
        (">", "proper superset"),
        (">=", "superset"),
    ] {
        let m = lower_ok(&format!(
            "def main():\n\
            \x20   set[int64] a = {{1, 2}}\n\
            \x20   set[int64] b = {{1, 2, 3}}\n\
            \x20   print(str(a {op} b))\n",
        ));
        assert!(
            has(&m, "zap_set_len") && has_prefix(&m, "zap_set_contains_"),
            "set `{op}` must be membership-structural"
        );
    }
}

// ---------------------------------------------------------------- deque -------

#[test]
fn deque_ordering_is_lexicographic_structural() {
    let m = lower_ok(
        "def main():\n\
        \x20   deque[int64] a = deque[int64]()\n\
        \x20   a.append(1)\n\
        \x20   deque[int64] b = deque[int64]()\n\
        \x20   b.append(2)\n\
        \x20   print(str(a < b))\n",
    );
    assert!(has(&m, "zap_deque_len"), "deque `<` must read lengths");
    assert!(
        has(&m, "zap_deque_get_i64"),
        "deque `<` must walk elements for lexicographic order"
    );
}

#[test]
fn deque_str_ordering_uses_str_cmp() {
    let m = lower_ok(
        "def main():\n\
        \x20   deque[str] a = deque[str]()\n\
        \x20   a.append(\"a\")\n\
        \x20   deque[str] b = deque[str]()\n\
        \x20   b.append(\"b\")\n\
        \x20   print(str(a < b))\n",
    );
    assert!(
        has(&m, "zap_str_cmp"),
        "str element ordering must use zap_str_cmp"
    );
}

// --------------------------------------------- dict / heap / oset rejected ----

#[test]
fn dict_ordering_is_rejected() {
    lower_err_contains(
        "def main():\n\
        \x20   dict[str, int64] a = {\"x\": 1}\n\
        \x20   dict[str, int64] b = {\"y\": 2}\n\
        \x20   print(str(a < b))\n",
        "not orderable",
    );
}

#[test]
fn heap_ordering_is_rejected() {
    lower_err_contains(
        "def main():\n\
        \x20   heap[int64] a = heap[int64]()\n\
        \x20   heap[int64] b = heap[int64]()\n\
        \x20   print(str(a < b))\n",
        "not orderable",
    );
}

#[test]
fn oset_ordering_is_rejected() {
    lower_err_contains(
        "def main():\n\
        \x20   oset[int64] a = oset[int64]([1])\n\
        \x20   oset[int64] b = oset[int64]([2])\n\
        \x20   print(str(a < b))\n",
        "not orderable",
    );
}
