//! Lexicographic list ordering — lowering (ZIR) checks for #1334 (list sibling).
//!
//! `< <= > >=` on two lists used to fall through `binop`'s generic numeric path,
//! which `ptrtoint`-compares the two list headers (a meaningless
//! allocation-address compare). The fix lowers it to a structural lexicographic
//! comparison (reads both lengths, walks elements via `binop` on each pair).
//! Host-only (no LLVM); runtime values are covered by the `list_ordering` golden
//! on the LLVM CI matrix.

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

#[test]
fn list_lt_is_lexicographic_structural() {
    // `a < b` must read both lengths and walk elements, not pointer-compare the
    // two list headers.
    let m = lower_ok(
        "def main():\n\
        \x20   list[int64] a = [1, 2]\n\
        \x20   list[int64] b = [1, 3]\n\
        \x20   print(str(a < b))\n",
    );
    assert!(has(&m, "zap_list_len"), "list `<` must read lengths");
    assert!(
        has(&m, "zap_list_get_i64"),
        "list `<` must walk elements for lexicographic order"
    );
}

#[test]
fn list_all_order_ops_lower_structurally() {
    for op in ["<", "<=", ">", ">="] {
        let m = lower_ok(&format!(
            "def main():\n\
            \x20   list[int64] a = [1, 2]\n\
            \x20   list[int64] b = [1, 2, 3]\n\
            \x20   print(str(a {op} b))\n",
        ));
        assert!(
            has(&m, "zap_list_len") && has(&m, "zap_list_get_i64"),
            "list `{op}` must be lexicographic-structural"
        );
    }
}

#[test]
fn list_str_ordering_uses_str_cmp() {
    // Reference (`str`) elements order by contents (`zap_str_cmp`), not pointer.
    let m = lower_ok(
        "def main():\n\
        \x20   list[str] a = [\"a\", \"b\"]\n\
        \x20   list[str] b = [\"a\", \"c\"]\n\
        \x20   print(str(a < b))\n",
    );
    assert!(
        has(&m, "zap_str_cmp"),
        "str element ordering must use zap_str_cmp"
    );
}

#[test]
fn list_float_ordering_uses_fcmp() {
    // float elements compare through the element `<` (an `FCmp` in codegen); the
    // structural walk still reads lengths and the typed float getter.
    let m = lower_ok(
        "def main():\n\
        \x20   list[float64] a = [1.0, 2.0]\n\
        \x20   list[float64] b = [1.0, 3.0]\n\
        \x20   print(str(a < b))\n",
    );
    assert!(has(&m, "zap_list_len"));
    assert!(
        has(&m, "zap_list_get_f64"),
        "float element read-back must use zap_list_get_f64"
    );
}

#[test]
fn list_ordering_coexists_with_identity() {
    // `is` / `is not` stay a pointer identity test (no structural walk for them).
    let m = lower_ok(
        "def main():\n\
        \x20   list[int64] a = [1]\n\
        \x20   list[int64] b = [1]\n\
        \x20   print(str(a < b), str(a is not b))\n",
    );
    assert!(has(&m, "zap_list_len"), "the `<` is structural");
}
