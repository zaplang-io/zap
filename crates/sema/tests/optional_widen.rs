//! Bare-payload widening into `Optional[T]` (#1265).
//!
//! A concrete value of type `T` coerces to `Optional[T]` by implicit `Some(T)`
//! wrapping, in every type-contextual position — matching Python's
//! `x: Optional[int] = 1` and completing the bare-`None` widening that already
//! held. Before this, sema accepted a bare payload only in a struct-constructor
//! field (which it does not type-check at all) and rejected it in a `return`
//! (`ZAP4003`), an `Optional[T]` parameter (`ZAP4021`), a typed binding and a
//! list-literal element (`ZAP4001`), even though an explicit `Optional.Some(1)`
//! was accepted everywhere. These host-only checks pin that each position now
//! type-checks; the end-to-end tagged-`Some` *values* (`is_some`/`unwrap`/
//! `unwrap_or`) are covered by the `optional_widen` golden and the
//! `optional_widen_module` golden project on CI (the wrap itself is #1262, in IR
//! lowering).

use zapc_sema::check_str;

#[track_caller]
fn sema_errors(src: &str) -> Vec<(String, String)> {
    let (terrs, perrs, lerrs) = check_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    terrs
        .into_iter()
        .map(|e| (e.code.to_string(), e.message))
        .collect()
}

#[track_caller]
fn assert_ok(src: &str) {
    let errs = sema_errors(src);
    assert!(errs.is_empty(), "expected no sema errors, got: {errs:?}");
}

#[track_caller]
fn assert_code(src: &str, code: &str) {
    let errs = sema_errors(src);
    assert!(
        errs.iter().any(|(c, _)| c == code),
        "expected a {code} error, got: {errs:?}"
    );
}

// ----- the four positions that used to reject a bare payload -----

#[test]
fn return_scalar_payload() {
    assert_ok(
        "def make() -> Optional[int64]:\n\
        \x20   return 41\n",
    );
}

#[test]
fn return_zero_payload() {
    // The adversarial zero: a raw 0 reads like a null Optional, so this must be
    // the *wrapped* zero, not a bare value — but at sema it is simply accepted.
    assert_ok(
        "def make() -> Optional[int64]:\n\
        \x20   return 0\n",
    );
}

#[test]
fn return_reference_payload() {
    assert_ok(
        "def label() -> Optional[str]:\n\
        \x20   return \"hi\"\n",
    );
}

#[test]
fn optional_parameter_scalar_payload() {
    assert_ok(
        "def take(Optional[int64] x) -> int64:\n\
        \x20   return x.unwrap_or(-1)\n\
        def main():\n\
        \x20   print(take(4))\n",
    );
}

#[test]
fn typed_binding_scalar_payload() {
    assert_ok(
        "def main():\n\
        \x20   Optional[int64] x = 5\n\
        \x20   print(x.unwrap())\n",
    );
}

#[test]
fn typed_binding_reference_payload() {
    assert_ok(
        "def main():\n\
        \x20   Optional[str] s = \"zap\"\n\
        \x20   print(s.unwrap())\n",
    );
}

#[test]
fn list_literal_scalar_payload_elements() {
    assert_ok(
        "def main():\n\
        \x20   list[Optional[int64]] xs = [1, 0, 2]\n\
        \x20   print(xs[0].unwrap())\n",
    );
}

#[test]
fn list_literal_mixed_payload_none_some() {
    assert_ok(
        "def main():\n\
        \x20   list[Optional[int64]] xs = [1, None, Optional.Some(3)]\n\
        \x20   print(xs[0].unwrap())\n",
    );
}

#[test]
fn dict_value_payload_binding() {
    assert_ok(
        "def main():\n\
        \x20   dict[str, Optional[int64]] m = {\"a\": 1}\n\
        \x20   print(m[\"a\"].unwrap())\n",
    );
}

#[test]
fn reference_struct_payload_return() {
    assert_ok(
        "struct Item:\n\
        \x20   int64 value\n\
        def wrap() -> Optional[Item]:\n\
        \x20   return Item(5)\n",
    );
}

// ----- existing behavior preserved -----

#[test]
fn bare_none_still_widens_everywhere() {
    // `None` infers to `Unknown`, which `assignable` admits everywhere — the
    // widening partner this completes. Must stay accepted.
    assert_ok(
        "def make() -> Optional[int64]:\n\
        \x20   return None\n\
        def main():\n\
        \x20   Optional[int64] x = None\n\
        \x20   print(make().is_none(), x.is_none())\n",
    );
}

#[test]
fn explicit_some_still_accepted() {
    assert_ok(
        "def make() -> Optional[int64]:\n\
        \x20   return Optional.Some(7)\n",
    );
}

#[test]
fn optional_to_optional_not_rewrapped() {
    // An existing Optional flows by the ordinary Optional→Optional identity, not
    // the new widening (which excludes an Optional source).
    assert_ok(
        "def pass_through(Optional[int64] x) -> Optional[int64]:\n\
        \x20   return x\n",
    );
}

#[test]
fn unrelated_type_mismatch_still_rejected() {
    // Widening targets `Optional[T]` only — a plain non-optional mismatch is
    // still a clean error.
    assert_code(
        "def main():\n\
        \x20   int64 x = \"hello\"\n\
        \x20   print(x)\n",
        "ZAP4001",
    );
}
