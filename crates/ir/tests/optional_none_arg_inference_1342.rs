//! #1342: a payload-less generic-enum variant (`Optional.None`) passed as a
//! call argument or a struct-constructor field must infer its type argument
//! `T` from the callee's declared parameter / field type — the way `return`
//! already pins it from the function's return type.
//!
//! Before the fix, the monomorphizer rewrote every call argument with **no**
//! expected type, so a payload-less variant argument had nothing to infer `T`
//! from and lowering failed with ZAP6001 ("cannot infer type argument 'T' of
//! generic 'Optional'"), even though the parameter / field declared the exact
//! `Optional[...]` instance. A non-generic callee's argument is now rewritten
//! with the callee's parameter / field type as its expected-type sink.
//!
//! These tests run the real lowering pass (mono + AST→ZIR) **without** sema's
//! `TypeMap`, proving mono's own inferer closes the gap, so they guard the fix
//! wherever `cargo test` runs (host-only, no LLVM). Some/None execution is
//! covered end-to-end by the `optional_none_arg_inference_1342` golden fixture.

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

const R: &str = "struct R:\n\
    \x20   int64 value\n";

// --- The failing positions: Optional.None as a function argument / ctor field.

#[test]
fn optional_none_as_function_argument_lowers() {
    lower_ok(&format!(
        "{R}\
        def take(Optional[R] o) -> int64:\n\
        \x20   if o.is_some():\n\
        \x20       return o.unwrap().value\n\
        \x20   return -1\n\
        def main():\n\
        \x20   print(take(Optional.None))\n"
    ));
}

#[test]
fn optional_none_as_constructor_field_lowers() {
    lower_ok(&format!(
        "{R}\
        struct Box:\n\
        \x20   Optional[R] slot\n\
        def main():\n\
        \x20   Box b = Box(Optional.None)\n\
        \x20   print(b.slot.is_some())\n"
    ));
}

#[test]
fn optional_none_as_named_argument_lowers() {
    lower_ok(&format!(
        "{R}\
        def take(Optional[R] o) -> int64:\n\
        \x20   return -1\n\
        def main():\n\
        \x20   print(take(o=Optional.None))\n"
    ));
}

#[test]
fn optional_none_as_named_constructor_field_lowers() {
    lower_ok(&format!(
        "{R}\
        struct Box:\n\
        \x20   Optional[R] slot\n\
        def main():\n\
        \x20   Box b = Box(slot=Optional.None)\n\
        \x20   print(b.slot.is_some())\n"
    ));
}

#[test]
fn optional_none_as_non_first_argument_lowers() {
    // The expected type must track the argument's position, not just arg 0.
    lower_ok(&format!(
        "{R}\
        def take(int64 n, Optional[R] o) -> int64:\n\
        \x20   return n\n\
        def main():\n\
        \x20   print(take(1, Optional.None))\n"
    ));
}

#[test]
fn optional_none_as_non_first_constructor_field_lowers() {
    lower_ok(&format!(
        "{R}\
        struct Box:\n\
        \x20   int64 tag\n\
        \x20   Optional[R] slot\n\
        def main():\n\
        \x20   Box b = Box(1, Optional.None)\n\
        \x20   print(b.slot.is_some())\n"
    ));
}

// --- Generality: a user-defined generic enum's payload-less variant, same
//     positions. Not an `Optional`-only special case.

#[test]
fn user_generic_enum_empty_variant_argument_lowers() {
    lower_ok(
        "enum Maybe[T]:\n\
        \x20   Just(T value)\n\
        \x20   Nothing\n\
        def take(Maybe[int64] m) -> int64:\n\
        \x20   return 0\n\
        def main():\n\
        \x20   print(take(Maybe.Nothing))\n",
    );
}

// --- Regressions: the positions that already worked stay clean.

#[test]
fn optional_some_argument_still_lowers() {
    lower_ok(&format!(
        "{R}\
        def take(Optional[R] o) -> int64:\n\
        \x20   return -1\n\
        def main():\n\
        \x20   print(take(Optional.Some(R(1))))\n"
    ));
}

#[test]
fn optional_none_return_still_lowers() {
    lower_ok(&format!(
        "{R}\
        def f() -> Optional[R]:\n\
        \x20   return Optional.None\n\
        def main():\n\
        \x20   print(f().is_some())\n"
    ));
}

#[test]
fn bare_none_argument_and_field_still_lower() {
    lower_ok(&format!(
        "{R}\
        struct Box:\n\
        \x20   Optional[R] slot\n\
        def take(Optional[R] o) -> int64:\n\
        \x20   return -1\n\
        def main():\n\
        \x20   print(take(None))\n\
        \x20   Box b = Box(None)\n\
        \x20   print(b.slot.is_some())\n"
    ));
}

// --- Fail-closed: with no expected type at all, the diagnostic still fires.

#[test]
fn optional_none_without_expected_still_reports_zap6001() {
    // An `auto` binding to a bare `Optional.None` has no expected type to pin
    // `T`; this must still be rejected (not silently mis-instantiated).
    lower_err_cannot_infer(&format!(
        "{R}\
        def main():\n\
        \x20   auto x = Optional.None\n\
        \x20   print(x)\n"
    ));
}
