//! Element-wise math over arrays (#505, D-0168): `math.<fn>(array)` maps the
//! function and yields a new float array, via an `Inst::ArrayUnaryOp`. Scalar
//! `math.<fn>(x)` is unchanged. Host-only (AST->ZIR).
use crate::lower::lower_module;
use zapc_ir_core::Inst;

fn lower(src: &str) -> zapc_ir_core::Module {
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
    m
}

fn unary_ops(m: &zapc_ir_core::Module) -> usize {
    m.funcs
        .iter()
        .flat_map(|f| f.blocks.iter())
        .flat_map(|b| b.insts.iter())
        .filter(|i| matches!(i, Inst::ArrayUnaryOp { .. }))
        .count()
}

fn lower_fails(src: &str) {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty() && lerrs.is_empty());
    let (_m, errs) = lower_module(&module);
    assert!(!errs.is_empty(), "expected a lowering error for\n{src}");
}

#[test]
fn math_over_array_emits_unary_op() {
    let m = lower(
            "def main():\n    auto a = array[f64]([4.0, 9.0])\n    auto b = math.sqrt(a)\n    print(sum(b))\n",
        );
    assert_eq!(unary_ops(&m), 1);
    // int array -> still one unary op (result is f64).
    let m2 = lower("def main():\n    auto a = array[i64]([1, 4])\n    print(sum(math.exp(a)))\n");
    assert_eq!(unary_ops(&m2), 1);
    // every wired function + N-D shape.
    lower(
        "def main():\n    auto m = array[f64]([[1.0, 2.0], [3.0, 4.0]])\n    \
             print(sum(math.sqrt(m)), sum(math.exp(m)), sum(math.log(m)), sum(math.sin(m)), \
             sum(math.cos(m)), sum(math.tan(m)), sum(math.fabs(m)))\n",
    );
}

#[test]
fn scalar_math_is_unchanged() {
    // No array involved -> no ArrayUnaryOp, still a scalar call.
    let m = lower("def main():\n    print(math.sqrt(2.0))\n");
    assert_eq!(unary_ops(&m), 0);
    let m2 = lower("def main():\n    auto x = 9\n    print(math.sqrt(x))\n");
    assert_eq!(unary_ops(&m2), 0);
}

#[test]
fn unwired_math_fn_on_array_is_rejected() {
    // floor/round/etc. are not element-wise-wired yet (#505 ships the core set).
    lower_fails("def main():\n    auto a = array[f64]([1.5, 2.5])\n    auto b = math.floor(a)\n");
}
