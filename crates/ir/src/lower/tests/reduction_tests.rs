//! Array reduction methods `a.sum()/min()/max()/mean()/prod()` with an
//! optional `axis=` (#507, D-0170). Host-only (AST->ZIR): asserts which
//! runtime entry point each form lowers to (full reductions keep the dtype
//! suffix; `axis=` routes to the axis kernels), that `min`/`max` and the
//! `axis=` forms are raising (Invoke under `main`'s implicit handler), and
//! that bad surfaces are rejected — a walled garden around the dispatch,
//! independent of the runtime kernels (which run in the CI goldens).
use crate::lower::lower_module;
use zapc_ir_core::Inst;

/// Every runtime-call name emitted (Call + the unwinding Invoke).
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

/// True when `func` is emitted as a raising Invoke (not a plain Call).
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
    assert!(perrs.is_empty() && lerrs.is_empty());
    let (_m, errs) = lower_module(&module);
    assert!(
        errs.is_empty(),
        "unexpected lowering errors for\n{src}\n{errs:?}"
    );
}

// --- full reductions -> scalar ---

#[test]
fn full_reductions_hit_their_dtype_entry_points() {
    let pre = "def main():\n    auto a = array[i64]([3, 1, 2])\n    ";
    assert!(has(&format!("{pre}print(a.sum())\n"), "zap_array_sum_i64"));
    assert!(has(&format!("{pre}print(a.min())\n"), "zap_array_min_i64"));
    assert!(has(&format!("{pre}print(a.max())\n"), "zap_array_max_i64"));
    assert!(has(
        &format!("{pre}print(a.prod())\n"),
        "zap_array_prod_i64"
    ));
    assert!(has(
        &format!("{pre}print(a.mean())\n"),
        "zap_array_mean_i64"
    ));
    // A float array selects the f-suffixed entry points.
    let pf = "def main():\n    auto a = array[f32]([3.0, 1.0])\n    ";
    assert!(has(&format!("{pf}print(a.sum())\n"), "zap_array_sum_f32"));
    assert!(has(&format!("{pf}print(a.mean())\n"), "zap_array_mean_f32"));
}

#[test]
fn min_max_are_raising_but_sum_prod_mean_are_not() {
    // min/max raise ValueError on an empty array, so they invoke; sum/prod/
    // mean have defined empty results and lower to a plain call.
    let pre = "def main():\n    auto a = array[i64]([3, 1, 2])\n    ";
    assert!(is_invoked(
        &format!("{pre}print(a.min())\n"),
        "zap_array_min_i64"
    ));
    assert!(is_invoked(
        &format!("{pre}print(a.max())\n"),
        "zap_array_max_i64"
    ));
    assert!(!is_invoked(
        &format!("{pre}print(a.sum())\n"),
        "zap_array_sum_i64"
    ));
    assert!(!is_invoked(
        &format!("{pre}print(a.prod())\n"),
        "zap_array_prod_i64"
    ));
    assert!(!is_invoked(
        &format!("{pre}print(a.mean())\n"),
        "zap_array_mean_i64"
    ));
}

// --- axis reductions ---

#[test]
fn axis_reduction_on_nd_routes_to_the_axis_kernel() {
    let pre = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    ";
    // sum/min/max/prod share zap_array_reduce_axis; mean has its own.
    assert!(has(
        &format!("{pre}print(sum(m.sum(axis=0)))\n"),
        "zap_array_reduce_axis"
    ));
    assert!(has(
        &format!("{pre}print(sum(m.max(axis=1)))\n"),
        "zap_array_reduce_axis"
    ));
    assert!(has(
        &format!("{pre}print(sum(m.mean(axis=0)))\n"),
        "zap_array_mean_axis"
    ));
    // These are raising (out-of-range axis -> ValueError).
    assert!(is_invoked(
        &format!("{pre}print(sum(m.sum(axis=0)))\n"),
        "zap_array_reduce_axis"
    ));
    assert!(is_invoked(
        &format!("{pre}print(sum(m.mean(axis=0)))\n"),
        "zap_array_mean_axis"
    ));
    // A negative axis is accepted (normalized at runtime).
    lowers_ok(&format!("{pre}print(sum(m.sum(axis=-1)))\n"));
}

#[test]
fn axis_on_rank1_is_a_full_reduction_with_an_axis_check() {
    // a.sum(axis=0) on a 1-D array is the scalar full reduction, but the axis
    // is still validated at runtime (so axis=5 faults).
    let src = "def main():\n    auto a = array[i64]([1, 2, 3])\n    print(a.sum(axis=0))\n";
    assert!(has(src, "zap_array_sum_i64"));
    assert!(has(src, "zap_array_axis_check"));
    assert!(!has(src, "zap_array_reduce_axis"));
}

// --- rejections ---

#[test]
fn unknown_array_method_is_rejected() {
    lower_fails("def main():\n    auto a = array[i64]([1, 2])\n    print(a.median())\n");
}

#[test]
fn positional_arg_to_a_reduction_is_rejected() {
    // The axis must be passed as a keyword: `a.sum(0)` is rejected.
    lower_fails("def main():\n    auto a = array[i64]([1, 2])\n    print(a.sum(0))\n");
}

#[test]
fn unknown_keyword_to_a_reduction_is_rejected() {
    lower_fails("def main():\n    auto a = array[i64]([1, 2])\n    print(a.sum(dim=0))\n");
}
