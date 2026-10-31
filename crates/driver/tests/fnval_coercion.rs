//! A named function (or any `fn(...)` value) coerced to a structurally
//! **incompatible** `fn(...)` type must be rejected with a precise type error
//! (I#465, D-0067).
//!
//! First-class function values are monomorphic: the `<fn>$fnval` thunk is
//! emitted to the source function's own signature, while the indirect call site
//! uses the *expected* `fn`-type. If the two disagree on arity, a parameter
//! type, or the return type, they miscompile under codegen. Sema is permissive
//! about function types (a `fn(...)` annotation resolves to the universally
//! assignable `Unknown`), so the check lives in AST→ZIR lowering, where the real
//! interned signatures are known — the same stage that already rejects a lambda
//! of the wrong arity or `None` bound to a value struct.
//!
//! These assert the stable `ZAPnnnn` code *and* the message, exercised through
//! the real front-end + lowering pipeline. They run on the host (no LLVM); the
//! end-to-end run of the legitimate case is pinned by the `fnval_*` goldens.

use zapc_ir::LowerError;

/// The three handlers used by the repro, each structurally incompatible with
/// the `fn(Req) -> Resp` parameter of `call_it`, plus `call_it`/`main`. `which`
/// selects the handler passed at the (single) call site.
fn program(handler: &str) -> String {
    format!(
        "struct Req:\n    str path\n\
         struct Resp:\n    int64 status\n\n\
         def h_arity() -> Resp:\n    return Resp(1)\n\
         def h_ptype(str s) -> Resp:\n    return Resp(len(s))\n\
         def h_rtype(Req r) -> int64:\n    return 7\n\
         def good(Req r) -> Resp:\n    return Resp(200)\n\n\
         def call_it(fn(Req) -> Resp h) -> Resp:\n    return h(Req(\"/\"))\n\n\
         def main():\n    print(str(call_it({handler}).status))\n"
    )
}

/// Run the front-end (parse + sema) then AST→ZIR lowering, returning the
/// lowering errors. Asserts the program lexes, parses, **and type-checks
/// cleanly** — the whole point of #465 is that sema accepts the mismatch, so the
/// soundness backstop must be the lowering stage.
fn lower_errors(src: &str) -> Vec<LowerError> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    let serrs = zapc_sema::check(&module);
    assert!(
        serrs.is_empty(),
        "sema is expected to accept the fn-value coercion (it has no fn-type \
         model); got {serrs:?}"
    );
    zapc_ir::lower_module(&module).1
}

/// Assert that lowering `src` produces exactly one error with the given code and
/// a message containing each expected fragment.
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

#[test]
fn arity_mismatch_rejected() {
    // `fn() -> Resp` passed where `fn(Req) -> Resp` is expected: the thunk would
    // drop the argument.
    assert_rejected(
        &program("h_arity"),
        "ZAP4021",
        &[
            "fn() -> Resp",
            "fn(Req) -> Resp",
            "takes 0 argument(s)",
            "takes 1",
        ],
    );
}

#[test]
fn parameter_type_mismatch_rejected() {
    // `fn(str) -> Resp` passed where `fn(Req) -> Resp` is expected: the thunk
    // would receive a `Req` where it reads a `str`.
    assert_rejected(
        &program("h_ptype"),
        "ZAP4021",
        &["is not compatible", "parameter 1", "Req"],
    );
}

#[test]
fn return_type_mismatch_rejected() {
    // `fn(Req) -> int64` passed where `fn(Req) -> Resp` is expected: the caller
    // would read a scalar `i64` as a `Resp` struct.
    assert_rejected(
        &program("h_rtype"),
        "ZAP4021",
        &["return type", "i64", "Resp"],
    );
}

#[test]
fn compatible_handler_accepted() {
    // The structurally matching handler lowers with no error.
    let errs = lower_errors(&program("good"));
    assert!(
        errs.is_empty(),
        "compatible coercion should lower: {errs:?}"
    );
}

#[test]
fn lambda_still_accepted() {
    // A `lambda` takes its parameter types from the expected signature, so it is
    // always compatible by construction (D-0067) — the check must not regress
    // this legitimate first-class-fn use.
    let src = "def apply(fn(int64) -> int64 f, int64 x) -> int64:\n    return f(x)\n\n\
               def main():\n    print(apply(lambda n: n + 1, 10))\n";
    let errs = lower_errors(src);
    assert!(errs.is_empty(), "lambda coercion should lower: {errs:?}");
}

#[test]
fn container_roundtrip_accepted() {
    // A function value stored in a `list` element loses its signature (elements
    // are uniform reference words, read back as a plain `ptr`); re-attaching the
    // expected call signature at the `fn(...)`-annotated binding is the
    // legitimate D-0067 round-trip, NOT a mismatch. The check must only fire on
    // a source whose exact signature is known to disagree — this mirrors the
    // `fnval_store` golden and guards the check against over-rejecting (the
    // first cut rejected this as "a value of type `ptr` is not a function").
    let src = "def inc(int64 x) -> int64:\n    return x + 1\n\n\
               def main():\n    auto ops = [inc]\n    \
               fn(int64) -> int64 g = ops[0]\n    print(g(41))\n";
    let errs = lower_errors(src);
    assert!(
        errs.is_empty(),
        "a `ptr` collection element re-typed to `fn(...)` should lower: {errs:?}"
    );
}

#[test]
fn incompatible_let_binding_rejected() {
    // The coercion funnel also guards `fn(...)`-annotated `let` bindings
    // (assignment), reported as a type mismatch in assignment.
    let src = "def h() -> int64:\n    return 1\n\n\
               def main():\n    fn(int64) -> int64 f = h\n    print(f(0))\n";
    // Scalar types render in their ZIR spelling (`int64` -> `i64`).
    assert_rejected(src, "ZAP4001", &["is not compatible", "fn(i64) -> i64"]);
}

#[test]
fn incompatible_return_rejected() {
    // ...and `return`ing a wrong-shaped function value, reported as a return
    // type mismatch. (`g` is `fn(str) -> int64`, incompatible with the declared
    // `fn(int64) -> int64` return type.)
    let src = "def g(str s) -> int64:\n    return 1\n\n\
               def make() -> fn(int64) -> int64:\n    return g\n\n\
               def main():\n    print(0)\n";
    assert_rejected(src, "ZAP4003", &["is not compatible", "parameter 1"]);
}
