//! Tuple types as explicit generic type arguments (#1478, #1479).
//!
//! The monomorphizer could not take a tuple type as a generic type argument,
//! which broke generic calls with an explicit `[...]` type argument two ways:
//!
//!  * **#1479** — a type argument *containing* a tuple (`run[dict[bytes,
//!    (bytes, bytes)]]`). `expr_to_type` had no `Expr::Tuple` arm, so the whole
//!    type argument resolved to `None`; the generic call was not recognized and
//!    fell through to the structural rewrite, so a generic *method* call was
//!    mis-lowered as a field access (`ZAP6001: no field 'run' on struct`).
//!
//!  * **#1478** — a type argument that *is* a tuple (`run[(Optional[bytes],
//!    Optional[bytes])]`). `type_args_from_index` split every tuple index into
//!    separate arguments regardless of the generic's arity, so a one-parameter
//!    generic bound `T` to only the first element; the tuple-typed argument
//!    then could not lower (`ZAP6001: this expression is not supported by
//!    codegen yet (Phase 1 slice)` for a `None` element, or `cannot destructure
//!    a non-tuple value` for the mis-typed result).
//!
//! The parser renders `g[A, B]` and `g[(A, B)]` identically as a tuple index,
//! so the split is now gated on the generic's declared parameter count: a tuple
//! index is split only when its arity matches (`dict[K, V]`, `two[A, B]`);
//! otherwise the whole index is one (possibly tuple) type argument.
//!
//! These host-only checks (no LLVM) pin that every such call now lowers. The
//! end-to-end runtime *values* are covered by the
//! `generic_tuple_type_arg` golden on CI.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting no lex/parse/lower errors.
fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
}

// --------------------------------------------------------------------------
// #1478 — a tuple type that IS the single explicit type argument.
// --------------------------------------------------------------------------

#[test]
fn generic_method_tuple_targ_none_elements_exact_repro() {
    // The issue's reducer: a generic method whose explicit `T` is a tuple of
    // options, called with `(None, None)` and a lambda producing the pair.
    lower_ok(
        "struct Pool:\n\
        \x20   int64 unused\n\
        \x20   def pair(self) -> (Optional[bytes], Optional[bytes]):\n\
        \x20       return (Optional.Some(b\"v\"), Optional.Some(b\"t\"))\n\
        struct Router:\n\
        \x20   Pool pool\n\
        \x20   def run[T](self, T fallback, fn(Pool) -> T action) -> T:\n\
        \x20       return action(self.pool)\n\
        \x20   def pair(self) -> (Optional[bytes], Optional[bytes]):\n\
        \x20       return self.run[(Optional[bytes], Optional[bytes])]((None, None), lambda pool: pool.pair())\n\
        def main():\n\
        \x20   auto router = Router(Pool(0))\n\
        \x20   auto value, token = router.pair()\n\
        \x20   print(value.unwrap(), token.unwrap())\n",
    );
}

#[test]
fn generic_free_fn_tuple_targ_none_elements() {
    // The same shape on a generic *free function* (no receiver): the arity
    // gating lives in the shared `type_args_from_index`, so both paths fix.
    lower_ok(
        "def run[T](T fallback) -> T:\n\
        \x20   return fallback\n\
        def main():\n\
        \x20   auto r = run[(Optional[bytes], Optional[bytes])]((None, None))\n\
        \x20   auto a, b = r\n\
        \x20   print(a.is_none(), b.is_none())\n",
    );
}

#[test]
fn generic_method_tuple_targ_all_present() {
    // No `None` at all: before the fix the one-parameter `T` was bound to only
    // the first element, so the result was a scalar `Optional`, not a tuple,
    // and destructuring failed with `cannot destructure a non-tuple value`.
    lower_ok(
        "struct Router:\n\
        \x20   int64 unused\n\
        \x20   def run[T](self, T fallback) -> T:\n\
        \x20       return fallback\n\
        def main():\n\
        \x20   auto router = Router(0)\n\
        \x20   auto r = router.run[(Optional[bytes], Optional[bytes])]((Optional.Some(b\"a\"), Optional.Some(b\"b\")))\n\
        \x20   auto a, b = r\n\
        \x20   print(a.unwrap(), b.unwrap())\n",
    );
}

#[test]
fn generic_method_tuple_targ_mixed_present_and_none() {
    lower_ok(
        "struct Router:\n\
        \x20   int64 unused\n\
        \x20   def run[T](self, T fallback) -> T:\n\
        \x20       return fallback\n\
        def main():\n\
        \x20   auto router = Router(0)\n\
        \x20   auto r = router.run[(Optional[bytes], Optional[bytes])]((Optional.Some(b\"a\"), None))\n\
        \x20   auto a, b = r\n\
        \x20   print(a.unwrap(), b.is_none())\n",
    );
}

#[test]
fn generic_fn_tuple_targ_scalar_payload_none() {
    // Scalar-payload options (`int64`): the tagged-`None` element path, not the
    // null-pointer one, exercised through a tuple type argument.
    lower_ok(
        "def run[T](T fallback) -> T:\n\
        \x20   return fallback\n\
        def main():\n\
        \x20   auto r = run[(Optional[int64], Optional[int64])]((None, None))\n\
        \x20   auto a, b = r\n\
        \x20   print(a.is_none(), b.is_none())\n",
    );
}

#[test]
fn generic_fn_three_element_tuple_targ() {
    // A three-element tuple argument (arity 3 for a one-parameter generic):
    // still a single tuple type, mixed present/absent payloads.
    lower_ok(
        "def run[T](T fallback) -> T:\n\
        \x20   return fallback\n\
        def main():\n\
        \x20   auto r = run[(Optional[bytes], Optional[int64], Optional[bytes])]((None, Optional.Some(7), None))\n\
        \x20   auto a, b, c = r\n\
        \x20   print(a.is_none(), b.unwrap(), c.is_none())\n",
    );
}

// --------------------------------------------------------------------------
// #1479 — a type argument CONTAINING a tuple (nested).
// --------------------------------------------------------------------------

#[test]
fn generic_method_dict_of_tuple_values_exact_repro() {
    // The issue's reducer: `run[dict[bytes, (bytes, bytes)]]`. The nested
    // `(bytes, bytes)` value type made the whole type argument unresolvable, so
    // `run` was mis-lowered as a field access (`no field 'run' on struct`).
    lower_ok(
        "struct Pool:\n\
        \x20   int64 unused\n\
        \x20   def pairs(self) -> dict[bytes, (bytes, bytes)]:\n\
        \x20       dict[bytes, (bytes, bytes)] found = {}\n\
        \x20       found[b\"k\"] = (b\"value\", b\"token\")\n\
        \x20       return found\n\
        struct Router:\n\
        \x20   Pool pool\n\
        \x20   def run[T](self, T fallback, fn(Pool) -> T action) -> T:\n\
        \x20       return action(self.pool)\n\
        \x20   def pairs(self) -> dict[bytes, (bytes, bytes)]:\n\
        \x20       dict[bytes, (bytes, bytes)] empty = {}\n\
        \x20       auto found = self.run[dict[bytes, (bytes, bytes)]](empty, lambda pool: pool.pairs())\n\
        \x20       return found\n\
        def main():\n\
        \x20   auto router = Router(Pool(0))\n\
        \x20   auto value, token = router.pairs()[b\"k\"]\n\
        \x20   print(value, token)\n",
    );
}

#[test]
fn generic_free_fn_dict_of_tuple_values() {
    lower_ok(
        "def run[T](T fallback) -> T:\n\
        \x20   return fallback\n\
        def main():\n\
        \x20   dict[bytes, (bytes, bytes)] empty = {}\n\
        \x20   auto d = run[dict[bytes, (bytes, bytes)]](empty)\n\
        \x20   d[b\"k\"] = (b\"v\", b\"t\")\n\
        \x20   auto value, token = d[b\"k\"]\n\
        \x20   print(value, token)\n",
    );
}

#[test]
fn generic_fn_list_of_tuple_targ() {
    // A nested tuple inside a `list` type argument resolves too.
    lower_ok(
        "def run[T](T fallback) -> T:\n\
        \x20   return fallback\n\
        def main():\n\
        \x20   list[(bytes, bytes)] empty = []\n\
        \x20   auto xs = run[list[(bytes, bytes)]](empty)\n\
        \x20   xs.append((b\"v\", b\"t\"))\n\
        \x20   auto value, token = xs[0]\n\
        \x20   print(value, token)\n",
    );
}

#[test]
fn generic_fn_optional_of_tuple_targ() {
    // A nested tuple inside an `Optional` type argument.
    lower_ok(
        "def run[T](T fallback) -> T:\n\
        \x20   return fallback\n\
        def main():\n\
        \x20   auto o = run[Optional[(bytes, bytes)]](Optional.Some((b\"v\", b\"t\")))\n\
        \x20   auto value, token = o.unwrap()\n\
        \x20   print(value, token)\n",
    );
}

// --------------------------------------------------------------------------
// Arity guard — a multi-parameter generic must still SPLIT its tuple index,
// so the fix narrows only the arity-mismatched (tuple-as-single-arg) case.
// --------------------------------------------------------------------------

#[test]
fn two_parameter_generic_fn_still_splits_type_args() {
    lower_ok(
        "def two[A, B](A a, B b) -> A:\n\
        \x20   return a\n\
        def main():\n\
        \x20   print(two[int64, str](1, \"x\"))\n",
    );
}

#[test]
fn two_parameter_generic_method_still_splits_type_args() {
    lower_ok(
        "struct Pair:\n\
        \x20   int64 unused\n\
        \x20   def two[A, B](self, A a, B b) -> B:\n\
        \x20       return b\n\
        def main():\n\
        \x20   auto p = Pair(0)\n\
        \x20   print(p.two[int64, str](1, \"x\"))\n",
    );
}

#[test]
fn two_parameter_generic_with_a_tuple_element_arg() {
    // A two-parameter generic whose FIRST argument is itself a tuple:
    // `[(bytes, bytes), int64]` — arity 2 matches, so it splits into a tuple
    // type and an `int64`, and the tuple element still resolves.
    lower_ok(
        "def two[A, B](A a, B b) -> B:\n\
        \x20   return b\n\
        def main():\n\
        \x20   print(two[(bytes, bytes), int64]((b\"v\", b\"t\"), 5))\n",
    );
}
