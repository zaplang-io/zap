//! #1348: calling a first-class function value read back from a *typed
//! collection* must lower to an indirect closure call, not a top-level
//! name lookup.
//!
//! A `fn(..) -> R` value (I#163) is physically a single closure-record pointer
//! word. When stored in a `list`/`dict`/`set`/channel or a collection-typed
//! struct field, the element kind used to collapse to a bare `Elem::Ptr`, so a
//! value read back by **iteration** (`for cb in cbs`) or by **indexing** into an
//! unannotated binding (`auto cb = cbs[0]`) came back as `Ty::Ptr`. The call
//! site then could not tell the value was callable and fell back to resolving
//! the name as a top-level function, failing with
//! `ZAP6001 "call to unknown function"`.
//!
//! The fix adds a dedicated `Elem::Func(sig)` (mirroring `Elem::Nested`/`Bytes`/
//! `Complex`) so read-back recovers the exact `Ty::Func`, and the call lowers to
//! a `CallClosure` (indirect call through the closure record). An *annotated*
//! read-back (`fn(..) -> R g = cbs[0]`) already worked because the annotation
//! re-attached the signature; these tests cover the forms that did **not**.
//!
//! The tests run the real AST→ZIR lowering pass (`lower_module`) **without**
//! sema's `TypeMap`, so they guard the fix host-only wherever `cargo test` runs
//! (no LLVM). End-to-end execution — that each call actually returns 42 / the
//! observation list — is covered by the `fnvalue_from_list_1348` golden fixture.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

/// Lower `src` (no sema types); return the rendered ZIR, asserting a clean
/// parse and clean lowering.
fn lower_ok(src: &str) -> String {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (zir, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "expected clean lowering, got: {errs:?}");
    format!("{zir}")
}

/// Assert `src` is rejected at lowering with a ZAP6001 "call to unknown
/// function" — the pre-fix failure, used as a fail-closed guard.
fn lower_err_unknown_call(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_zir, errs) = lower_module(&ast);
    assert!(
        errs.iter()
            .any(|e| e.message.contains("call to unknown function")),
        "expected a ZAP6001 'call to unknown function', got: {errs:?}"
    );
}

/// The number of indirect closure calls (`callclosure`) in the rendered ZIR —
/// proof the call lowered to an indirect call through the closure record rather
/// than a direct/static call to a named function.
fn callclosure_count(zir: &str) -> usize {
    zir.matches("callclosure").count()
}

const SAMPLE: &str = "def sample() -> int64:\n\
    \x20   return 42\n";

// --- The issue's exact reduction: a local `list[fn() -> int64]`, iterated. ---

#[test]
fn local_list_iterated_call_lowers_indirect() {
    let zir = lower_ok(&format!(
        "{SAMPLE}\
def main():\n\
    \x20   list[fn() -> int64] callbacks = [sample]\n\
    \x20   for callback in callbacks:\n\
    \x20       assert callback() == 42\n"
    ));
    assert!(
        callclosure_count(&zir) >= 1,
        "the loop-variable call must lower to a CallClosure, got:\n{zir}"
    );
}

// --- Indexed read-back, both directly on the index and via an `auto` binding
//     (the two forms the issue calls out; neither re-attaches the signature). ---

#[test]
fn indexed_call_direct_lowers_indirect() {
    let zir = lower_ok(&format!(
        "{SAMPLE}\
def main():\n\
    \x20   list[fn() -> int64] callbacks = [sample]\n\
    \x20   assert callbacks[0]() == 42\n"
    ));
    assert!(callclosure_count(&zir) >= 1, "got:\n{zir}");
}

#[test]
fn indexed_call_via_auto_binding_lowers_indirect() {
    let zir = lower_ok(&format!(
        "{SAMPLE}\
def main():\n\
    \x20   list[fn() -> int64] callbacks = [sample]\n\
    \x20   auto callback = callbacks[0]\n\
    \x20   assert callback() == 42\n"
    ));
    assert!(callclosure_count(&zir) >= 1, "got:\n{zir}");
}

// --- The list held in a struct FIELD (iterated and indexed). ---

#[test]
fn struct_field_list_iterated_call_lowers_indirect() {
    let zir = lower_ok(&format!(
        "{SAMPLE}\
struct Registry:\n\
    \x20   list[fn() -> int64] callbacks\n\
def main():\n\
    \x20   Registry reg = Registry([sample])\n\
    \x20   for callback in reg.callbacks:\n\
    \x20       assert callback() == 42\n"
    ));
    assert!(callclosure_count(&zir) >= 1, "got:\n{zir}");
}

#[test]
fn struct_field_list_indexed_call_lowers_indirect() {
    let zir = lower_ok(&format!(
        "{SAMPLE}\
struct Registry:\n\
    \x20   list[fn() -> int64] callbacks\n\
def main():\n\
    \x20   Registry reg = Registry([sample])\n\
    \x20   assert reg.callbacks[0]() == 42\n"
    ));
    assert!(callclosure_count(&zir) >= 1, "got:\n{zir}");
}

// --- A callable WITH PARAMETERS read back from a list. ---

const DBL: &str = "def dbl(int64 x) -> int64:\n\
    \x20   return x * 2\n";

#[test]
fn parameterised_callable_from_list_lowers_indirect() {
    let zir = lower_ok(&format!(
        "{DBL}\
def main():\n\
    \x20   list[fn(int64) -> int64] fns = [dbl]\n\
    \x20   for f in fns:\n\
    \x20       assert f(21) == 42\n\
    \x20   assert fns[0](21) == 42\n"
    ));
    assert!(
        callclosure_count(&zir) >= 2,
        "both the loop and indexed calls must be indirect, got:\n{zir}"
    );
}

// --- A callable returning a NON-SCALAR (the otel #3 shape): a list of
//     callbacks each returning a `list[int64]` of observations. ---

const OBS: &str = "def observe() -> list[int64]:\n\
    \x20   return [1, 2, 3]\n";

#[test]
fn nonscalar_result_callable_from_local_list_lowers_indirect() {
    let zir = lower_ok(&format!(
        "{OBS}\
def main():\n\
    \x20   list[fn() -> list[int64]] gauges = [observe]\n\
    \x20   for g in gauges:\n\
    \x20       assert g() == [1, 2, 3]\n\
    \x20   assert gauges[0]() == [1, 2, 3]\n"
    ));
    assert!(callclosure_count(&zir) >= 2, "got:\n{zir}");
}

#[test]
fn otel_observable_gauge_field_shape_lowers_indirect() {
    // The otel #3 shape: a meter registers a `list` of observable-gauge
    // callbacks (a struct field), each returning a `list` of observations.
    let zir = lower_ok(&format!(
        "{OBS}\
struct Meter:\n\
    \x20   list[fn() -> list[int64]] gauges\n\
def main():\n\
    \x20   Meter m = Meter([observe])\n\
    \x20   for g in m.gauges:\n\
    \x20       assert g() == [1, 2, 3]\n\
    \x20   assert m.gauges[0]() == [1, 2, 3]\n"
    ));
    assert!(callclosure_count(&zir) >= 2, "got:\n{zir}");
}

// --- A `dict` VALUE of function values, read back and called (the element-type
//     fix is central: every value-position collection recovers `Ty::Func`). ---

#[test]
fn dict_value_function_called_lowers_indirect() {
    let zir = lower_ok(&format!(
        "{SAMPLE}\
def main():\n\
    \x20   dict[str, fn() -> int64] handlers = {{\"a\": sample}}\n\
    \x20   assert handlers[\"a\"]() == 42\n"
    ));
    assert!(callclosure_count(&zir) >= 1, "got:\n{zir}");
}

// --- Regression guard: a function value held DIRECTLY in a variable still
//     lowers to an indirect call (the I#163 first-class-fn path), unchanged. ---

#[test]
fn direct_variable_call_still_indirect() {
    let zir = lower_ok(&format!(
        "{SAMPLE}\
def main():\n\
    \x20   fn() -> int64 cb = sample\n\
    \x20   assert cb() == 42\n"
    ));
    assert!(
        callclosure_count(&zir) >= 1,
        "the direct first-class-fn call must stay indirect, got:\n{zir}"
    );
}

// --- Fail-closed guard: calling a genuinely unknown name is still a ZAP6001,
//     so the fix did not turn every bare call into a silent indirect call. ---

#[test]
fn unknown_name_call_still_rejected() {
    lower_err_unknown_call(
        "def main():\n\
    \x20   assert not_a_function() == 42\n",
    );
}
