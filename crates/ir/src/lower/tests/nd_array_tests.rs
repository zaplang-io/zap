//! N-D numeric arrays: shape + multi-axis indexing (#500, D-0164). These run
//! on the host (no LLVM): they exercise AST->ZIR lowering directly, so the
//! shape/rank typing and the row-fold indexing lowering are a walled garden.
//! Construction, full/partial indexing, `.shape`/`.ndim`/`.size`, assignment,
//! element-wise ops, and reductions must lower cleanly; ragged literals, rank
//! mismatches, over-indexing, N-D slices, and whole-row stores must be
//! rejected with a lowering diagnostic.
use crate::lower::lower_module;
use zapc_ir_core::Inst;

fn lower_errs(src: &str) -> Vec<String> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty(), "parse errors in test source: {perrs:?}");
    assert!(lerrs.is_empty(), "lex errors in test source: {lerrs:?}");
    let (_m, errs) = lower_module(&module);
    errs.into_iter().map(|e| format!("{e:?}")).collect()
}

fn lowers_ok(src: &str) {
    let errs = lower_errs(src);
    assert!(
        errs.is_empty(),
        "unexpected lowering errors for\n{src}\n{errs:?}"
    );
}

fn lower_fails(src: &str) {
    let errs = lower_errs(src);
    assert!(!errs.is_empty(), "expected a lowering error for\n{src}");
}

/// Every runtime-call function name emitted anywhere in the lowered program
/// (both `Call` and the unwinding `Invoke`), so a test can prove which
/// runtime entry points an N-D operation lowers to.
fn call_funcs(src: &str) -> Vec<String> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty(), "parse errors in test source: {perrs:?}");
    assert!(lerrs.is_empty(), "lex errors in test source: {lerrs:?}");
    let (m, errs) = lower_module(&module);
    assert!(errs.is_empty(), "unexpected lowering errors: {errs:?}");
    m.funcs
        .iter()
        .flat_map(|f| f.blocks.iter())
        .flat_map(|b| b.insts.iter())
        .filter_map(|i| match i {
            Inst::Call { func, .. } => Some(func.clone()),
            Inst::Invoke { func, .. } => Some(func.clone()),
            _ => None,
        })
        .collect()
}

fn count(src: &str, func: &str) -> usize {
    call_funcs(src).iter().filter(|f| *f == func).count()
}

// --- construction ---

#[test]
fn two_d_construction_lowers() {
    // A rectangular nested literal builds a rank-2 array via new_nd + two
    // set_dim calls (one per axis) and four element stores.
    let src = "def main():\n    auto m = array[f64]([[1.0, 2.0], [3.0, 4.0]])\n    print(len(m))\n";
    lowers_ok(src);
    assert_eq!(count(src, "zap_array_new_nd"), 1);
    assert_eq!(count(src, "zap_array_set_dim"), 2);
    assert_eq!(count(src, "zap_array_set_f64"), 4);
}

#[test]
fn three_d_construction_and_index_lowers() {
    lowers_ok(
        "def main():\n    auto c = array[i64]([[[1, 2], [3, 4]], [[5, 6], [7, 8]]])\n    \
             print(c[1, 0, 1])\n",
    );
}

#[test]
fn one_d_construction_still_uses_plain_new() {
    // A flat literal keeps the historical 1-D path (new, not new_nd).
    let src = "def main():\n    auto a = array[f64]([1.0, 2.0, 3.0])\n    print(a[0])\n";
    lowers_ok(src);
    assert_eq!(count(src, "zap_array_new"), 1);
    assert_eq!(count(src, "zap_array_new_nd"), 0);
    assert_eq!(count(src, "zap_array_set_dim"), 0);
}

#[test]
fn empty_and_zero_axis_literals_lower() {
    lowers_ok("def main():\n    auto a = array[f64]([])\n    print(len(a))\n");
    lowers_ok("def main():\n    auto m = array[i64]([[], []])\n    print(len(m))\n");
}

// --- indexing ---

#[test]
fn full_index_takes_the_flat_fast_path() {
    // #894: a FULL multi-axis integer index `m[i, j]` must NOT allocate an
    // intermediate rank-1 row view. It lowers to an inline flat offset
    // (one `zap_array_ckidx` per axis + one trailing-axis `zap_array_dim`
    // for the Horner multiply) feeding a single scalar `zap_array_get_i64`.
    let src = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    print(m[1, 0])\n";
    lowers_ok(src);
    assert_eq!(
        count(src, "zap_array_row"),
        0,
        "no row-view allocation on a full index"
    );
    assert_eq!(count(src, "zap_array_get_i64"), 1);
    assert_eq!(
        count(src, "zap_array_ckidx"),
        2,
        "one per-axis bounds check"
    );
    // Only the trailing axis (axis 1) contributes a Horner multiplier.
    assert_eq!(count(src, "zap_array_dim"), 1);
}

#[test]
fn full_index_rank3_fast_path() {
    // Rank 3: still no row fold; three axis checks and two Horner dims.
    let src = "def main():\n    \
                   auto m = array[f64]([[[1.0, 2.0], [3.0, 4.0]], [[5.0, 6.0], [7.0, 8.0]]])\n    \
                   print(m[1, 0, 1])\n";
    lowers_ok(src);
    assert_eq!(count(src, "zap_array_row"), 0);
    assert_eq!(count(src, "zap_array_get_f64"), 1);
    assert_eq!(count(src, "zap_array_ckidx"), 3);
    assert_eq!(count(src, "zap_array_dim"), 2);
}

#[test]
fn full_index_store_takes_the_flat_fast_path() {
    // #894: `m[i, j] = v` and `m[i, j] += v` write through the same inline
    // flat offset — no row-view allocation on the store path either.
    let store = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    m[0, 1] = 9\n";
    lowers_ok(store);
    assert_eq!(count(store, "zap_array_row"), 0);
    assert_eq!(count(store, "zap_array_set_i64"), 5); // 4 construction fills + 1 store
    assert_eq!(count(store, "zap_array_ckidx"), 2);

    let aug = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    m[1, 1] += 5\n";
    lowers_ok(aug);
    assert_eq!(count(aug, "zap_array_row"), 0);
    // read-modify-write: one get + one set on the same flat slot (plus the
    // four construction fills), one ckidx pair reused for the slot.
    assert_eq!(count(aug, "zap_array_get_i64"), 1);
    assert_eq!(count(aug, "zap_array_set_i64"), 5);
    assert_eq!(count(aug, "zap_array_ckidx"), 2);
}

#[test]
fn chained_full_index_fuses_like_comma_index() {
    // #894 part 2: `m[i][j]` on a rank-2 array fuses to the SAME flat scalar
    // access as `m[i, j]` — the dead intermediate row view is not allocated.
    let src = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
                   auto i = 1\n    auto j = 0\n    print(m[i][j])\n";
    lowers_ok(src);
    assert_eq!(
        count(src, "zap_array_row"),
        0,
        "chained full index must not allocate a row view"
    );
    assert_eq!(count(src, "zap_array_get_i64"), 1);
    assert_eq!(count(src, "zap_array_ckidx"), 2);
    assert_eq!(count(src, "zap_array_dim"), 1);
}

#[test]
fn chained_full_index_rank3_fuses() {
    // Rank-3 `m[i][j][k]` with all axes indexed also fuses to one flat get.
    let src = "def main():\n    \
                   auto m = array[f64]([[[1.0, 2.0], [3.0, 4.0]], [[5.0, 6.0], [7.0, 8.0]]])\n    \
                   print(m[1][0][1])\n";
    lowers_ok(src);
    assert_eq!(count(src, "zap_array_row"), 0);
    assert_eq!(count(src, "zap_array_get_f64"), 1);
    assert_eq!(count(src, "zap_array_ckidx"), 3);
}

#[test]
fn chained_partial_index_still_views() {
    // A PARTIAL chain (fewer indices than axes) is not a full access, so it
    // keeps the per-step row views — `m[i][j]` on a rank-3 array yields a
    // rank-1 view via two row folds and is not fused.
    let src = "def main():\n    \
                   auto m = array[i64]([[[1, 2], [3, 4]], [[5, 6], [7, 8]]])\n    \
                   auto r = m[0][1]\n    print(r[0], sum(r))\n";
    lowers_ok(src);
    assert_eq!(
        count(src, "zap_array_row"),
        2,
        "partial chain keeps row folds"
    );
    assert_eq!(count(src, "zap_array_ckidx"), 0);
}

#[test]
fn bound_row_view_is_not_fused() {
    // A row view bound to a name is a first-class value: `row = m[i]` still
    // allocates it, and the later `row[j]` is a plain 1-D get (no chain).
    let src = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
                   auto row = m[1]\n    print(row[0], row[1])\n";
    lowers_ok(src);
    assert_eq!(count(src, "zap_array_row"), 1, "bound view is materialized");
    assert_eq!(count(src, "zap_array_ckidx"), 0);
}

#[test]
fn impure_chain_index_is_not_fused() {
    // A non-pure link index (`i + 1`) is not lowered speculatively, so the
    // chain is not fused — it falls back to the per-step row view (unchanged
    // behavior), still correct.
    let src = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
                   auto i = 0\n    auto j = 1\n    print(m[i + 1][j])\n";
    lowers_ok(src);
    assert_eq!(
        count(src, "zap_array_row"),
        1,
        "impure chain keeps the row fold"
    );
    assert_eq!(count(src, "zap_array_ckidx"), 0);
}

#[test]
fn fancy_index_chain_is_not_fused() {
    // `m[idx][j]` with an integer index ARRAY is advanced indexing (#510),
    // not a plain integer chain: it must not be fused (the gather copy is a
    // real value), so no per-axis ckidx is emitted.
    let src = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
                   auto idx = array[i64]([0, 1])\n    print(m[idx][0])\n";
    lowers_ok(src);
    assert_eq!(
        count(src, "zap_array_ckidx"),
        0,
        "fancy chain is not a flat index"
    );
}

#[test]
fn partial_index_returns_a_row_view() {
    // m[i] on a rank-2 array -> a rank-1 view (one row fold, no scalar get).
    let src = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
                   auto row = m[0]\n    print(len(row), row[1])\n";
    lowers_ok(src);
    // one fold for m[0]; the row[1] get is a plain 1-D get on the view.
    assert_eq!(count(src, "zap_array_row"), 1);
}

#[test]
fn one_d_index_unchanged() {
    // Rank-1 a[i] is still a direct get with no row fold.
    let src = "def main():\n    auto a = array[i64]([1, 2, 3])\n    print(a[2])\n";
    lowers_ok(src);
    assert_eq!(count(src, "zap_array_row"), 0);
    assert_eq!(count(src, "zap_array_get_i64"), 1);
}

// --- attributes ---

#[test]
fn shape_ndim_size_lower() {
    lowers_ok(
        "def main():\n    auto m = array[f64]([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]])\n    \
             auto s = m.shape\n    print(s[0], s[1], m.ndim, m.size)\n",
    );
    // .shape reads each axis via zap_array_dim; .size via zap_array_count.
    let src = "def main():\n    auto m = array[f64]([[1.0, 2.0], [3.0, 4.0]])\n    \
                   print(m.shape[0], m.shape[1], m.size)\n";
    assert!(count(src, "zap_array_dim") >= 2);
    assert_eq!(count(src, "zap_array_count"), 1);
}

#[test]
fn unknown_array_attribute_is_rejected() {
    lower_fails("def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    print(m.strides)\n");
}

// --- assignment / augmented ---

#[test]
fn full_index_assignment_lowers() {
    lowers_ok(
        "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    m[0, 1] = 9\n    \
             print(m[0, 1])\n",
    );
}

#[test]
fn augmented_full_index_lowers() {
    lowers_ok(
        "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    m[1, 1] += 5\n    \
             print(m[1, 1])\n",
    );
}

// --- element-wise / reductions ---

#[test]
fn same_rank_elementwise_and_scalar_broadcast_lower() {
    lowers_ok(
        "def main():\n    auto a = array[i64]([[1, 2], [3, 4]])\n    \
             auto b = array[i64]([[5, 6], [7, 8]])\n    print(sum(a + b), sum(a * 2))\n",
    );
}

#[test]
fn full_reduction_over_nd_lowers() {
    lowers_ok(
        "def main():\n    auto m = array[f64]([[1.0, 2.0], [3.0, 4.0]])\n    print(sum(m))\n",
    );
}

// --- rejections ---

#[test]
fn ragged_literals_are_rejected() {
    // Short row.
    lower_fails("def main():\n    auto m = array[i64]([[1, 2], [3]])\n");
    // Inconsistent nesting depth (scalar beside a list).
    lower_fails("def main():\n    auto m = array[i64]([1, [2, 3]])\n");
    // Nested deeper than the first row.
    lower_fails("def main():\n    auto m = array[i64]([[1, 2], [[3], [4]]])\n");
}

#[test]
fn too_many_indices_is_rejected() {
    lower_fails("def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    print(m[0, 1, 0])\n");
}

#[test]
fn cross_rank_and_cross_shape_elementwise_broadcast_lowers() {
    // Broadcasting (#501): cross-rank (2-D + 1-D) and cross-shape element-wise
    // ops now lower; compatibility is a runtime check (zap_array_broadcast_check),
    // not a lowering error. Even an incompatible pair lowers and fails at runtime.
    let src = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
                   auto v = array[i64]([10, 20])\n    print(sum(m + v))\n";
    lowers_ok(src);
    assert_eq!(count(src, "zap_array_broadcast_check"), 1);
    // A size-1 column stretched against a full matrix also lowers.
    lowers_ok(
        "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
             auto c = array[i64]([[10], [20]])\n    print(sum(m + c))\n",
    );
}

#[test]
fn nd_axis0_slice_lowers_as_a_view() {
    // N-D axis-0 slicing is a rank-preserving strided view since #502.
    let src = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
                   auto r = m[0:1]\n    print(sum(r))\n";
    lowers_ok(src);
    assert_eq!(count(src, "zap_array_slice_axis"), 1);
}

#[test]
fn whole_row_store_is_rejected() {
    lower_fails(
        "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
             m[0] = array[i64]([9, 9])\n",
    );
}
