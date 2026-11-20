//! Array true division `/` and non-commutative scalar-on-left (#506, D-0169).
//! Host-only (AST->ZIR): every element-wise op lowers to a single
//! `Inst::ArrayBinOp`, so these assert the op, the promoted result dtype, and
//! the `scalar`/`scalar_lhs` flags directly — a walled garden around the
//! operand ordering and the NumPy true-division dtype rule, independent of
//! codegen (the SIMD kernel + inf/nan behaviour run in the CI goldens).
use crate::lower::lower_module;
use zapc_ir_core::{BinOp, Dtype, Inst};

struct BinInfo {
    op: BinOp,
    out: Dtype,
    scalar: bool,
    scalar_lhs: bool,
}

fn binops(src: &str) -> Vec<BinInfo> {
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
            Inst::ArrayBinOp {
                op,
                out,
                scalar,
                scalar_lhs,
                ..
            } => Some(BinInfo {
                op: *op,
                out: *out,
                scalar: *scalar,
                scalar_lhs: *scalar_lhs,
            }),
            _ => None,
        })
        .collect()
}

/// The single `ArrayBinOp` a one-expression program lowers to.
fn one(src: &str) -> BinInfo {
    let v = binops(src);
    assert_eq!(v.len(), 1, "expected exactly one ArrayBinOp for\n{src}");
    v.into_iter().next().unwrap()
}

fn has_bin_div(src: &str) -> bool {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty() && lerrs.is_empty());
    let (m, errs) = lower_module(&module);
    assert!(errs.is_empty(), "unexpected lowering errors: {errs:?}");
    m.funcs
        .iter()
        .flat_map(|f| f.blocks.iter())
        .flat_map(|b| b.insts.iter())
        .any(|i| matches!(i, Inst::Bin { op: BinOp::Div, .. }))
}

fn broadcast_checks(src: &str) -> usize {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty() && lerrs.is_empty());
    let (m, errs) = lower_module(&module);
    assert!(errs.is_empty(), "unexpected lowering errors: {errs:?}");
    m.funcs
        .iter()
        .flat_map(|f| f.blocks.iter())
        .flat_map(|b| b.insts.iter())
        .filter(|i| match i {
            Inst::Call { func, .. } | Inst::Invoke { func, .. } => {
                func == "zap_array_broadcast_check"
            }
            _ => false,
        })
        .count()
}

fn lower_fails(src: &str) {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty() && lerrs.is_empty());
    let (_m, errs) = lower_module(&module);
    assert!(!errs.is_empty(), "expected a lowering error for\n{src}");
}

// --- true division: array / array ---

#[test]
fn int_array_div_lifts_to_f64() {
    // NumPy true division: an all-integer `/` yields f64 (not integer div).
    let b = one("def main():\n    auto a = array[i64]([1, 2])\n    \
             auto c = array[i64]([2, 4])\n    print(sum(a / c))\n");
    assert!(matches!(b.op, BinOp::Div));
    assert_eq!(b.out, Dtype::F64);
    assert!(!b.scalar);
    assert!(!b.scalar_lhs);
    // Narrow integer dtypes also lift all the way to f64, not f32.
    let b8 = one("def main():\n    auto a = array[i8]([1, 2])\n    \
             auto c = array[i8]([2, 4])\n    print(sum(a / c))\n");
    assert_eq!(b8.out, Dtype::F64);
}

#[test]
fn float_array_div_keeps_width() {
    // f32 / f32 stays f32; a mixed f32/f64 array-array div promotes to f64.
    let b = one("def main():\n    auto a = array[f32]([1.0, 2.0])\n    \
             auto c = array[f32]([2.0, 4.0])\n    print(sum(a / c))\n");
    assert_eq!(b.out, Dtype::F32);
    let m = one("def main():\n    auto a = array[f32]([1.0, 2.0])\n    \
             auto c = array[f64]([2.0, 4.0])\n    print(sum(a / c))\n");
    assert_eq!(m.out, Dtype::F64);
}

#[test]
fn array_div_array_emits_broadcast_check() {
    // Like +,-,*, a two-array `/` guards shape compatibility at runtime.
    let src = "def main():\n    auto a = array[i64]([[1, 2], [3, 4]])\n    \
                   auto v = array[i64]([10, 20])\n    print(sum(a / v))\n";
    assert_eq!(broadcast_checks(src), 1);
    assert_eq!(one(src).out, Dtype::F64);
}

// --- scalar on the right ---

#[test]
fn array_div_scalar_is_true_division() {
    // int array / int scalar -> f64; f32 array / int scalar stays f32 (weak).
    let bi = one("def main():\n    auto a = array[i64]([1, 2])\n    print(sum(a / 2))\n");
    assert!(matches!(bi.op, BinOp::Div));
    assert_eq!(bi.out, Dtype::F64);
    assert!(bi.scalar && !bi.scalar_lhs);
    let bf = one("def main():\n    auto a = array[f32]([1.0, 2.0])\n    print(sum(a / 2))\n");
    assert_eq!(bf.out, Dtype::F32);
    assert!(bf.scalar && !bf.scalar_lhs);
}

#[test]
fn array_div_by_literal_does_not_take_scalar_fast_path() {
    // The scalar `/`-by-nonzero-literal fast path (#374) must not fire for an
    // array operand: `a / 2` lowers to an ArrayBinOp, never an Inst::Bin div.
    let src = "def main():\n    auto a = array[i64]([1, 2])\n    print(sum(a / 2))\n";
    assert_eq!(binops(src).len(), 1);
    assert!(!has_bin_div(src));
}

#[test]
fn array_sub_scalar_on_right_is_plain() {
    let b = one("def main():\n    auto a = array[i64]([1, 2])\n    print(sum(a - 3))\n");
    assert!(matches!(b.op, BinOp::Sub));
    assert_eq!(b.out, Dtype::I64);
    assert!(b.scalar && !b.scalar_lhs);
}

// --- scalar on the left ---

#[test]
fn scalar_div_array_is_scalar_lhs() {
    // `s / a` keeps scalar-on-left ordering (non-commutative) and true-divides.
    let b = one("def main():\n    auto a = array[i64]([1, 2])\n    print(sum(2 / a))\n");
    assert!(matches!(b.op, BinOp::Div));
    assert_eq!(b.out, Dtype::F64);
    assert!(b.scalar && b.scalar_lhs);
    // float scalar on the left over an f32 array keeps f32.
    let bf = one("def main():\n    auto a = array[f32]([1.0, 2.0])\n    print(sum(2.0 / a))\n");
    assert_eq!(bf.out, Dtype::F32);
    assert!(bf.scalar_lhs);
}

#[test]
fn scalar_sub_array_is_scalar_lhs() {
    let b = one("def main():\n    auto a = array[i64]([1, 2])\n    print(sum(10 - a))\n");
    assert!(matches!(b.op, BinOp::Sub));
    assert_eq!(b.out, Dtype::I64);
    assert!(b.scalar && b.scalar_lhs);
}

#[test]
fn commutative_scalar_on_left_is_reordered_not_flagged() {
    // `s + a` / `s * a` reorder to `array op scalar`, so they never set
    // scalar_lhs (the kernel computes `array[i] op scalar`).
    let add = one("def main():\n    auto a = array[i64]([1, 2])\n    print(sum(2 + a))\n");
    assert!(matches!(add.op, BinOp::Add) && add.scalar && !add.scalar_lhs);
    let mul = one("def main():\n    auto a = array[i64]([1, 2])\n    print(sum(3 * a))\n");
    assert!(matches!(mul.op, BinOp::Mul) && mul.scalar && !mul.scalar_lhs);
}

// --- rejections / deferrals ---

#[test]
fn floor_div_on_arrays_is_deferred() {
    // `//` is not yet wired for arrays (#506 ships `/` only).
    lower_fails("def main():\n    auto a = array[i64]([1, 2])\n    auto b = a // 2\n");
    lower_fails(
        "def main():\n    auto a = array[i64]([1, 2])\n    \
             auto c = array[i64]([2, 4])\n    auto b = a // c\n",
    );
    lower_fails("def main():\n    auto a = array[i64]([1, 2])\n    auto b = 8 // a\n");
}

#[test]
fn mod_on_arrays_is_deferred() {
    lower_fails("def main():\n    auto a = array[i64]([1, 2])\n    auto b = a % 2\n");
}
