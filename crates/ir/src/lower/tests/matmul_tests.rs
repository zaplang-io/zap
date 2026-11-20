//! Matrix multiply `@` / `dot` (#509, D-0172, PEP 465). Host-only (AST->ZIR):
//! asserts which runtime kernel each operand-rank combination lowers to
//! (1-D·1-D dot -> scalar `dot{i,f}`; the array-producing cases -> `matmul`),
//! that all are raising (Invoke under `main`'s handler, for the shape-mismatch
//! ValueError), that `dot(a, b)` matches `a @ b`, and that `@=` and the `@`
//! precedence parse. A walled garden around the operator plumbing + dispatch,
//! independent of the runtime kernels (which run in the CI goldens).
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
fn one_d_dot_product_is_a_scalar_dot() {
    // int 1-D·1-D -> integer dot; it is raising (length mismatch -> ValueError).
    let si = "def main():\n    auto a = array[i64]([1, 2, 3])\n    \
                  auto b = array[i64]([4, 5, 6])\n    print(a @ b)\n";
    assert!(is_invoked(si, "zap_array_doti"));
    assert!(!has(si, "zap_array_matmul"));
    // float operand -> float dot.
    let sf = "def main():\n    auto a = array[f64]([1.0, 2.0])\n    \
                  auto b = array[f64]([3.0, 4.0])\n    print(a @ b)\n";
    assert!(is_invoked(sf, "zap_array_dotf"));
}

#[test]
fn two_d_matmul_uses_the_matmul_kernel() {
    let s = "def main():\n    auto a = array[i64]([[1, 2], [3, 4]])\n    \
                 auto b = array[i64]([[5, 6], [7, 8]])\n    auto c = a @ b\n    print(sum(c))\n";
    assert!(is_invoked(s, "zap_array_matmul"));
    assert!(!has(s, "zap_array_doti"));
}

#[test]
fn mixed_rank_matmul_uses_the_matmul_kernel() {
    // 2-D · 1-D and 1-D · 2-D both produce a 1-D array via matmul (not dot).
    let s1 = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
                  auto v = array[i64]([5, 6])\n    print(sum(m @ v))\n";
    assert!(is_invoked(s1, "zap_array_matmul"));
    let s2 = "def main():\n    auto v = array[i64]([5, 6])\n    \
                  auto m = array[i64]([[1, 2], [3, 4]])\n    print(sum(v @ m))\n";
    assert!(is_invoked(s2, "zap_array_matmul"));
}

#[test]
fn dot_builtin_matches_the_operator() {
    let s = "def main():\n    auto a = array[i64]([1, 2, 3])\n    \
                 auto b = array[i64]([4, 5, 6])\n    print(dot(a, b))\n";
    assert!(is_invoked(s, "zap_array_doti"));
    let s2 = "def main():\n    auto a = array[i64]([[1, 2], [3, 4]])\n    \
                  auto b = array[i64]([[5, 6], [7, 8]])\n    print(sum(dot(a, b)))\n";
    assert!(is_invoked(s2, "zap_array_matmul"));
}

#[test]
fn in_place_matmul_lowers() {
    // `m @= n` rebinds through the matmul kernel.
    let s = "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
                 auto n = array[i64]([[1, 0], [0, 1]])\n    m @= n\n    print(sum(m))\n";
    lowers_ok(s);
    assert!(is_invoked(s, "zap_array_matmul"));
}

#[test]
fn matmul_binds_like_multiply() {
    // PEP 465: `@` shares `*`'s precedence, above `+`. `a + b @ c` parses as
    // `a + (b @ c)`, so the matmul lowers (the dot runs before the scalar add
    // would even apply). Here a/b/c are 1-D so `b @ c` is a scalar dot.
    let s = "def main():\n    auto b = array[i64]([1, 2])\n    \
                 auto c = array[i64]([3, 4])\n    print(10 + b @ c)\n";
    lowers_ok(s);
    assert!(is_invoked(s, "zap_array_doti"));
}

#[test]
fn matmul_on_scalars_is_rejected() {
    // `@` is only defined for arrays; a scalar `@` is a type error (sema) —
    // and non-array operands never reach a matmul kernel.
    lower_fails("def main():\n    auto x = 3 @ 4\n");
}
