//! A generic parameter's default must fit the type argument it is used with
//! (#1705).
//!
//! When a generic call *omits* a defaulted parameter, the default is
//! materialized at the call site and passed into the specialization the call
//! selects. Monomorphization never checked that the default's type was
//! assignable to that specialization's (substituted) parameter type, so when the
//! type parameter was pinned by a source *other* than the default — an explicit
//! type argument (`setting[str]()`), the expected/return sink (`str s =
//! setting()`), or another argument (`pair("hi")`) — a `bool` default like
//! `False` was reinterpreted into a `str`/pointer slot with no diagnostic (a
//! type-unsound lowering).
//!
//! The fix checks the omitted default against the chosen parameter type with
//! `sema_types::assignable` (the same rule sema applies to every other
//! assignment) and rejects a genuine mismatch with `ZAP4001`. It is per *call*,
//! so a call that *supplies* the argument is unaffected (the default is never
//! used), and conservative/fail-open: an untyped numeric literal still fits any
//! numeric target, and a user/container parameter type or a non-literal default
//! is left untouched. This is distinct from #1704 (inferring `T` *from* an
//! omitted default), whose own specialization is always sound because there `T`
//! equals the default's type.
//!
//! Host-only lowering checks (no LLVM): `lower_module` runs the mono pass, so a
//! `ZAP4001` from the check surfaces here.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
}

/// Lowering must fail; the joined diagnostics must mention `ZAP4001` and the
/// offending parameter name.
fn lower_rejects(src: &str, param: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(!errs.is_empty(), "expected a lowering error, got none");
    let joined = errs
        .iter()
        .map(|e| format!("{:?}: {}", e.code, e.message))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("ZAP4001") && joined.contains(param),
        "expected a ZAP4001 mentioning parameter {param:?}, got:\n{joined}"
    );
}

// ----- The three ways a type parameter is pinned against an incompatible default -----

#[test]
fn explicit_type_arg_with_incompatible_default_is_rejected() {
    // `setting[str]()` pins `T = str`; the omitted `bool` default `False` cannot
    // be assigned to `str` — previously lowered `bconst false` into a `ptr` slot.
    lower_rejects(
        "def setting[T](T value=False) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   auto x = setting[str]()\n\
        \x20   print(x)\n",
        "value",
    );
}

#[test]
fn return_sink_with_incompatible_default_is_rejected() {
    // The binding's `str` drives `T = str` through the return sink; the omitted
    // `int` default is unassignable to `str`.
    lower_rejects(
        "def f[T](T value=0) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   str s = f()\n\
        \x20   print(s)\n",
        "value",
    );
}

#[test]
fn sibling_argument_with_incompatible_default_is_rejected() {
    // `pair("hi")` pins `T = str` from `a`; the omitted sibling `b`'s `int`
    // default is unassignable to `str`.
    lower_rejects(
        "def pair[T](T a, T b=0) -> T:\n\
        \x20   return a\n\
        def main():\n\
        \x20   print(pair(\"hi\"))\n",
        "b",
    );
}

#[test]
fn incompatible_default_on_generic_method_is_rejected() {
    lower_rejects(
        "struct Holder:\n\
        \x20   int seed\n\
        \x20   def choose[T](self, T value=False) -> T:\n\
        \x20       return value\n\
        def main():\n\
        \x20   auto h = Holder(0)\n\
        \x20   auto v = h.choose[str]()\n\
        \x20   print(v)\n",
        "value",
    );
}

#[test]
fn bytes_default_into_str_specialization_is_rejected() {
    // `bytes` and `str` are distinct reference types; a `bytes` default must not
    // be reinterpreted as a `str`.
    lower_rejects(
        "def f[T](T value=b\"x\") -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   auto v = f[str]()\n\
        \x20   print(v)\n",
        "value",
    );
}

// ----- A supplied argument means the default is never used: not checked -----

#[test]
fn supplying_the_argument_bypasses_the_check() {
    // Even though the `bool` default would not fit `str`, `setting(value="x")`
    // supplies the argument, so the default is never materialized and the call
    // is fine (the specialization is `setting$str` with a dead default).
    lower_ok(
        "def setting[T](T value=False) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   print(setting(value=\"x\"))\n",
    );
}

// ----- No false positives: valid defaults still lower -----

#[test]
fn untyped_int_default_fits_every_numeric_specialization() {
    // An untyped integer literal default flows into any numeric type argument,
    // exactly as `int8 x = 0` / `float64 y = 0` do — never a false positive.
    for ty in [
        "int8", "int16", "int32", "int64", "uint8", "float64", "bigint", "complex",
    ] {
        lower_ok(&format!(
            "def f[T](T value=0) -> T:\n\
            \x20   return value\n\
            def main():\n\
            \x20   auto v = f[{ty}]()\n\
            \x20   print(v)\n"
        ));
    }
}

#[test]
fn untyped_float_default_fits_float_specializations() {
    for ty in ["float32", "float64", "complex"] {
        lower_ok(&format!(
            "def f[T](T value=1.5) -> T:\n\
            \x20   return value\n\
            def main():\n\
            \x20   auto v = f[{ty}]()\n\
            \x20   print(v)\n"
        ));
    }
}

#[test]
fn matching_scalar_default_lowers() {
    // The default's type equals the chosen type argument — always assignable.
    lower_ok(
        "def f[T](T value=False) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   auto v = f[bool]()\n\
        \x20   print(v)\n",
    );
}

#[test]
fn signed_widening_default_is_accepted() {
    // The inferred `T` equals the default's own type, and a narrow-int default
    // widens implicitly up the signed tower, so an omitted-default call that
    // pins `T` from the default stays sound.
    lower_ok(
        "def f[T](T value=-1) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   print(f())\n",
    );
}

#[test]
fn container_parameter_default_is_not_checked() {
    // A non-primitive (container) parameter type is outside the fail-open check;
    // with an explicit type argument the call lowers unchanged.
    lower_ok(
        "def f[T](T value=[1]) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   auto v = f[list[int64]]()\n\
        \x20   print(v)\n",
    );
}

#[test]
fn non_generic_function_with_scalar_default_still_lowers() {
    // The check also runs on non-generic specializations, but must agree with
    // sema there: a valid non-generic default (sema already accepted it) lowers.
    lower_ok(
        "def f(str name=\"zap\", int count=3) -> str:\n\
        \x20   return name\n\
        def main():\n\
        \x20   print(f())\n",
    );
}
