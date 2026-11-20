//! Generic type-argument inference from a *binary* argument expression (#1128).
//!
//! Monomorphization (`zapc_mono`) runs a small best-effort type inferer over a
//! generic constructor's / generic call's argument expressions and unifies the
//! inferred types against the generic's type parameters (I#154). Sibling of
//! #1117 (the unary arm): the inferer typed a bare integer literal `2` as
//! `int64`, but it typed *every* binary expression — including the integer
//! subtraction `2 - 5` — as "unknown". So `Optional.Some(2 - 5)` could not pin
//! `T` and bailed with `ZAP6001` ("cannot infer type argument"), while the
//! bare `Optional.Some(2)` compiled fine. An untyped `auto count = 2 - 5`
//! binding then carried that "unknown" into every later use, so the issue's
//! `Optional.Some(count)` failed the same way.
//!
//! The fix types a binary expression in the inferer, mirroring sema's binary
//! result-type rules (`arith_result` / `sema_types::num_join`): arithmetic
//! promotes through the numeric tower (`2 - 5` → `int64`, `2 + 1.0` → `float64`,
//! `6 / 2` → `float64` by true division, D-0024); `+` also concatenates two
//! equal sequence types (`str`/`bytes`/`list`); bitwise/shift operators yield
//! `int64` (or `uint64` with an unsigned operand, #552); and the comparison,
//! logical, membership and identity operators are `bool`. An operand the
//! inferer cannot type stays "unknown", so no previously-inferring site changes
//! behaviour and a genuinely unresolvable parameter is still rejected.
//!
//! These are host-only lowering checks (no LLVM): `lower_module` runs the mono
//! pass first, so a `ZAP6001` from inference surfaces here. The runtime *value*
//! and *inferred-type* round-trips (that `2 - 5` is `-3` as an int, that
//! `6 / 2` is `3.0` as a float, etc.) are pinned by the `binary_infer_optional`
//! golden, which executes on CI.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting no lex/parse/lower errors (the program is
/// accepted by the whole frontend, monomorphization included).
fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
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

// ----- The issue's minimal reproduction -----

#[test]
fn optional_some_subtraction_via_auto_binding_infers() {
    // The exact program from #1128: the untyped `auto` binding must carry the
    // subtraction's `int64` type into the later generic constructor.
    lower_ok(
        "def main():\n\
        \x20   auto count = 2 - 5\n\
        \x20   auto value = Optional.Some(count)\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn optional_some_subtraction_direct_infers() {
    // The subtraction passed straight to the constructor (no binding).
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(2 - 5)\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn optional_some_bare_int_still_infers() {
    // Guard the baseline the issue says already worked, so the fix cannot
    // regress the plain-literal path it is modelled on.
    lower_ok(
        "def main():\n\
        \x20   auto count = 3\n\
        \x20   auto value = Optional.Some(count)\n\
        \x20   print(value.is_some())\n",
    );
}

// ----- Integer arithmetic operators all pin int64 -----

#[test]
fn optional_integer_addition_infers() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(2 + 3)\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn optional_integer_multiplication_infers() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(2 * 3)\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn optional_integer_floordiv_infers() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(7 // 2)\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn optional_integer_modulo_infers() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(7 % 3)\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn optional_integer_power_infers() {
    // `**` joins two ints to `int64` (Zap integer power, not Python's float),
    // matching sema's `num_join`.
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(2 ** 3)\n\
        \x20   print(value.is_some())\n",
    );
}

// ----- Float and mixed arithmetic -----

#[test]
fn optional_float_addition_infers() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(2.5 + 1.0)\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn optional_mixed_int_float_addition_infers() {
    // `int + float` promotes to `float64` (num_join), so `T = float64`.
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(3 + 0.5)\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn optional_true_division_infers() {
    // True division (D-0024) always yields `float64`, even for two ints; the
    // `3.0` value this produces is pinned by the golden.
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(6 / 2)\n\
        \x20   print(value.is_some())\n",
    );
}

// ----- Bitwise and shift operators pin int64 -----

#[test]
fn optional_bitand_infers() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(6 & 3)\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn optional_bitor_infers() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(6 | 1)\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn optional_bitxor_infers() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(6 ^ 3)\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn optional_shift_left_infers() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(1 << 4)\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn optional_shift_right_infers() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(256 >> 2)\n\
        \x20   print(value.is_some())\n",
    );
}

// ----- Comparison, logical, membership operators are bool -----

#[test]
fn optional_comparison_infers_bool() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(2 < 5)\n\
        \x20   print(value.unwrap())\n",
    );
}

#[test]
fn optional_equality_infers_bool() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(2 == 2)\n\
        \x20   print(value.unwrap())\n",
    );
}

#[test]
fn optional_logical_and_infers_bool() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(True and False)\n\
        \x20   print(value.unwrap())\n",
    );
}

#[test]
fn optional_logical_or_infers_bool() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(False or True)\n\
        \x20   print(value.unwrap())\n",
    );
}

#[test]
fn optional_membership_infers_bool() {
    // `in` is always `bool`; the list literal operand is irrelevant to the
    // result type.
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(3 in [1, 2, 3])\n\
        \x20   print(value.unwrap())\n",
    );
}

// ----- `+` as sequence concatenation -----

#[test]
fn optional_string_concatenation_infers_str() {
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some(\"a\" + \"b\")\n\
        \x20   print(value.unwrap())\n",
    );
}

#[test]
fn optional_list_concatenation_infers_list() {
    // `list[int64] + list[int64]` → `list[int64]`, so `T = list[int64]`.
    lower_ok(
        "def main():\n\
        \x20   auto value = Optional.Some([1] + [2])\n\
        \x20   print(value.is_some())\n",
    );
}

// ----- The fix is general, not Optional-only -----

#[test]
fn user_generic_struct_subtraction_infers() {
    // A user generic constructor exercises the *same* inferer; a subtraction
    // must pin `T` here too.
    lower_ok(
        "struct Box[T]:\n\
        \x20   T item\n\
        def main():\n\
        \x20   auto b = Box(2 - 5)\n\
        \x20   print(b.item)\n",
    );
}

#[test]
fn generic_fn_subtraction_infers() {
    // A generic *free function*'s own type parameter (I#161), inferred from a
    // binary-expression argument.
    lower_ok(
        "def wrap[T](T x) -> Optional[T]:\n\
        \x20   return Optional.Some(x)\n\
        def main():\n\
        \x20   print(wrap(2 - 5).unwrap())\n",
    );
}

// ----- Nested constructor as a method argument (positional + keyword) -----

#[test]
fn subtraction_in_positional_method_arg() {
    lower_ok(
        "struct Sampler:\n\
        \x20   int seed\n\
        \x20   def pick(self, Optional[int64] count = None) -> bool:\n\
        \x20       return count.is_some()\n\
        def main():\n\
        \x20   auto s = Sampler(0)\n\
        \x20   print(s.pick(Optional.Some(2 - 5)))\n",
    );
}

#[test]
fn subtraction_in_keyword_method_arg() {
    // The Redis `srandmember(key, count=...)` shape with an arithmetic count.
    lower_ok(
        "struct Sampler:\n\
        \x20   int seed\n\
        \x20   def pick(self, Optional[int64] count = None) -> bool:\n\
        \x20       return count.is_some()\n\
        def main():\n\
        \x20   auto s = Sampler(0)\n\
        \x20   print(s.pick(count=Optional.Some(2 - 5)))\n",
    );
}

// ----- auto/let binding, typed binding and return position -----

#[test]
fn subtraction_in_typed_binding() {
    lower_ok(
        "def main():\n\
        \x20   Optional[int64] option = Optional.Some(2 - 5)\n\
        \x20   print(option.is_some())\n",
    );
}

#[test]
fn subtraction_in_return_position() {
    lower_ok(
        "def diff() -> Optional[int64]:\n\
        \x20   return Optional.Some(2 - 5)\n\
        def main():\n\
        \x20   print(diff().unwrap())\n",
    );
}

#[test]
fn division_result_binding_reused_infers_float() {
    // The `auto`-binding propagation path (the issue's shape) with true
    // division: the binding must carry `float64`, not the integer operand type.
    lower_ok(
        "def main():\n\
        \x20   auto ratio = 6 / 2\n\
        \x20   auto value = Optional.Some(ratio)\n\
        \x20   print(value.is_some())\n",
    );
}

// ----- Don't over-accept: a genuinely unresolvable parameter still fails -----

#[test]
fn binary_arg_does_not_mask_unresolvable_param() {
    // `Pair[A, B]` with only `a = 2 - 5` given: the subtraction pins `A` exactly
    // as a bare literal would, but `B` is still unconstrained, so inference must
    // *still* fail — and specifically on `B`, proving `A` was resolved from the
    // binary expression rather than the whole call being rejected on `A`.
    lower_err_contains(
        "struct Pair[A, B]:\n\
        \x20   A a\n\
        \x20   B b\n\
        def main():\n\
        \x20   auto p = Pair(a=2 - 5)\n\
        \x20   print(p.a)\n",
        "type argument 'B'",
    );
}

#[test]
fn unmodeled_operand_keeps_binary_unknown() {
    // Fail-closed boundary (the companion #1132): a built-in call's return type
    // is not modelled by the inferer, so `len(xs)` is "unknown"; the fix must
    // not invent a type for `len(xs) - 5`, so this still fails to infer rather
    // than mis-inferring. (When #1132 teaches the inferer built-in returns, this
    // becomes an `lower_ok` — but until then the boundary is pinned here.)
    lower_err_contains(
        "def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto value = Optional.Some(len(xs) - 5)\n\
        \x20   print(value.is_some())\n",
        "cannot infer type argument",
    );
}
