//! Generic type-argument inference from an *omitted* parameter's typed default
//! (#1704).
//!
//! Monomorphization (`zapc_mono`) infers a generic function's / method's type
//! arguments by unifying the types of the *supplied* call arguments (and, as a
//! fallback, the expected/return type) against the declared parameter/return
//! types (I#161). An omitted parameter contributed nothing, so a call that
//! relied entirely on a parameter's default to pin a type parameter bailed with
//! `ZAP6001` ("cannot infer type argument"):
//!
//! ```zap
//! def setting[T](T value=False) -> T:
//!     return value
//! setting()            # ZAP6001 before the fix — T never bound
//! setting(value=True)  # fine — T = bool from the argument
//! ```
//!
//! The fix adds an *omitted-default sink* to `infer_fn_targs`: after argument
//! unification and the return/expected sink leave a type parameter free, each
//! parameter that was omitted at the call site and carries a concretely typed
//! default contributes that default's type. The default is the lowest-
//! precedence source — a supplied argument (positional or keyword), an explicit
//! type argument, and the expected/return sink all win — so explicit value and
//! type arguments keep their normal precedence. A default that mono cannot type
//! statically (a call, a collection/constructor expression, an untyped name)
//! contributes nothing, so inference fails closed exactly as before.
//!
//! These are host-only lowering checks (no LLVM): `lower_module` runs the mono
//! pass first, so a `ZAP6001` from inference surfaces here, and the chosen
//! specialization is observable as a lowered function name (`setting$bool`,
//! `setting$str`, …). The runtime *value* round-trip (that an omitted call
//! actually returns the default value, and that explicit arguments win) is
//! pinned by the `omitted_default_infer` golden, which executes on CI.

use zapc_ir::lower_module;
use zapc_ir_core::Module;
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting no lex/parse/lower errors, and return the
/// lowered module so a test can assert over the chosen specializations.
fn lower_ok(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    module
}

/// Parse + lower `src`, asserting lowering fails with a message containing
/// `needle` (parse/lex must be clean so the failure is the lowering one we mean
/// to pin).
fn lower_err_contains(src: &str, needle: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(!errs.is_empty(), "expected a lowering error, got none");
    let joined = errs
        .iter()
        .map(|e| format!("{e:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains(needle),
        "expected a lowering error containing {needle:?}, got:\n{joined}"
    );
}

/// `true` when the lowered module defines a function whose name is exactly
/// `name` (the monomorphized specialization, e.g. `setting$bool`).
fn has_fn(m: &Module, name: &str) -> bool {
    m.funcs.iter().any(|f| f.name == name)
}

// ----- The issue's minimal reproduction -----

#[test]
fn omitted_bool_default_infers_bool() {
    // The exact program from #1704: the no-argument call must pin `T = bool`
    // from the default `False`, and the explicit keywords keep working.
    let m = lower_ok(
        "def setting[T](T value=False) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   assert not setting()\n\
        \x20   assert setting(value=True)\n\
        \x20   assert setting(value=\"message\") == \"message\"\n",
    );
    assert!(has_fn(&m, "setting$bool"), "expected setting$bool");
    assert!(has_fn(&m, "setting$str"), "expected setting$str");
}

#[test]
fn omitted_default_alone_infers() {
    // Nothing but the default is available: it alone must pin `T`.
    let m = lower_ok(
        "def setting[T](T value=False) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   auto v = setting()\n\
        \x20   print(v)\n",
    );
    assert!(has_fn(&m, "setting$bool"));
}

// ----- The scalar/unary/const default value forms -----

#[test]
fn omitted_int_default_infers_int64() {
    let m = lower_ok(
        "def f[T](T value=7) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   print(f())\n",
    );
    assert!(has_fn(&m, "f$int64"));
}

#[test]
fn omitted_negative_int_default_infers_int64() {
    // A negative literal parses as `Neg` over an int literal; the default typer
    // unwraps the sign, so `-1` types exactly as `1` does (mirrors #1117).
    let m = lower_ok(
        "def f[T](T value=-1) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   print(f())\n",
    );
    assert!(has_fn(&m, "f$int64"));
}

#[test]
fn omitted_float_default_infers_float64() {
    let m = lower_ok(
        "def f[T](T value=1.5) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   print(f())\n",
    );
    assert!(has_fn(&m, "f$float64"));
}

#[test]
fn omitted_negative_float_default_infers_float64() {
    let m = lower_ok(
        "def f[T](T value=-2.5) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   print(f())\n",
    );
    assert!(has_fn(&m, "f$float64"));
}

#[test]
fn omitted_imaginary_default_infers_complex() {
    let m = lower_ok(
        "def f[T](T value=2j) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   print(f())\n",
    );
    assert!(has_fn(&m, "f$complex"));
}

#[test]
fn omitted_str_default_infers_str() {
    let m = lower_ok(
        "def f[T](T value=\"hi\") -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   print(f())\n",
    );
    assert!(has_fn(&m, "f$str"));
}

#[test]
fn omitted_bytes_default_infers_bytes() {
    let m = lower_ok(
        "def f[T](T value=b\"hi\") -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   print(f())\n",
    );
    assert!(has_fn(&m, "f$bytes"));
}

#[test]
fn omitted_not_literal_default_infers_bool() {
    // `not x` is boolean regardless of operand, like the `rewrite_expr` unary
    // arm; a `value=not True` default therefore pins `T = bool`.
    let m = lower_ok(
        "def f[T](T value=not True) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   print(f())\n",
    );
    assert!(has_fn(&m, "f$bool"));
}

#[test]
fn omitted_const_global_default_infers_its_type() {
    // A default that names an explicitly typed module `const` (#661 / D-0112)
    // contributes that global's concrete type.
    let m = lower_ok(
        "const int64 DEF = 7\n\
        def f[T](T value=DEF) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   print(f())\n",
    );
    assert!(has_fn(&m, "f$int64"));
}

// ----- Precedence: an explicit argument / type always wins over the default -----

#[test]
fn supplied_positional_argument_overrides_default() {
    // `f(3)` binds the parameter positionally; `T` is `int64` from the argument,
    // never `bool` from the omitted default.
    let m = lower_ok(
        "def f[T](T value=False) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   print(f(3))\n",
    );
    assert!(has_fn(&m, "f$int64"));
    assert!(
        !has_fn(&m, "f$bool"),
        "the default must not win over an argument"
    );
}

#[test]
fn supplied_keyword_argument_overrides_default() {
    let m = lower_ok(
        "def f[T](T value=False) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   print(f(value=\"x\"))\n",
    );
    assert!(has_fn(&m, "f$str"));
    assert!(!has_fn(&m, "f$bool"));
}

#[test]
fn explicit_type_argument_overrides_default() {
    // `f[int64]()` pins `T` explicitly; the omitted `bool` default must not
    // produce a `bool` specialization.
    let m = lower_ok(
        "def f[T](T value=0) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   auto v = f[int64]()\n\
        \x20   print(v)\n",
    );
    assert!(has_fn(&m, "f$int64"));
    assert!(!has_fn(&m, "f$bool"));
}

#[test]
fn expected_return_sink_overrides_default() {
    // The return type `-> T` unifies with the binding's declared `str`, so `T`
    // is `str` from the expected sink — which runs before the default sink.
    let m = lower_ok(
        "def f[T](T value=0) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   str s = f()\n\
        \x20   print(s)\n",
    );
    assert!(has_fn(&m, "f$str"));
    assert!(!has_fn(&m, "f$int64"));
}

// ----- A second, supplied parameter pins T; the omitted default does not fight it -----

#[test]
fn argument_pins_t_even_when_another_param_has_a_default() {
    // `pair("hi")` binds `a`, so `T = str`; the omitted `b`'s `int` default
    // does not override the already-chosen specialization.
    let m = lower_ok(
        "def pair[T](T a, T b=0) -> T:\n\
        \x20   return a\n\
        def main():\n\
        \x20   print(pair(\"hi\"))\n",
    );
    assert!(has_fn(&m, "pair$str"));
    assert!(!has_fn(&m, "pair$int64"));
}

// ----- Methods carry their own type parameters (I#161) -----

#[test]
fn omitted_default_infers_on_generic_method() {
    let m = lower_ok(
        "struct Holder:\n\
        \x20   int seed\n\
        \x20   def pick[T](self, T value=False) -> T:\n\
        \x20       return value\n\
        def main():\n\
        \x20   auto h = Holder(0)\n\
        \x20   auto v = h.pick()\n\
        \x20   print(v)\n",
    );
    // The method specialization carries the owner and the inferred `bool`.
    assert!(
        m.funcs
            .iter()
            .any(|f| f.name.contains("pick") && f.name.contains("bool")),
        "expected a bool-specialized pick method, got: {:?}",
        m.funcs.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
}

// ----- Insufficient type information still fails closed (ZAP6001) -----

#[test]
fn non_defaulted_generic_still_cannot_infer() {
    // No argument, no default, no expected type: `T` is genuinely unbound, so
    // the call must still be diagnosed. The default sink changes nothing here.
    lower_err_contains(
        "def make[T]() -> T:\n\
        \x20   raise ValueError(\"unreachable\")\n\
        def main():\n\
        \x20   auto v = make()\n\
        \x20   print(v)\n",
        "cannot infer type argument 'T'",
    );
}

#[test]
fn compound_default_fails_closed() {
    // A collection-literal default is concretely typed but is deliberately not
    // modeled by the pure default typer (the inference family's documented
    // boundary, like GenExp in #1131). With no other source for `T`, the call
    // fails closed with the same diagnostic rather than mis-inferring.
    lower_err_contains(
        "def f[T](T value=[1]) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   auto v = f()\n\
        \x20   print(v)\n",
        "cannot infer type argument 'T'",
    );
}

#[test]
fn compound_default_with_explicit_type_still_lowers() {
    // The boundary is inference-only: when the type is supplied explicitly, the
    // (unmodeled) compound default is irrelevant and the call lowers fine.
    let m = lower_ok(
        "def f[T](T value=[1]) -> T:\n\
        \x20   return value\n\
        def main():\n\
        \x20   auto v = f[list[int64]]()\n\
        \x20   print(v)\n",
    );
    assert!(m.funcs.iter().any(|f| f.name.starts_with("f$")));
}
