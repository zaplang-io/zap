//! A parameter whose declared type is a function type `fn(...) -> T` must
//! type-check its argument: a value that is *not a function* (e.g. an `int`)
//! passed where a `fn(...)` is expected must be rejected (#1104).
//!
//! This is the sibling of the #465 fn-value coercion check. #465 rejects a
//! function value whose *signature* disagrees with the expected `fn(...)` type;
//! it fires only when the source is itself a `Ty::Func`. A *non-function* source
//! (a scalar, a struct, a collection) slipped straight through, so `takes(7)` for
//! a `fn() -> int64` parameter compiled silently and coerced an `i64` word into a
//! pointer (closure-record) slot. #1104 closes that gap in the same funnel.
//!
//! The legitimate signature-less round-trip must stay accepted: a function value
//! read back from a collection element is a bare `ptr` (its signature erased), and
//! re-attaching the expected call signature at a `fn(...)` slot is the D-0067
//! round-trip, not a mismatch (covered in `fnval_coercion.rs`). Because of that,
//! the check fires on a *scalar* source (`int`/`float`/`bool`/`char` — the issue
//! repro), whose word kind genuinely disagrees with a pointer slot; a
//! pointer-kind non-function (a `str`) is indistinguishable from an erased fn
//! value and is a documented limitation (`nonfn_pointer_value_is_a_known_limitation`).
//!
//! Sema is permissive about function types (`fn(...)` resolves to `Unknown`), so
//! the check lives in AST->ZIR lowering, where the interned signatures are known.
//! A `LowerError` is a real compile error reported before codegen.

use zapc_ir::LowerError;

fn lower_errors(src: &str) -> Vec<LowerError> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    let serrs = zapc_sema::check(&module);
    assert!(
        serrs.is_empty(),
        "sema is expected to accept the fn-typed-parameter misuse (it has no \
         fn-type model); got {serrs:?}"
    );
    zapc_ir::lower_module(&module).1
}

#[track_caller]
fn assert_rejected(src: &str, code: &str, fragments: &[&str]) {
    let errs = lower_errors(src);
    assert_eq!(
        errs.len(),
        1,
        "expected exactly one lowering error, got {errs:?}"
    );
    let e = &errs[0];
    assert_eq!(
        e.code.to_string(),
        code,
        "wrong code for message {:?}",
        e.message
    );
    for frag in fragments {
        assert!(
            e.message.contains(frag),
            "message {:?} is missing fragment {frag:?}",
            e.message
        );
    }
}

#[track_caller]
fn assert_accepted(src: &str) {
    let errs = lower_errors(src);
    assert!(errs.is_empty(), "expected clean lowering, got {errs:?}");
}

// ---------------------------------------------------------------------------
// #1104 repro: a non-function value passed to a `fn(...)` parameter.
// ---------------------------------------------------------------------------

#[test]
fn nonfn_literal_rejected() {
    // The issue's exact repro: `takes(7)` where `takes` wants `fn() -> int64`.
    let src = "def takes(fn() -> int64 f) -> int64:\n\
        \x20   return f()\n\n\
         def main():\n\
        \x20   print(takes(7))\n";
    assert_rejected(src, "ZAP4021", &["is not a function", "fn() -> i64"]);
}

#[test]
fn nonfn_typed_binding_rejected() {
    // A typed `int64 n = 7` is rejected too (not just a bare literal).
    let src = "def takes(fn() -> int64 f) -> int64:\n\
        \x20   return f()\n\n\
         def main():\n\
        \x20   int64 n = 7\n\
        \x20   print(takes(n))\n";
    assert_rejected(src, "ZAP4021", &["is not a function"]);
}

#[test]
fn nonfn_pointer_value_is_a_known_limitation() {
    // KNOWN LIMITATION (documented, not a regression): a `str` — like any
    // pointer-kind value — lowers to a bare `ptr`, which is exactly the
    // representation a function value read back from a collection element has
    // (its signature erased). The two are indistinguishable at the lowering
    // layer, so admitting the legitimate D-0067 round-trip
    // (`container_roundtrip_accepted`) means a `str` argument to a `fn(...)`
    // parameter still lowers. The soundness-relevant case the issue repro
    // targets — a *scalar* (an `int`), whose word kind genuinely disagrees with
    // a pointer slot — IS caught (`nonfn_literal_rejected`). This mirrors the
    // same `ptr` boundary #465 documents for signature mismatches.
    let src = "def takes(fn() -> int64 f) -> int64:\n\
        \x20   return f()\n\n\
         def main():\n\
        \x20   print(takes(\"x\"))\n";
    assert_accepted(src);
}

#[test]
fn nonfn_generic_rejected() {
    // The generic form from the issue: `def takes[V](fn() -> V f) -> V`, called
    // `takes(7)`, must be rejected as well.
    let src = "def takes[V](fn() -> V f) -> V:\n\
        \x20   fn() -> V g = f\n\
        \x20   return g()\n\n\
         def main():\n\
        \x20   int64 r = takes(7)\n\
        \x20   print(r)\n";
    assert_rejected(src, "ZAP4021", &["is not a function"]);
}

// ---------------------------------------------------------------------------
// Positive cases — these were legal before and must stay legal.
// ---------------------------------------------------------------------------

#[test]
fn named_fn_value_accepted() {
    let src = "def inc() -> int64:\n\
        \x20   return 1\n\n\
         def takes(fn() -> int64 f) -> int64:\n\
        \x20   return f()\n\n\
         def main():\n\
        \x20   print(takes(inc))\n";
    assert_accepted(src);
}

#[test]
fn lambda_accepted() {
    let src = "def takes(fn() -> int64 f) -> int64:\n\
        \x20   return f()\n\n\
         def main():\n\
        \x20   print(takes(lambda: 1))\n";
    assert_accepted(src);
}

#[test]
fn closure_accepted() {
    // A lambda capturing an enclosing local is still a legitimate fn value.
    let src = "def takes(fn() -> int64 f) -> int64:\n\
        \x20   return f()\n\n\
         def main():\n\
        \x20   int64 k = 41\n\
        \x20   print(takes(lambda: k + 1))\n";
    assert_accepted(src);
}

#[test]
fn generic_factory_accepted() {
    // The #972 `defaultdict`-shaped case: a generic `fn() -> V` factory with a
    // correctly-typed function argument must still lower.
    let src = "def makeInt() -> int64:\n\
        \x20   return 0\n\n\
         def takes[V](fn() -> V f) -> V:\n\
        \x20   fn() -> V g = f\n\
        \x20   return g()\n\n\
         def main():\n\
        \x20   int64 r = takes(makeInt)\n\
        \x20   print(r)\n";
    assert_accepted(src);
}

#[test]
fn container_roundtrip_accepted() {
    // A fn value stored in a list loses its signature (read back as a bare
    // `ptr`); re-attaching the expected call signature at the `fn(...)` parameter
    // is the legitimate D-0067 round-trip, which #1104 must not break.
    let src = "def inc() -> int64:\n\
        \x20   return 1\n\n\
         def takes(fn() -> int64 f) -> int64:\n\
        \x20   return f()\n\n\
         def main():\n\
        \x20   auto ops = [inc]\n\
        \x20   print(takes(ops[0]))\n";
    assert_accepted(src);
}

// ---------------------------------------------------------------------------
// Higher-order: a `fn(fn() -> int64) -> int64` parameter.
// ---------------------------------------------------------------------------

#[test]
fn higher_order_nonfn_rejected() {
    let src = "def apply2(fn(fn() -> int64) -> int64 f) -> int64:\n\
        \x20   return 0\n\n\
         def main():\n\
        \x20   print(apply2(7))\n";
    assert_rejected(src, "ZAP4021", &["is not a function"]);
}

#[test]
fn higher_order_fn_accepted() {
    // `apply2` wants a `fn(fn() -> int64) -> int64`; `callOne` is exactly that,
    // and inside `apply2` the inner `f(one)` passes a matching `fn() -> int64`.
    let src = "def one() -> int64:\n\
        \x20   return 1\n\n\
         def callOne(fn() -> int64 g) -> int64:\n\
        \x20   return g()\n\n\
         def apply2(fn(fn() -> int64) -> int64 f) -> int64:\n\
        \x20   return f(one)\n\n\
         def main():\n\
        \x20   print(apply2(callOne))\n";
    assert_accepted(src);
}
