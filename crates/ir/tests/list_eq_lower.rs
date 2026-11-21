//! List structural equality — lowering (ZIR) checks for #1239.
//!
//! Before the fix, `list == list` fell through to the generic numeric/reference
//! comparison path, which `ptrtoint`-compared the two list headers: an identity
//! test, so two separately allocated but equal lists reported unequal. These are
//! host-only lowering checks (no LLVM): they lower a program to ZIR and assert
//! that `==`/`!=` on lists dispatches to a *structural* element-wise compare
//! (reads both lengths via `zap_list_len` and walks elements via
//! `zap_list_get_*`), not a bare pointer compare. The end-to-end runtime *values*
//! are covered by the `list_structural_eq` golden on the LLVM CI matrix.

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

/// Count of emitted `Call`/`Invoke` to a symbol with the given prefix.
fn count_with_prefix(m: &Module, prefix: &str) -> usize {
    called_symbols(m)
        .iter()
        .filter(|s| s.starts_with(prefix))
        .count()
}

#[test]
fn list_int_eq_is_structural() {
    // `==` on two int lists must compare lengths and walk elements, not compare
    // the two list pointers.
    let m = lower_ok(
        "def main():\n\
        \x20   list[int64] a = [1, 2, 3]\n\
        \x20   list[int64] b = [1, 2, 3]\n\
        \x20   print(str(a == b))\n",
    );
    let syms = called_symbols(&m);
    assert!(
        syms.iter().any(|s| s == "zap_list_len"),
        "list `==` must read lengths (zap_list_len); got {syms:?}"
    );
    assert!(
        syms.iter().any(|s| s.starts_with("zap_list_get_")),
        "list `==` must read elements (zap_list_get_*); got {syms:?}"
    );
}

#[test]
fn list_float_eq_is_structural() {
    // The element type observed in #1239 (otel#9). Floats compare through the
    // element `==`, which the loop drives per element.
    let m = lower_ok(
        "def main():\n\
        \x20   list[float64] a = [0.0, 1.0]\n\
        \x20   list[float64] b = [0.0, 1.0]\n\
        \x20   print(str(a == b))\n",
    );
    assert!(
        called_symbols(&m).iter().any(|s| s == "zap_list_len"),
        "list[float64] `==` must be structural"
    );
    // Element read-back is the typed float entry point.
    assert!(
        called_symbols(&m).iter().any(|s| s == "zap_list_get_f64"),
        "float element read-back must use zap_list_get_f64"
    );
}

#[test]
fn list_str_eq_is_structural() {
    // Reference (`str`) elements: the element compare routes through `zap_str_eq`,
    // not a pointer identity test on each element.
    let m = lower_ok(
        "def main():\n\
        \x20   list[str] a = [\"x\", \"y\"]\n\
        \x20   list[str] b = [\"x\", \"y\"]\n\
        \x20   print(str(a == b))\n",
    );
    let syms = called_symbols(&m);
    assert!(
        syms.iter().any(|s| s == "zap_list_len"),
        "list[str] `==` must be structural"
    );
    assert!(
        syms.iter().any(|s| s == "zap_str_eq"),
        "str element equality must use contents (zap_str_eq); got {syms:?}"
    );
}

#[test]
fn list_ne_is_structural_too() {
    // `!=` reuses the same structural compare and negates the result.
    let m = lower_ok(
        "def main():\n\
        \x20   list[int64] a = [1, 2]\n\
        \x20   list[int64] b = [1, 3]\n\
        \x20   print(str(a != b))\n",
    );
    assert!(
        called_symbols(&m).iter().any(|s| s == "zap_list_len"),
        "list `!=` must be structural"
    );
}

#[test]
fn nested_list_eq_recurses() {
    // A `list[list[int64]]` element compare recurses into the same structural
    // path: the outer compare reads the outer lengths, and each element compare
    // reads the inner lengths — so `zap_list_len` is emitted for both levels
    // (at least two distinct structural compares are lowered).
    let m = lower_ok(
        "def main():\n\
        \x20   list[list[int64]] a = [[1, 2], [3]]\n\
        \x20   list[list[int64]] b = [[1, 2], [3]]\n\
        \x20   print(str(a == b))\n",
    );
    // Outer `==` + the nested element `==` each emit their own length reads.
    assert!(
        count_with_prefix(&m, "zap_list_len") >= 2,
        "nested list equality must recurse into element lists"
    );
}

#[test]
fn list_eq_is_distinct_from_identity() {
    // `==` (structural) and `is`/`is not` (identity) coexist on the same lists:
    // the structural compare still reads lengths/elements even when an identity
    // check is also present in the program.
    let m = lower_ok(
        "def main():\n\
        \x20   list[int64] a = [1, 2]\n\
        \x20   list[int64] b = [1, 2]\n\
        \x20   print(str(a == b))\n\
        \x20   print(str(a is not b))\n",
    );
    assert!(
        called_symbols(&m).iter().any(|s| s == "zap_list_len"),
        "structural `==` must still be lowered alongside an identity `is not`"
    );
}

#[test]
fn list_bool_eq_lowers_and_is_structural() {
    // list[bool] equality is independent of #1241 (which is iterate-then-append):
    // literal construction + structural `==` must lower cleanly. bool elements
    // share the i64 word, so read-back is the i64 entry point.
    let m = lower_ok(
        "def main():\n\
        \x20   list[bool] a = [True, False]\n\
        \x20   list[bool] b = [True, False]\n\
        \x20   print(str(a == b))\n",
    );
    assert!(
        called_symbols(&m).iter().any(|s| s == "zap_list_len"),
        "list[bool] `==` must be structural"
    );
}

#[test]
fn golden_program_lowers_without_error() {
    // The full `list_structural_eq` golden must lower cleanly on the host
    // (sema + lowering), so the only thing left for the LLVM CI matrix to add is
    // the runtime values: literal-RHS comparisons, empty lists, aliasing, and the
    // copy helpers across int64/float64/str/bool.
    let src = include_str!("../../../tests/golden/list_structural_eq.zap");
    let _ = lower_ok(src);
}
