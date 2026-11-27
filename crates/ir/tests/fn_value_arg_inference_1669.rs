//! #1669: a generic free-function call must infer its type parameter from a
//! bare callable reference passed as an argument — a named top-level function,
//! or a compiler-synthesized decorator callback — exactly as it already does
//! from a typed local binding.
//!
//! Before the fix, a bare function name used in value position lowered to a
//! `CTy::Unknown` (the monomorphizer only looked the name up as a *variable*,
//! never as a known function), so `accept(callback)` had nothing to unify the
//! parameter type `Inner` against and lowering failed with ZAP6001 ("cannot
//! infer type argument 'Inner' of function 'accept'; write it explicitly
//! (I#161)"). A typed local binding (`fn(Context, Namespace) handler = callback`
//! then `accept(handler)`) worked only because the binding carried the declared
//! function type. The monomorphizer now synthesizes the full callable signature
//! for a bare function reference, so the direct call infers identically.
//!
//! These tests run the real lowering pass (mono + AST→ZIR) **without** sema's
//! `TypeMap`, proving mono's own inferer closes the gap, so they guard the fix
//! wherever `cargo test` runs (host-only, no LLVM).

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_zir, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "expected clean lowering, got: {errs:?}");
}

fn lower_err_cannot_infer(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_zir, errs) = lower_module(&ast);
    assert!(
        errs.iter()
            .any(|e| e.message.contains("cannot infer type argument")),
        "expected the 'cannot infer type argument' ZAP6001, got: {errs:?}"
    );
}

const CTX: &str = "struct Context:\n\
    \x20   str name\n";

// --- The exact issue reproduction: a bare named function as a generic
//     free-function argument, inferring `Inner` from its full signature.

#[test]
fn bare_named_function_infers_generic_free_fn_arg() {
    lower_ok(&format!(
        "{CTX}\
        def accept[Inner](Inner previous):\n\
        \x20   print(\"called\")\n\
        def callback(Context ctx, Namespace ns):\n\
        \x20   print(ctx.name)\n\
        def main():\n\
        \x20   accept(callback)\n"
    ));
}

// --- The workaround that already worked must keep working (regression guard).

#[test]
fn typed_local_binding_still_infers() {
    lower_ok(&format!(
        "{CTX}\
        def accept[Inner](Inner previous):\n\
        \x20   print(\"called\")\n\
        def callback(Context ctx, Namespace ns):\n\
        \x20   print(ctx.name)\n\
        def main():\n\
        \x20   fn(Context, Namespace) handler = callback\n\
        \x20   accept(handler)\n"
    ));
}

// --- Inference must use the *complete* callable signature, including an
//     explicit return type, not just the parameter list.

#[test]
fn bare_function_with_return_type_infers() {
    lower_ok(&format!(
        "{CTX}\
        def accept[Inner](Inner previous) -> Inner:\n\
        \x20   return previous\n\
        def make(Context ctx) -> int64:\n\
        \x20   return 1\n\
        def main():\n\
        \x20   auto f = accept(make)\n\
        \x20   print(f(Context(name=\"x\")))\n"
    ));
}

// --- A no-parameter function reference infers too (empty signature).

#[test]
fn bare_zero_arg_function_infers() {
    lower_ok(
        "def accept[Inner](Inner previous):\n\
        \x20   print(\"called\")\n\
        def tick():\n\
        \x20   print(\"tick\")\n\
        def main():\n\
        \x20   accept(tick)\n",
    );
}

// --- The inferred type argument must flow through the return-type sink: the
//     callback becomes the generic's return value and is callable at its exact
//     signature (no fn-coercion mismatch).

#[test]
fn inferred_callable_is_returned_and_called() {
    lower_ok(&format!(
        "{CTX}\
        def wrap[Inner](Inner previous) -> Inner:\n\
        \x20   return previous\n\
        def handler(Context ctx, Namespace ns):\n\
        \x20   print(ctx.name)\n\
        def main():\n\
        \x20   auto h = wrap(handler)\n\
        \x20   h(Context(name=\"x\"), Namespace())\n"
    ));
}

// --- A bare function reference passed to a *second* positional parameter (the
//     `prepare(str, Inner)` shape from the sibling #1668 repro) still infers.

#[test]
fn bare_function_as_non_first_arg_infers() {
    lower_ok(&format!(
        "{CTX}\
        def prepare[Inner](str label, Inner previous):\n\
        \x20   print(label)\n\
        def callback(Context ctx, Namespace ns):\n\
        \x20   print(ctx.name)\n\
        def main():\n\
        \x20   prepare(\"outer\", callback)\n"
    ));
}

// --- Fail-closed: a genuinely un-pinnable parameter still reports ZAP6001, so
//     the synthesized-signature path does not mask real inference failures.

#[test]
fn unrelated_unpinnable_parameter_still_reports_zap6001() {
    lower_err_cannot_infer(
        "def accept[Inner, Extra](Inner previous):\n\
        \x20   print(\"called\")\n\
        def callback():\n\
        \x20   print(\"cb\")\n\
        def main():\n\
        \x20   accept(callback)\n",
    );
}

// --- A local variable must still shadow a same-named function: the binding's
//     type wins over the synthesized function signature.

#[test]
fn local_binding_shadows_function_name() {
    lower_ok(
        "def accept[Inner](Inner previous) -> Inner:\n\
        \x20   return previous\n\
        def value() -> int64:\n\
        \x20   return 0\n\
        def main():\n\
        \x20   int64 value = 7\n\
        \x20   auto r = accept(value)\n\
        \x20   print(r + 1)\n",
    );
}
