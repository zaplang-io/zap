//! Complex array comparisons and reductions must be rejected at lowering, not
//! silently mis-routed to int/float kernels (zaplang-io/zap#1721) — the sibling
//! cluster of #1719 (complex `@`/`dot`).
//!
//! A `complex` array element (#938) is a 16-byte `(re, im)` pair carried at runtime
//! as a pointer-word. Several array ops dispatch on `is_float()` / dtype and have no
//! `DTYPE_COMPLEX` case, so a complex operand used to fall through:
//!
//! - **Comparisons** `== != < <= > >=` (`lower_array_cmp` → `zap_array_cmp` /
//!   `zap_array_cmp_scalar`) `mm_load_i` the complex word as a raw `int64` → silent
//!   garbage (even `==`).
//! - **Reductions** `min`/`max`/`prod`/`mean` and axis-`sum` (`lower_array_method`):
//!   `min`/`max`/`prod`/`mean` on complex emit undefined `zap_array_{…}_complex`
//!   (link error), and `sum(axis=)` routes to `zap_array_reduce_axis`, whose switch
//!   has no `DTYPE_COMPLEX` case (`default: break`) → an unfilled result buffer
//!   (silently wrong).
//!
//! The stopgap (mirroring #1719 and the sort/unique complex guards) rejects these at
//! lowering — a compile-time error, never a corrupting run or a link error:
//!   - ordering `< <= > >=` and `min`/`max`: complex has **no ordering** (Zap's choice,
//!     same as `sort`/`argsort` reject), a permanent rejection.
//!   - `==`/`!=`, `prod`/`mean`, and axis reductions: **not yet supported** for complex
//!     (real kernels deferred to the complex-array epic #938).
//!   - full `sum(a)` over a complex array stays supported (`zap_array_sum_complex`).
//!
//! Host-only lowering checks (no LLVM), the analog of `collection_order_lower.rs`.

use zapc_ir::{lower_module, Inst, Module};
use zapc_syntax::parser::parse_str;

/// Runtime symbols that must NOT be emitted for a rejected complex op (emitting one
/// means the mis-route is back).
const CMP: &[&str] = &["zap_array_cmp", "zap_array_cmp_scalar"];
const REDUCE_AXIS: &[&str] = &["zap_array_reduce_axis", "zap_array_mean_axis"];

fn lower(src: &str) -> (Module, Vec<String>) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    let errs: Vec<String> = errs.iter().map(|e| format!("{e:?}")).collect();
    (module, errs)
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

/// Assert `src` is rejected at lowering with an error containing `needle`, and that
/// none of `forbidden` runtime symbols were emitted.
fn rejects(src: &str, needle: &str, forbidden: &[&str]) {
    let (m, errs) = lower(src);
    assert!(
        errs.iter().any(|e| e.contains(needle)),
        "expected a lowering error containing {needle:?}; got errs={errs:?}, symbols={:?}",
        called_symbols(&m)
    );
    let syms = called_symbols(&m);
    for bad in forbidden {
        assert!(
            !syms.iter().any(|s| s == bad),
            "a rejected complex op must not emit {bad:?}; got {syms:?}"
        );
    }
}

/// Assert `src` lowers with no errors and emits `expect`.
fn lowers_with(src: &str, expect: &str) {
    let (m, errs) = lower(src);
    assert!(errs.is_empty(), "unexpected lower errors: {errs:?}");
    let syms = called_symbols(&m);
    assert!(
        syms.iter().any(|s| s == expect),
        "expected {expect:?} to be emitted; got {syms:?}"
    );
}

// Source helpers.
const CA: &str = "array[complex]([complex(1.0, 1.0), complex(2.0, 0.0)])";
const CB: &str = "array[complex]([complex(0.0, 1.0), complex(1.0, 0.0)])";
const CM: &str = "array[complex]([[complex(1.0, 0.0), complex(0.0, 1.0)], \
                  [complex(0.0, -1.0), complex(1.0, 0.0)]])";
const FA: &str = "array[float]([1.0, 2.0])";
const FM: &str = "array[float]([[1.0, 2.0], [3.0, 4.0]])";

fn prog(body: &str) -> String {
    format!("def main():\n{body}")
}

// ---- comparisons: ordering has no complex ordering (permanent) -------------

#[test]
fn complex_array_ordering_lt_rejects() {
    rejects(
        &prog(&format!(
            "    auto a = {CA}\n    auto b = {CB}\n    print(str((a < b).ndim))\n"
        )),
        "no ordering",
        CMP,
    );
}

#[test]
fn complex_array_ordering_all_preds_reject() {
    for op in ["<", "<=", ">", ">="] {
        rejects(
            &prog(&format!(
                "    auto a = {CA}\n    auto b = {CB}\n    print(str((a {op} b).ndim))\n"
            )),
            "no ordering",
            CMP,
        );
    }
}

#[test]
fn complex_array_ordering_vs_real_scalar_rejects() {
    // array-vs-scalar path (`zap_array_cmp_scalar`) must also reject.
    rejects(
        &prog(&format!(
            "    auto a = {CA}\n    print(str((a > 1.0).ndim))\n"
        )),
        "no ordering",
        CMP,
    );
}

// ---- comparisons: equality is meaningful but kernel deferred (#938) --------

#[test]
fn complex_array_eq_rejects_not_yet_supported() {
    rejects(
        &prog(&format!(
            "    auto a = {CA}\n    auto b = {CB}\n    print(str((a == b).ndim))\n"
        )),
        "not yet supported",
        CMP,
    );
}

#[test]
fn complex_array_ne_rejects_not_yet_supported() {
    rejects(
        &prog(&format!(
            "    auto a = {CA}\n    auto b = {CB}\n    print(str((a != b).ndim))\n"
        )),
        "not yet supported",
        CMP,
    );
}

#[test]
fn real_scalar_lhs_vs_complex_array_eq_rejects() {
    // scalar-vs-array path with the complex array on the right.
    rejects(
        &prog(&format!(
            "    auto b = {CB}\n    print(str((1.0 == b).ndim))\n"
        )),
        "not yet supported",
        CMP,
    );
}

// ---- reductions: min/max have no ordering (permanent) ----------------------

#[test]
fn complex_min_max_method_reject_no_ordering() {
    for m in ["min", "max"] {
        rejects(
            &prog(&format!("    auto a = {CA}\n    print(str(a.{m}()))\n")),
            "no ordering",
            REDUCE_AXIS,
        );
    }
}

#[test]
fn complex_min_max_axis_reject_no_ordering() {
    for m in ["min", "max"] {
        rejects(
            &prog(&format!(
                "    auto a = {CM}\n    print(str(a.{m}(axis=0).ndim))\n"
            )),
            "no ordering",
            REDUCE_AXIS,
        );
    }
}

// ---- reductions: prod/mean meaningful but kernel deferred (#938) -----------

#[test]
fn complex_prod_method_rejects_not_yet_supported() {
    rejects(
        &prog(&format!("    auto a = {CA}\n    print(str(a.prod()))\n")),
        "not yet supported",
        REDUCE_AXIS,
    );
}

#[test]
fn complex_mean_method_rejects_not_yet_supported() {
    rejects(
        &prog(&format!("    auto a = {CA}\n    print(str(a.mean()))\n")),
        "not yet supported",
        REDUCE_AXIS,
    );
}

#[test]
fn complex_mean_axis_rejects_not_yet_supported() {
    rejects(
        &prog(&format!(
            "    auto a = {CM}\n    print(str(a.mean(axis=0).ndim))\n"
        )),
        "not yet supported",
        REDUCE_AXIS,
    );
}

// ---- reductions: full sum stays supported; axis sum is deferred ------------

#[test]
fn complex_sum_full_method_still_lowers() {
    lowers_with(
        &prog(&format!("    auto a = {CA}\n    print(str(a.sum()))\n")),
        "zap_array_sum_complex",
    );
}

#[test]
fn complex_sum_builtin_still_lowers() {
    lowers_with(
        &prog(&format!("    auto a = {CA}\n    print(str(sum(a)))\n")),
        "zap_array_sum_complex",
    );
}

#[test]
fn complex_sum_axis_rejects_not_yet_supported() {
    rejects(
        &prog(&format!(
            "    auto a = {CM}\n    print(str(a.sum(axis=0).ndim))\n"
        )),
        "not yet supported",
        REDUCE_AXIS,
    );
}

// ---- positive controls: real/int ops are untouched (guards are narrow) -----

#[test]
fn real_array_eq_still_lowers() {
    lowers_with(
        &prog(&format!(
            "    auto a = {FA}\n    auto b = {FA}\n    print(str((a == b).ndim))\n"
        )),
        "zap_array_cmp",
    );
}

#[test]
fn real_array_ordering_still_lowers() {
    lowers_with(
        &prog(&format!(
            "    auto a = {FA}\n    auto b = {FA}\n    print(str((a < b).ndim))\n"
        )),
        "zap_array_cmp",
    );
}

#[test]
fn real_min_method_still_lowers() {
    lowers_with(
        &prog(&format!("    auto a = {FA}\n    print(str(a.min()))\n")),
        "zap_array_min_f64",
    );
}

#[test]
fn real_prod_method_still_lowers() {
    lowers_with(
        &prog(&format!("    auto a = {FA}\n    print(str(a.prod()))\n")),
        "zap_array_prod_f64",
    );
}

#[test]
fn real_sum_axis_still_lowers() {
    lowers_with(
        &prog(&format!(
            "    auto a = {FM}\n    print(str(a.sum(axis=0).ndim))\n"
        )),
        "zap_array_reduce_axis",
    );
}

#[test]
fn real_mean_axis_still_lowers() {
    lowers_with(
        &prog(&format!(
            "    auto a = {FM}\n    print(str(a.mean(axis=0).ndim))\n"
        )),
        "zap_array_mean_axis",
    );
}
