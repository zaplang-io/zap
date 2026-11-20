//! Element-wise comparison -> `array[bool]`, mask combination `& | ^ ~`, and
//! the ambiguous-truth rule (#510, D-0175). Host-only (AST->ZIR): asserts the
//! comparison/logical kernels each form lowers to, that array⊗array compares
//! and mask combiners are raising (Invoke — they run `zap_array_broadcast_check`
//! and raise ValueError on incompatible shapes), that a scalar on either side
//! routes through `cmp_scalar`, and that truthiness / `and`/`or`/`not` /
//! chained comparison on an array raise NumPy's "ambiguous" ValueError. A
//! walled garden around the mask-source plumbing; the kernels themselves run
//! in the CI goldens.
use crate::lower::lower_module;
use zapc_ir_core::Inst;

fn all_calls(src: &str) -> Vec<String> {
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
    all_calls(src).iter().any(|f| f == func)
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

const A1: &str = "def main():\n    auto a = array[i64]([1, 2, 3, 4])\n    ";
const AB: &str =
    "def main():\n    auto a = array[i64]([1, 2, 3])\n    auto b = array[i64]([3, 2, 1])\n    ";

#[test]
fn scalar_comparison_makes_a_bool_mask() {
    // `a > 3` -> cmp_scalar (not raising: no broadcast, so a plain Call).
    let s = format!("{A1}auto m = a > 3\n    print(m[0])\n");
    assert!(has(&s, "zap_array_cmp_scalar"));
    assert!(!has(&s, "zap_array_cmp"));
}

#[test]
fn all_six_relational_ops_lower_to_a_compare() {
    for op in ["==", "!=", "<", "<=", ">", ">="] {
        let s = format!("{A1}auto m = a {op} 2\n    print(m[0])\n");
        assert!(
            has(&s, "zap_array_cmp_scalar"),
            "op {op} did not lower to cmp_scalar"
        );
    }
}

#[test]
fn scalar_on_left_also_lowers_to_compare() {
    // `3 < a` must preserve operand order (scalar_lhs); still cmp_scalar.
    let s = format!("{A1}auto m = 3 < a\n    print(m[0])\n");
    assert!(has(&s, "zap_array_cmp_scalar"));
}

#[test]
fn float_scalar_comparison_lowers_to_compare() {
    let s = "def main():\n    auto a = array[f64]([1.0, 2.0])\n    auto m = a >= 1.5\n    print(m[0])\n";
    assert!(has(s, "zap_array_cmp_scalar"));
}

#[test]
fn array_array_comparison_is_a_raising_compare() {
    // `a == b` broadcasts -> the kernel runs broadcast_check and can raise,
    // so it is an Invoke, and it is `zap_array_cmp` (not the scalar form).
    let s = format!("{AB}auto m = a == b\n    print(m[0])\n");
    assert!(is_invoked(&s, "zap_array_cmp"));
    assert!(!has(&s, "zap_array_cmp_scalar"));
}

#[test]
fn mask_combiners_lower_to_logical() {
    for (op, _) in [("&", 0), ("|", 1), ("^", 2)] {
        let s = format!("{A1}auto m = (a > 1) {op} (a < 4)\n    print(m[0])\n");
        assert!(
            is_invoked(&s, "zap_array_logical"),
            "op {op} did not lower to logical"
        );
    }
}

#[test]
fn invert_lowers_to_lognot() {
    let s = format!("{A1}auto m = ~(a > 2)\n    print(m[0])\n");
    assert!(has(&s, "zap_array_lognot"));
}

#[test]
fn bitwise_on_non_bool_arrays_is_rejected() {
    // `&` on integer arrays is not mask combination (integer-array bitwise is
    // a separate element-wise op, out of #510's scope).
    lower_fails(&format!("{AB}auto m = a & b\n    print(m[0])\n"));
}

#[test]
fn invert_of_a_non_bool_array_is_rejected() {
    lower_fails(&format!("{A1}auto m = ~a\n    print(m[0])\n"));
}

#[test]
fn array_in_if_condition_raises_ambiguous() {
    let s = format!("{A1}if a > 3:\n        print(1)\n");
    assert!(is_invoked(&s, "zap_array_truth_ambiguous"));
}

#[test]
fn chained_comparison_on_an_array_raises_ambiguous() {
    // `1 < a < 4` desugars through `and`, whose truthy on the first (array)
    // link raises — exactly NumPy's behavior.
    let s = format!("{A1}auto m = 1 < a < 4\n    print(m)\n");
    assert!(is_invoked(&s, "zap_array_truth_ambiguous"));
}

#[test]
fn logical_and_or_not_on_arrays_raise_ambiguous() {
    let sand = format!("{AB}auto m = (a == b) and (a != b)\n    print(m)\n");
    assert!(is_invoked(&sand, "zap_array_truth_ambiguous"));
    let snot = format!("{A1}auto m = not (a > 1)\n    print(m)\n");
    assert!(is_invoked(&snot, "zap_array_truth_ambiguous"));
}

// --- Fork 2: boolean-mask indexing (#510) ---

#[test]
fn boolean_mask_index_selects_via_mask_select() {
    // `a[a > 2]` -> compare (scalar) then a raising mask-select gather.
    let s = format!("{A1}auto r = a[a > 2]\n    print(r[0])\n");
    assert!(has(&s, "zap_array_cmp_scalar"));
    assert!(is_invoked(&s, "zap_array_mask_select"));
}

#[test]
fn boolean_mask_from_a_variable_selects() {
    let s = format!("{A1}auto m = a > 2\n    auto r = a[m]\n    print(r[0])\n");
    assert!(is_invoked(&s, "zap_array_mask_select"));
}

#[test]
fn full_shape_2d_mask_selects() {
    let s = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
                 auto k = m > 2\n    auto r = m[k]\n    print(r[0])\n";
    assert!(is_invoked(s, "zap_array_mask_select"));
}

#[test]
fn masked_scalar_assignment_clips() {
    // `a[a < 0] = 0` -> mask-assign-scalar (raising on shape mismatch).
    let s = "def main():\n    auto a = array[i64]([-1, 2, -3, 4])\n    \
                 a[a < 0] = 0\n    print(a[0])\n";
    assert!(is_invoked(s, "zap_array_mask_assign_scalar"));
    assert!(!has(s, "zap_array_mask_assign_array"));
}

#[test]
fn masked_array_assignment_scatters() {
    let s = "def main():\n    auto a = array[i64]([1, 2, 3, 4])\n    \
                 auto vals = array[i64]([7, 8])\n    a[a > 2] = vals\n    print(a[0])\n";
    assert!(is_invoked(s, "zap_array_mask_assign_array"));
}

#[test]
fn masked_float_assignment_lowers() {
    let s = "def main():\n    auto a = array[f64]([1.0, -2.0, 3.0])\n    \
                 a[a < 0.0] = 9.0\n    print(a[1])\n";
    assert!(is_invoked(s, "zap_array_mask_assign_scalar"));
}

// --- Fork 4a: fancy integer-array gather / scatter (#510) ---

#[test]
fn integer_index_array_gathers_via_take() {
    // `a[idx]` with an integer index array -> a raising gather (bounds check).
    let s =
        format!("{A1}auto idx = array[i64]([0, 2, 2, 3])\n    auto r = a[idx]\n    print(r[0])\n");
    assert!(is_invoked(&s, "zap_array_take"));
    assert!(!has(&s, "zap_array_mask_select"));
}

#[test]
fn nd_array_gather_on_axis0() {
    let s = "def main():\n    auto m = array[i64]([[1, 2], [3, 4], [5, 6]])\n    \
                 auto idx = array[i64]([2, 0])\n    auto r = m[idx]\n    print(r[0, 0])\n";
    assert!(is_invoked(s, "zap_array_take"));
}

#[test]
fn fancy_scalar_scatter_assignment() {
    let s = format!("{A1}auto idx = array[i64]([0, 2])\n    a[idx] = 9\n    print(a[0])\n");
    assert!(is_invoked(&s, "zap_array_scatter_scalar"));
}

#[test]
fn fancy_array_scatter_assignment() {
    let s = format!(
        "{A1}auto idx = array[i64]([0, 2])\n    auto v = array[i64]([7, 8])\n    \
             a[idx] = v\n    print(a[0])\n"
    );
    assert!(is_invoked(&s, "zap_array_scatter_array"));
}

// --- Fork 5: where(cond, x, y) + nonzero (#510) ---

#[test]
fn where_with_scalar_fill_lowers_to_select() {
    // `where(a > 0, a, 0)` -> a raising select; the int 0 is wrapped as a
    // 1-element array.
    let s = format!("{A1}auto r = where(a > 0, a, 0)\n    print(r[0])\n");
    assert!(is_invoked(&s, "zap_array_where"));
    assert!(has(&s, "zap_array_from_scalar_i"));
}

#[test]
fn where_promotes_with_a_float_scalar() {
    let s = format!("{A1}auto r = where(a > 0, a, 2.5)\n    print(r[0])\n");
    assert!(is_invoked(&s, "zap_array_where"));
    assert!(has(&s, "zap_array_from_scalar_f"));
}

#[test]
fn where_with_two_arrays_takes_neither_scalar_wrapper() {
    let s = format!("{AB}auto r = where(a > b, a, b)\n    print(r[0])\n");
    assert!(is_invoked(&s, "zap_array_where"));
    assert!(!has(&s, "zap_array_from_scalar_i"));
    assert!(!has(&s, "zap_array_from_scalar_f"));
}

#[test]
fn one_arg_where_is_nonzero() {
    // `where(a > 2)` -> a tuple of one index array (1-D cond).
    let s = format!("{A1}auto t = where(a > 2)\n    print(t[0][0])\n");
    assert!(has(&s, "zap_array_nonzero_axis"));
    assert!(!has(&s, "zap_array_where"));
}

#[test]
fn two_arg_where_is_rejected() {
    lower_fails(&format!("{A1}auto r = where(a > 0, a)\n    print(r[0])\n"));
}

// --- Fork 4b: general advanced / mixed indexing (#510) ---

const M2: &str = "def main():\n    auto m = array[i64]([[1, 2, 3], [4, 5, 6], [7, 8, 9]])\n    \
                      auto i = array[i64]([0, 2])\n    auto j = array[i64]([1, 0])\n    ";

#[test]
fn two_integer_index_arrays_use_the_fancy_engine() {
    // `m[i, j]` (two advanced indices) -> the general fancy engine, raising.
    let s = format!("{M2}auto r = m[i, j]\n    print(r[0])\n");
    assert!(is_invoked(&s, "zap_array_fancy"));
}

#[test]
fn mixed_slice_and_index_array_uses_fancy() {
    // `m[1:3, j]` mixes a basic slice with an advanced index.
    let s = format!("{M2}auto r = m[1:3, j]\n    print(r[0, 0])\n");
    assert!(is_invoked(&s, "zap_array_fancy"));
}

#[test]
fn index_array_with_full_axis_uses_fancy() {
    let s = format!("{M2}auto r = m[i, :]\n    print(r[0, 0])\n");
    assert!(is_invoked(&s, "zap_array_fancy"));
}

#[test]
fn index_array_with_scalar_axis_uses_fancy() {
    let s = format!("{M2}auto r = m[i, 2]\n    print(r[0])\n");
    assert!(is_invoked(&s, "zap_array_fancy"));
}

#[test]
fn pure_slice_tuple_still_a_strided_view_not_fancy() {
    // No array part -> the #502 strided-view path, never the fancy engine.
    let s = "def main():\n    auto m = array[i64]([[1, 2, 3], [4, 5, 6]])\n    \
                 auto r = m[0:2, 1]\n    print(r[0])\n";
    assert!(has(s, "zap_array_slice_axis"));
    assert!(!has(s, "zap_array_fancy"));
}

#[test]
fn plain_nd_integer_index_is_not_fancy() {
    // `m[1, 2]` (all integer, full index) stays the scalar-get path.
    let s = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
                 print(m[1, 0])\n";
    assert!(!has(s, "zap_array_fancy"));
    assert!(has(s, "zap_array_get_i64"));
}
