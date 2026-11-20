//! Monomorphization consumes sema's inferred types (#1167): a generic argument
//! whose type the monomorphizer's own best-effort inferer cannot derive is
//! pinned from the sema `TypeMap` the driver threads into lowering. This tests
//! the real integration path — `analyze_types` → `lower_module_opts_with_types`
//! — on the exact forms that previously failed with `ZAP6001`: a module
//! constant (`math.inf`, #1162) and built-in call returns (`len`, `str.upper`,
//! `list.pop`, #1132). Host-only (no LLVM): lowering runs the mono pass.

use zapc_ir::{lower_module, lower_module_opts_with_types};
use zapc_sema::analyze_types;
use zapc_syntax::parser::parse_str;

/// Assert `src` type-checks, and that lowering it **with** sema's types infers
/// (no lowering error) while lowering it **without** them still fails — proving
/// the sema `TypeMap`, not some change to mono's own inferer, is what closes the
/// gap.
fn sema_types_close_the_gap(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");

    let (errs, _warns, types) = analyze_types(&ast);
    assert!(errs.is_empty(), "sema errors: {errs:?}");

    let (_zir, with_errs) = lower_module_opts_with_types(&ast, false, &types);
    assert!(
        with_errs.is_empty(),
        "lowering with sema types should infer, got: {with_errs:?}"
    );

    let (_zir, without_errs) = lower_module(&ast);
    assert!(
        !without_errs.is_empty(),
        "expected lowering WITHOUT sema types to still fail (mono's own inferer \
         cannot type this argument); if this now passes, the test no longer \
         guards the sema-backfill path"
    );
}

#[test]
fn module_constant_math_inf_infers_with_sema_types() {
    // #1162: `math.inf` is a module-qualified constant mono cannot type.
    sema_types_close_the_gap(
        "import math\n\
        def main():\n\
        \x20   auto value = Optional.Some(math.inf)\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn builtin_len_return_infers_with_sema_types() {
    // #1132: a built-in free-function call's return type.
    sema_types_close_the_gap(
        "def main():\n\
        \x20   list[int64] xs = [1, 2, 3]\n\
        \x20   auto value = Optional.Some(len(xs))\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn builtin_str_method_return_infers_with_sema_types() {
    // #1132: a built-in method call's return type.
    sema_types_close_the_gap(
        "def main():\n\
        \x20   str s = \"hi\"\n\
        \x20   auto value = Optional.Some(s.upper())\n\
        \x20   print(value.is_some())\n",
    );
}

#[test]
fn builtin_list_method_return_infers_with_sema_types() {
    sema_types_close_the_gap(
        "def main():\n\
        \x20   list[int64] xs = [1, 2, 3]\n\
        \x20   auto value = Optional.Some(xs.pop())\n\
        \x20   print(value.is_some())\n",
    );
}

/// Assert `src` type-checks, but lowering it **with** sema's types is rejected
/// with the `cannot infer type argument` (`ZAP6001`) error — the fail-closed
/// boundary (#1190). Sema accepts the program: it types the generator/iterator
/// argument as a synthetic `$`-prefixed iterator type (`$genexpr` for a bare
/// generator expression, `$gen:<name>` for a generator function). That
/// synthetic, element-erased, unspellable type must never be bridged into
/// monomorphization as a user-nameable `Optional$$genexpr` / `Optional$$gen:…`
/// instance (an untested, likely-unsound struct/enum layout); the backfill must
/// leave the argument `Unknown` so the generic is rejected cleanly, exactly as
/// before #1167. We also pin that no `$`-sigil instance leaks into the ZIR.
fn sema_types_reject_synthetic(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");

    let (serrs, _warns, types) = analyze_types(&ast);
    assert!(serrs.is_empty(), "sema errors: {serrs:?}");

    let (zir, with_errs) = lower_module_opts_with_types(&ast, false, &types);
    let joined = with_errs
        .iter()
        .map(|e| format!("{:?} {}", e.code, e.message))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        with_errs
            .iter()
            .any(|e| e.message.contains("cannot infer type argument")),
        "a synthetic-iterator generic argument must be rejected with ZAP6001 \
         (not monomorphized over a `$`-sigil synthetic type); got errors:\n{joined}"
    );
    // Belt-and-suspenders: even if some other inference path ever typed it, no
    // monomorphized instance whose name carries the synthetic `$` sigil (beyond
    // the normal `Optional$<concrete>` mangling) may reach the ZIR.
    let leaked: Vec<&str> = zir
        .funcs
        .iter()
        .map(|f| f.name.as_str())
        .filter(|n| n.contains("$$genexpr") || n.contains("$$gen:") || n.contains("$$iface:"))
        .collect();
    assert!(
        leaked.is_empty(),
        "synthetic `$`-sigil type leaked into a monomorphized instance: {leaked:?}"
    );
}

#[test]
fn generator_expression_rejected_not_monomorphized() {
    // #1190: a bare generator expression's type is the synthetic `$genexpr`
    // iterator. It must be rejected with ZAP6001, NOT monomorphized over
    // `Optional$$genexpr` (the #1167 bridge regression).
    sema_types_reject_synthetic(
        "def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto o = Optional.Some((x for x in xs))\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn generator_expression_in_user_generic_rejected() {
    // The same synthetic type reaching a user-defined generic struct.
    sema_types_reject_synthetic(
        "struct Box[T]:\n\
        \x20   T item\n\
        def main():\n\
        \x20   list[int64] xs = [1, 2]\n\
        \x20   auto b = Box((x for x in xs))\n\
        \x20   print(b.item)\n",
    );
}

#[test]
fn generator_function_call_rejected_not_monomorphized() {
    // #1190: a generator *function* call `gen()` has the synthetic `$gen:gen`
    // iterator type. Passing it to a generic constructor must also be rejected,
    // not monomorphized over `Optional$$gen:gen`. (A generator must not declare
    // a return type — calling it yields an `Iterator`, I#67/D-0048.)
    sema_types_reject_synthetic(
        "def gen():\n\
        \x20   yield 1\n\
        \x20   yield 2\n\
        def main():\n\
        \x20   auto o = Optional.Some(gen())\n\
        \x20   print(o.is_some())\n",
    );
}

#[test]
fn baseline_without_types_is_unchanged() {
    // A program mono already infers on its own lowers identically with an empty
    // map (no sema types), so the typed path is a pure augmentation.
    let src = "def main():\n\
        \x20   auto value = Optional.Some(4)\n\
        \x20   print(value.is_some())\n";
    let (ast, _p, _l) = parse_str(src);
    let (_z, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "baseline should lower: {errs:?}");
}
