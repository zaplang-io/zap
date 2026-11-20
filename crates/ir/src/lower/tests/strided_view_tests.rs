//! Strided slice views `a[::2]`, `a[::-1]`, N-D axis slices `m[lo:hi]`, and
//! mixed int/slice indices `m[:, j]` (#502, D-0171). Host-only (AST->ZIR):
//! asserts which view primitive each form lowers to — a contiguous step-1
//! 1-D slice stays `zap_array_slice`; a stepped/reversed/N-D slice becomes
//! `zap_array_slice_axis`; a sliced axis inside a multi-axis index folds the
//! integer axes with `zap_array_index_axis` and the sliced axes with
//! `zap_array_slice_axis`. A walled garden around the fold, independent of
//! the runtime stride kernels (which run in the CI goldens).
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

fn count(src: &str, func: &str) -> usize {
    call_funcs(src).iter().filter(|f| *f == func).count()
}

// --- 1-D stepped / reversed ---

#[test]
fn contiguous_1d_slice_stays_the_fast_path() {
    // A step-1 1-D slice is still the contiguous `zap_array_slice` (NULL
    // strides), not the strided builder — no regression.
    let src = "def main():\n    auto a = array[i64]([1, 2, 3, 4])\n    \
                   auto v = a[1:3]\n    print(sum(v))\n";
    assert_eq!(count(src, "zap_array_slice"), 1);
    assert_eq!(count(src, "zap_array_slice_axis"), 0);
}

#[test]
fn stepped_1d_slice_is_a_strided_view() {
    let src = "def main():\n    auto a = array[i64]([1, 2, 3, 4, 5, 6])\n    \
                   auto v = a[::2]\n    print(sum(v))\n";
    assert_eq!(count(src, "zap_array_slice_axis"), 1);
    assert_eq!(count(src, "zap_array_slice"), 0);
}

#[test]
fn reversed_1d_slice_is_a_strided_view() {
    let src = "def main():\n    auto a = array[i64]([1, 2, 3])\n    \
                   auto v = a[::-1]\n    print(sum(v))\n";
    assert_eq!(count(src, "zap_array_slice_axis"), 1);
}

// --- N-D axis-0 slice ---

#[test]
fn nd_axis0_slice_keeps_rank() {
    let src = "def main():\n    auto m = array[i64]([[1, 2], [3, 4], [5, 6]])\n    \
                   auto r = m[1:3]\n    print(r.ndim, sum(r))\n";
    assert_eq!(count(src, "zap_array_slice_axis"), 1);
}

// --- mixed int/slice ---

#[test]
fn column_view_folds_slice_then_index() {
    // m[:, 1] -> slice axis 0 (keep), then integer-index the surviving axis.
    let src = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
                   auto col = m[:, 1]\n    print(col.ndim, sum(col))\n";
    assert_eq!(count(src, "zap_array_slice_axis"), 1);
    assert_eq!(count(src, "zap_array_index_axis"), 1);
}

#[test]
fn row_then_stepped_slice_mixes_fold() {
    // m[1, ::2] -> integer-index axis 0 (drop), then stepped-slice the row.
    let src = "def main():\n    auto m = array[i64]([[1, 2, 3, 4], [5, 6, 7, 8]])\n    \
                   auto r = m[1, ::2]\n    print(sum(r))\n";
    assert_eq!(count(src, "zap_array_index_axis"), 1);
    assert_eq!(count(src, "zap_array_slice_axis"), 1);
}

#[test]
fn write_through_a_strided_view_lowers() {
    // A write through a stepped view still lowers to a plain set (it aliases
    // the parent via the view's stride at runtime).
    let src = "def main():\n    auto a = array[i64]([1, 2, 3, 4])\n    \
                   auto v = a[::2]\n    v[0] = 9\n    print(sum(a))\n";
    assert_eq!(count(src, "zap_array_slice_axis"), 1);
    // The construction fills 4 elements, then the write-through is a 5th set.
    assert_eq!(count(src, "zap_array_set_i64"), 5);
}
