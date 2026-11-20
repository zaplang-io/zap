//! Shape ops (#511, D-0174): reshape/transpose/flatten (methods) + `.T`
//! (attribute), and concatenate/stack (builtins over a list literal).
//! Host-only (AST->ZIR): asserts which runtime entry point each form lowers
//! to, that the mutating/raising forms are Invokes, and that bad surfaces
//! are rejected. Runtime kernels run in the CI goldens.
use crate::lower::lower_module;
use zapc_ir_core::Inst;

fn call_funcs(src: &str) -> Vec<String> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(
        perrs.is_empty() && lerrs.is_empty(),
        "parse/lex: {perrs:?} {lerrs:?}"
    );
    let (m, errs) = lower_module(&module);
    assert!(
        errs.is_empty(),
        "unexpected lowering errors for\n{src}\n{errs:?}"
    );
    m.funcs
        .iter()
        .flat_map(|f| f.blocks.iter())
        .flat_map(|b| b.insts.iter())
        .filter_map(|i| match i {
            Inst::Call { func, .. } | Inst::Invoke { func, .. } => Some(func.clone()),
            _ => None,
        })
        .collect()
}
fn has(src: &str, func: &str) -> bool {
    call_funcs(src).iter().any(|f| f == func)
}
fn is_invoked(src: &str, func: &str) -> bool {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty() && lerrs.is_empty());
    let (m, errs) = lower_module(&module);
    assert!(errs.is_empty(), "unexpected lowering errors: {errs:?}");
    m.funcs
        .iter()
        .flat_map(|f| f.blocks.iter())
        .flat_map(|b| b.insts.iter())
        .any(|i| matches!(i, Inst::Invoke { func: f, .. } if f == func))
}
fn lower_fails(src: &str) {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty() && lerrs.is_empty());
    let (_m, errs) = lower_module(&module);
    assert!(!errs.is_empty(), "expected a lowering error for\n{src}");
}
fn lowers_ok(src: &str) {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(
        perrs.is_empty() && lerrs.is_empty(),
        "parse/lex: {perrs:?} {lerrs:?}"
    );
    let (_m, errs) = lower_module(&module);
    assert!(
        errs.is_empty(),
        "unexpected lowering errors for\n{src}\n{errs:?}"
    );
}

#[test]
fn reshape_builds_dims_buffer_and_is_raising() {
    let src = "def main():\n    auto a = array[i64]([1, 2, 3, 4, 5, 6])\n    \
                   auto m = a.reshape(2, 3)\n    print(m.ndim, sum(m))\n";
    assert!(has(src, "zap_i64buf"));
    assert_eq!(
        call_funcs(src)
            .iter()
            .filter(|f| *f == "zap_i64set")
            .count(),
        2
    );
    assert!(is_invoked(src, "zap_array_reshape")); // size mismatch -> ValueError
}

#[test]
fn reshape_allows_one_inferred_minus_one() {
    lowers_ok(
        "def main():\n    auto a = array[i64]([1, 2, 3, 4])\n    \
                   auto m = a.reshape(2, -1)\n    print(sum(m))\n",
    );
}

#[test]
fn transpose_method_and_t_attr_lower_to_a_view() {
    let m = "def main():\n    auto a = array[i64]([[1, 2, 3], [4, 5, 6]])\n    ";
    assert!(has(
        &format!("{m}print(sum(a.transpose()))\n"),
        "zap_array_transpose"
    ));
    assert!(has(&format!("{m}print(sum(a.T))\n"), "zap_array_transpose"));
    // transpose is a pure view: not a raising call.
    assert!(!is_invoked(
        &format!("{m}print(sum(a.T))\n"),
        "zap_array_transpose"
    ));
}

#[test]
fn flatten_lowers_to_a_copy() {
    let src = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
                   auto f = m.flatten()\n    print(f.ndim, sum(f))\n";
    assert!(has(src, "zap_array_flatten"));
}

#[test]
fn concatenate_and_stack_over_a_list_literal() {
    let pre = "def main():\n    auto a = array[i64]([[1, 2], [3, 4]])\n    \
                   auto b = array[i64]([[5, 6], [7, 8]])\n    ";
    assert!(is_invoked(
        &format!("{pre}print(sum(concatenate([a, b])))\n"),
        "zap_array_concat"
    ));
    assert!(is_invoked(
        &format!("{pre}print(sum(concatenate([a, b], axis=1)))\n"),
        "zap_array_concat"
    ));
    assert!(is_invoked(
        &format!("{pre}print(sum(stack([a, b])))\n"),
        "zap_array_stack"
    ));
    // each array element is pushed into a fresh pointer list
    assert_eq!(
        call_funcs(&format!("{pre}print(sum(concatenate([a, b])))\n"))
            .iter()
            .filter(|f| *f == "zap_list_push_ptr")
            .count(),
        2
    );
}

// --- rejections ---

#[test]
fn unknown_array_method_is_rejected() {
    lower_fails("def main():\n    auto a = array[i64]([1, 2])\n    print(a.ravel())\n");
}
#[test]
fn transpose_with_args_is_rejected() {
    lower_fails(
        "def main():\n    auto a = array[i64]([[1, 2], [3, 4]])\n    auto b = a.transpose(1, 0)\n",
    );
}
#[test]
fn reshape_with_no_dims_is_rejected() {
    lower_fails("def main():\n    auto a = array[i64]([1, 2])\n    auto b = a.reshape()\n");
}
#[test]
fn concatenate_needs_a_list_literal() {
    lower_fails("def main():\n    auto a = array[i64]([1, 2])\n    print(sum(concatenate(a)))\n");
}
#[test]
fn unknown_array_attribute_still_rejected() {
    lower_fails("def main():\n    auto a = array[i64]([1, 2])\n    print(a.strides)\n");
}
