//! Complex `@` / `dot` must be rejected at lowering, not silently mis-routed to
//! the integer matmul kernel (zaplang-io/zap#1719).
//!
//! A `complex` array element (#938) is a 16-byte `(re, im)` pair carried at
//! runtime as a pointer-word. `Dtype::promote(Complex, _) -> Complex` (code 11),
//! and `Complex` is neither `F32` nor `F64`, so before the guard the 1-D dot path
//! fell through to `zap_array_doti` and the 2-D path to `zap_array_matmul` with
//! the integer kernel (`mm_load_i`/`arr_store_i` have no `Complex` case → they
//! read/write the complex pointer-word as a raw `int64`) — silent garbage and
//! memory corruption with no error.
//!
//! The stopgap (size:S) rejects complex `@`/`dot` cleanly in `lower_array_matmul`,
//! exactly as sort/argsort/unique/ordering already reject complex — a compile-time
//! lowering error, not a runtime raise, and not a corrupting run. The full complex
//! matmul kernel (`zap_matmul_c`/`zap_array_dotc`) is deferred to the complex-array
//! epic #938; a later real kernel removes this guard and flips these cases to value
//! checks.
//!
//! These host-only checks run everywhere (no LLVM). They are the analog of
//! `collection_order_lower.rs` for the array matmul dtype guard, and they form the
//! walled garden so no future change re-opens the mis-route: every surface form
//! (`a @ b` and `dot(a, b)`), every shape pairing (1-D·1-D, 1-D·2-D, 2-D·1-D,
//! 2-D·2-D), and every complex↔real mix (complex@complex, complex@real,
//! real@complex) must reject, while every real/int matmul must still lower.

use zapc_ir::{lower_module, Inst, Module};
use zapc_syntax::parser::parse_str;

/// The guard's message substring. A later real complex kernel (#938) removes the
/// guard; these tests then fail loudly, flagging that they must flip to value
/// checks — the walled garden working as intended.
const NEEDLE: &str = "not yet supported for complex arrays";

/// Runtime symbols a *successful* (non-complex) matmul lowering emits. If any of
/// these ever appears for a complex operand, the mis-route is back.
const INT_MATMUL: &str = "zap_array_matmul";
const INT_DOT: &str = "zap_array_doti";

fn lower_err_contains(src: &str, needle: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(
        errs.iter().any(|e| format!("{e:?}").contains(needle)),
        "expected a lowering error containing {needle:?}; got errs={errs:?}, \
         called symbols={:?}",
        called_symbols(&module)
    );
    // The reject must happen *before* any matmul/dot call is emitted — a guard
    // that errored but still lowered the corrupting call would be no fix at all.
    let syms = called_symbols(&module);
    assert!(
        !syms.iter().any(|s| s == INT_MATMUL || s == INT_DOT),
        "complex matmul must emit no matmul/dot runtime call; got {syms:?}"
    );
}

fn lower_ok(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "unexpected lower errors: {errs:?}");
    module
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

fn has(m: &Module, sym: &str) -> bool {
    called_symbols(m).iter().any(|s| s == sym)
}

// Source helpers -----------------------------------------------------------

/// A 1-D complex vector literal.
const CX_VEC_A: &str = "array[complex]([complex(1.0, 1.0), complex(2.0, 0.0)])";
const CX_VEC_B: &str = "array[complex]([complex(0.0, 1.0), complex(1.0, 0.0)])";
/// A 2x2 complex matrix literal.
const CX_MAT: &str = "array[complex]([[complex(1.0, 0.0), complex(0.0, 1.0)], \
                      [complex(0.0, -1.0), complex(1.0, 0.0)]])";
/// Real counterparts (default `float`/`int` dtype).
const F_VEC: &str = "array[float]([1.0, 2.0])";
const F_MAT: &str = "array[float]([[1.0, 2.0], [3.0, 4.0]])";

fn main_body(binding: &str, expr: &str) -> String {
    format!("def main():\n    auto {binding} = {expr}\n    print(str(({binding}).ndim))\n")
}

// --- 1-D · 1-D dot product, `a @ b` ---------------------------------------

#[test]
fn complex_dot_at_operator_rejects() {
    // The issue's first repro: `a @ b` on two complex vectors mis-routed to
    // `zap_array_doti` (int64), reading the complex pointer-word as an int.
    lower_err_contains(
        &format!(
            "def main():\n    auto a = {CX_VEC_A}\n    auto b = {CX_VEC_B}\n    print(str(a @ b))\n"
        ),
        NEEDLE,
    );
}

#[test]
fn complex_dot_function_spelling_rejects() {
    // Same op, `dot(a, b)` spelling — also routes through `lower_array_matmul`.
    lower_err_contains(
        &format!(
            "def main():\n    auto a = {CX_VEC_A}\n    auto b = {CX_VEC_B}\n    print(str(dot(a, b)))\n"
        ),
        NEEDLE,
    );
}

// --- 2-D · 2-D matrix product ---------------------------------------------

#[test]
fn complex_matmul_2d_at_operator_rejects() {
    // The issue's second repro: `m @ m` mis-routed to the integer kernel with
    // dtype code 11, writing int64 words into a complex result buffer (corruption).
    lower_err_contains(
        &format!("def main():\n    auto m = {CX_MAT}\n    print(str((m @ m).ndim))\n"),
        NEEDLE,
    );
}

#[test]
fn complex_matmul_2d_function_spelling_rejects() {
    lower_err_contains(
        &format!("def main():\n    auto m = {CX_MAT}\n    print(str(dot(m, m).ndim))\n"),
        NEEDLE,
    );
}

// --- mixed rank: 1-D · 2-D and 2-D · 1-D ----------------------------------

#[test]
fn complex_matmul_1d_2d_rejects() {
    lower_err_contains(
        &format!(
            "def main():\n    auto v = {CX_VEC_A}\n    auto m = {CX_MAT}\n    print(str((v @ m).ndim))\n"
        ),
        NEEDLE,
    );
}

#[test]
fn complex_matmul_2d_1d_rejects() {
    lower_err_contains(
        &format!(
            "def main():\n    auto v = {CX_VEC_A}\n    auto m = {CX_MAT}\n    print(str((m @ v).ndim))\n"
        ),
        NEEDLE,
    );
}

// --- mixed complex ↔ real (promote makes either-complex → Complex) ---------

#[test]
fn complex_at_real_rejects() {
    // complex @ real: `promote(Complex, F64) -> Complex`, so a single out-is-Complex
    // check in the guard must catch this.
    lower_err_contains(
        &format!(
            "def main():\n    auto a = {CX_VEC_A}\n    auto b = {F_VEC}\n    print(str(a @ b))\n"
        ),
        NEEDLE,
    );
}

#[test]
fn real_at_complex_rejects() {
    // real @ complex: `promote(F64, Complex) -> Complex` — symmetric.
    lower_err_contains(
        &format!(
            "def main():\n    auto a = {F_VEC}\n    auto b = {CX_VEC_B}\n    print(str(a @ b))\n"
        ),
        NEEDLE,
    );
}

#[test]
fn complex_at_real_2d_rejects() {
    lower_err_contains(
        &format!(
            "def main():\n    auto a = {CX_MAT}\n    auto b = {F_MAT}\n    print(str((a @ b).ndim))\n"
        ),
        NEEDLE,
    );
}

// --- positive controls: real/int matmul must still lower (guard is narrow) --

#[test]
fn real_matmul_2d_still_lowers() {
    let m = lower_ok(&main_body("c", &format!("{F_MAT} @ {F_MAT}")));
    assert!(
        has(&m, INT_MATMUL),
        "a real 2-D matmul must still emit zap_array_matmul; got {:?}",
        called_symbols(&m)
    );
}

#[test]
fn real_dot_1d_still_lowers() {
    // Float dot uses the float accumulator kernel; the guard must not touch it.
    let m = lower_ok(&format!(
        "def main():\n    auto a = {F_VEC}\n    auto b = {F_VEC}\n    print(str(a @ b))\n"
    ));
    assert!(
        has(&m, "zap_array_dotf"),
        "a real 1-D dot must still emit a dot kernel; got {:?}",
        called_symbols(&m)
    );
}

#[test]
fn int_matmul_2d_still_lowers() {
    // The integer kernel path is correct for int dtypes — only complex was the
    // mis-route. Prove the guard does not over-reject int64.
    let src = "def main():\n    auto a = array[int64]([[1, 2], [3, 4]])\n    \
               auto b = array[int64]([[5, 6], [7, 8]])\n    print(str((a @ b).ndim))\n";
    let m = lower_ok(src);
    assert!(
        has(&m, INT_MATMUL),
        "an int 2-D matmul must still emit zap_array_matmul; got {:?}",
        called_symbols(&m)
    );
}
