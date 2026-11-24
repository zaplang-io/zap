//! Generic type-argument inference from a *unary* argument expression (#1117).
//!
//! Monomorphization (`zapc_mono`) runs a small best-effort type inferer over a
//! generic constructor's / generic call's argument expressions and unifies the
//! inferred types against the generic's type parameters (I#154). That inferer
//! typed a bare integer literal `4` as `int64` and a bare float literal `4.0`
//! as `float64`, but it typed *every* unary expression — including the
//! arithmetic negation `-4` that a negative literal actually parses to
//! (`UnOp::Neg` over `Expr::Int(4)`) — as "unknown". So `Optional.Some(-4)`
//! could not pin `T` and bailed with `ZAP6001` ("cannot infer type argument"),
//! while the sign-flipped `Optional.Some(4)` compiled fine.
//!
//! The fix types a unary expression in the inferer: `not x` is always `bool`,
//! while `-x` / `+x` / `~x` preserve the operand's inferred type. So a negated,
//! unary-plus, or bit-complemented literal infers exactly the type its
//! bare-literal counterpart does. The inferer is shared by every generic
//! inference site, so this is not an `Optional`-only patch: it equally fixes
//! user generic structs, nested positional/keyword method arguments, and so on.
//!
//! These are host-only lowering checks (no LLVM): `lower_module` runs the mono
//! pass first, so a `ZAP6001` from inference surfaces here. The runtime *value*
//! round-trip (that `-4` survives `unwrap()` as `-4`, not `4` or a type error)
//! is pinned by the `neg_literal_optional` golden, which executes on CI.

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
fn optional_some_negative_int_literal_infers() {
    // The exact program from #1117: inference must pin `T = int64` from `-4`.
    lower_ok(
        "def main():\n\
        \x20   auto option = Optional.Some(-4)\n\
        \x20   print(option.is_some())\n",
    );
}

#[test]
fn optional_some_positive_int_literal_still_infers() {
    // Guard the sign-flipped baseline the issue says already worked, so the fix
    // cannot regress the positive-literal path it is modelled on.
    lower_ok(
        "def main():\n\
        \x20   auto option = Optional.Some(4)\n\
        \x20   print(option.is_some())\n",
    );
}

#[test]
fn optional_some_negative_value_round_trips_through_unwrap() {
    // The typed `unwrap()` path lowers end-to-end; the actual `-4` value is
    // asserted by the `neg_literal_optional` golden on CI.
    lower_ok(
        "def main():\n\
        \x20   auto option = Optional.Some(-4)\n\
        \x20   print(option.unwrap())\n",
    );
}

// ----- Nested constructor as a method argument (acceptance item) -----

#[test]
fn optional_negative_in_positional_method_arg() {
    // `Optional[int64] count = None` parameter, constructor passed positionally.
    lower_ok(
        "struct Sampler:\n\
        \x20   int seed\n\
        \x20   def pick(self, Optional[int64] count = None) -> bool:\n\
        \x20       return count.is_some()\n\
        def main():\n\
        \x20   auto s = Sampler(0)\n\
        \x20   print(s.pick(Optional.Some(-4)))\n",
    );
}

#[test]
fn optional_negative_in_keyword_method_arg() {
    // Same, passed by keyword (the Redis `srandmember(key, count=...)` shape).
    lower_ok(
        "struct Sampler:\n\
        \x20   int seed\n\
        \x20   def pick(self, Optional[int64] count = None) -> bool:\n\
        \x20       return count.is_some()\n\
        def main():\n\
        \x20   auto s = Sampler(0)\n\
        \x20   print(s.pick(count=Optional.Some(-4)))\n",
    );
}

#[test]
fn optional_positive_in_keyword_method_arg_still_infers() {
    // The positive counterpart of the keyword-argument case stays accepted.
    lower_ok(
        "struct Sampler:\n\
        \x20   int seed\n\
        \x20   def pick(self, Optional[int64] count = None) -> bool:\n\
        \x20       return count.is_some()\n\
        def main():\n\
        \x20   auto s = Sampler(0)\n\
        \x20   print(s.pick(count=Optional.Some(4)))\n",
    );
}

// ----- The fix is general, not Optional-only -----

#[test]
fn user_generic_struct_negative_int_literal_infers() {
    // A user generic constructor exercises the *same* inferer; a negative
    // literal must pin `T` here too.
    lower_ok(
        "struct Box[T]:\n\
        \x20   T item\n\
        def main():\n\
        \x20   auto b = Box(-4)\n\
        \x20   print(b.item)\n",
    );
}

#[test]
fn user_generic_struct_positive_int_literal_still_infers() {
    lower_ok(
        "struct Box[T]:\n\
        \x20   T item\n\
        def main():\n\
        \x20   auto b = Box(4)\n\
        \x20   print(b.item)\n",
    );
}

#[test]
fn generic_fn_negative_int_literal_infers() {
    // A generic *free function*'s own type parameter (I#161), inferred from a
    // negative-literal argument.
    lower_ok(
        "def wrap[T](T x) -> Optional[T]:\n\
        \x20   return Optional.Some(x)\n\
        def main():\n\
        \x20   print(wrap(-4).unwrap())\n",
    );
}

// ----- Unary operator coverage -----

#[test]
fn optional_unary_plus_int_literal_infers() {
    lower_ok(
        "def main():\n\
        \x20   auto option = Optional.Some(+4)\n\
        \x20   print(option.is_some())\n",
    );
}

#[test]
fn optional_bitnot_int_literal_infers() {
    // `~x` is an integer-preserving unary; inference must still pin `int64`.
    lower_ok(
        "def main():\n\
        \x20   auto option = Optional.Some(~4)\n\
        \x20   print(option.is_some())\n",
    );
}

#[test]
fn optional_logical_not_infers_bool() {
    // `not x` is always `bool`, so `T = bool`.
    lower_ok(
        "def main():\n\
        \x20   auto option = Optional.Some(not True)\n\
        \x20   print(option.unwrap())\n",
    );
}

#[test]
fn optional_double_unary_negation_infers() {
    // `- -4` is `Neg(Neg(Int))`; the operand type must recurse through both.
    lower_ok(
        "def main():\n\
        \x20   auto option = Optional.Some(- -4)\n\
        \x20   print(option.is_some())\n",
    );
}

// ----- Numeric literal kinds -----

#[test]
fn optional_negative_float_literal_infers() {
    lower_ok(
        "def main():\n\
        \x20   auto option = Optional.Some(-4.0)\n\
        \x20   print(option.is_some())\n",
    );
}

#[test]
fn optional_negative_min_magnitude_int_literal_infers() {
    // The i64::MIN magnitude: inference still pins `T = int64` from the unary
    // negation (the #1117 behaviour), but the literal `9223372036854775808`
    // overflows `int64` and is therefore a `bigint`, which is not implicitly
    // assignable to the `int64` payload — exactly as `int64 x =
    // -9223372036854775808` is rejected (`ZAP4001`). Lowering now rejects it
    // rather than silently storing the `bigint` pointer into the scalar payload
    // slot (#1379); previously this lowered to that miscompile.
    lower_err_contains(
        "def main():\n\
        \x20   auto option = Optional.Some(-9223372036854775808)\n\
        \x20   print(option.is_some())\n",
        "bigint",
    );
}

// ----- auto/let binding and return position -----

#[test]
fn optional_negative_in_typed_binding() {
    // A negative literal inside a constructor on the RHS of a typed binding.
    lower_ok(
        "def main():\n\
        \x20   Optional[int64] option = Optional.Some(-4)\n\
        \x20   print(option.is_some())\n",
    );
}

#[test]
fn optional_negative_in_return_position() {
    lower_ok(
        "def neg() -> Optional[int64]:\n\
        \x20   return Optional.Some(-4)\n\
        def main():\n\
        \x20   print(neg().unwrap())\n",
    );
}

// ----- Don't over-accept: a genuinely unresolvable parameter still fails -----

#[test]
fn negative_literal_does_not_mask_unresolvable_param() {
    // `Pair[A, B]` with only `a=-4` given: the negative literal pins `A` exactly
    // as a positive literal would, but `B` is still unconstrained, so inference
    // must *still* fail — and specifically on `B`, proving `A` was resolved from
    // the negated literal rather than the whole call being rejected on `A`.
    lower_err_contains(
        "struct Pair[A, B]:\n\
        \x20   A a\n\
        \x20   B b\n\
        def main():\n\
        \x20   auto p = Pair(a=-4)\n\
        \x20   print(p.a)\n",
        "type argument 'B'",
    );
}
