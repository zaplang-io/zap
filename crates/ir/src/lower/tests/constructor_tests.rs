//! Array constructors zeros/ones/full/arange/linspace (#504, D-0167).
//! Host-only (AST->ZIR): the interception, shape parsing, and runtime-call
//! wiring are a walled garden independent of codegen.
use crate::lower::lower_module;
use zapc_ir_core::Inst;

fn calls(src: &str) -> Vec<String> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
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
            // arange lowers to a throwing call (Invoke) so its zero-step
            // ValueError is catchable; collect both call forms.
            Inst::Call { func, .. } | Inst::Invoke { func, .. } => Some(func.clone()),
            _ => None,
        })
        .collect()
}

fn count(src: &str, func: &str) -> usize {
    calls(src).iter().filter(|f| *f == func).count()
}

fn lowers_ok(src: &str) {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty() && lerrs.is_empty());
    let (_m, errs) = lower_module(&module);
    assert!(
        errs.is_empty(),
        "unexpected lowering errors for\n{src}\n{errs:?}"
    );
}

fn lower_fails(src: &str) {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty() && lerrs.is_empty());
    let (_m, errs) = lower_module(&module);
    assert!(!errs.is_empty(), "expected a lowering error for\n{src}");
}

#[test]
fn zeros_ones_full_lower_with_shape_and_fill() {
    // 1-D int zeros: new_nd + one set_dim + fill_i.
    let z = "def main():\n    auto a = zeros[i64](3)\n    print(sum(a))\n";
    lowers_ok(z);
    assert_eq!(count(z, "zap_array_new_nd"), 1);
    assert_eq!(count(z, "zap_array_set_dim"), 1);
    assert_eq!(count(z, "zap_array_fill_i"), 1);
    // N-D float ones: two set_dim, fill_f.
    let o = "def main():\n    auto a = ones[f64]((2, 3))\n    print(sum(a))\n";
    lowers_ok(o);
    assert_eq!(count(o, "zap_array_set_dim"), 2);
    assert_eq!(count(o, "zap_array_fill_f"), 1);
    // full with a value; runtime-valued dims are allowed.
    lowers_ok(
        "def main():\n    auto n = 4\n    auto a = full[i32]((n, 2), 7)\n    print(sum(a))\n",
    );
}

#[test]
fn arange_and_linspace_lower() {
    // integer arange with explicit + default step.
    let a = "def main():\n    auto a = arange[i64](0, 10, 2)\n    print(sum(a))\n";
    lowers_ok(a);
    assert_eq!(count(a, "zap_array_arange_i"), 1);
    lowers_ok("def main():\n    auto a = arange[i64](0, 5)\n    print(len(a))\n");
    // float arange uses the _f entry point.
    let af = "def main():\n    auto a = arange[f64](0.0, 1.0, 0.25)\n    print(len(a))\n";
    lowers_ok(af);
    assert_eq!(count(af, "zap_array_arange_f"), 1);
    // linspace.
    let l = "def main():\n    auto a = linspace[f64](0.0, 1.0, 5)\n    print(a[4])\n";
    lowers_ok(l);
    assert_eq!(count(l, "zap_array_linspace"), 1);
}

#[test]
fn constructor_misuse_is_rejected() {
    lower_fails("def main():\n    auto a = zeros[f128](3)\n"); // bad dtype
    lower_fails("def main():\n    auto a = zeros[f64](())\n"); // empty shape
    lower_fails("def main():\n    auto a = full[f64]((2, 2))\n"); // missing value
    lower_fails("def main():\n    auto a = zeros[f64](3, 4)\n"); // dims must be a tuple, not varargs
    lower_fails("def main():\n    auto a = arange[i64](0)\n"); // too few args
    lower_fails("def main():\n    auto a = linspace[f64](0.0, 1.0)\n"); // needs num
}
