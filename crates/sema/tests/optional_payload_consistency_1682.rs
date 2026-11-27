//! Incompatible raw payloads for `Optional[T]` after `Type[T]` binds `T` (#1682).
//!
//! A follow-up to #1545. Binding a type parameter `T` from a `Type[T]` argument
//! must also constrain an `Optional[T]` parameter: a raw value supplied for
//! `Optional[T]` lifts into the payload, so it must be compatible with the bound
//! payload type. With `def select[T](Type[T] kind, Optional[T] fallback = None)`,
//! the `Type[int64]` argument establishes `T = int64`, yet a raw `fallback="oops"`
//! was silently accepted and lowered as a bare string into an `Optional[int64]`
//! slot. This closes that gap.
//!
//! The payload lift stays permissive in exactly the ways #1265 intends: a
//! matching integer (by name or position, with numeric widening) still lifts to
//! `Optional[int64]`; omission and an explicit `None` still work; and an
//! explicitly wrapped matching payload still compiles. Only a genuinely
//! incompatible raw payload, or a wrongly wrapped payload, is now rejected.
//!
//! Host-only sema checks (no LLVM). The rule is additive — before this change the
//! `Optional[T]` payload erased to `Unknown`, so the payload lift admitted any
//! raw value regardless of the bound `T`.

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
        "expected a {code} error, got: {errs:?}",
    );
}

const SELECT: &str = "def select[T](Type[T] kind, Optional[T] fallback = None) -> Optional[T]:\n    return fallback\n";

// ---- the issue's reproduction -----------------------------------------------

#[test]
fn type_arg_binds_t_then_rejects_raw_str_payload_by_keyword() {
    // The exact repro: `Type[int64]` establishes `T = int64`; a raw `str` cannot
    // lift into `Optional[int64]`.
    assert_code(
        &format!("{SELECT}def main():\n    auto accepted = select(int64, fallback=\"oops\")\n    print(accepted)\n"),
        "ZAP4021",
    );
}

#[test]
fn type_arg_binds_t_then_rejects_raw_str_payload_by_position() {
    // Same mismatch, supplied positionally.
    assert_code(
        &format!("{SELECT}def main():\n    auto accepted = select(int64, \"oops\")\n    print(accepted)\n"),
        "ZAP4021",
    );
}

// ---- matching raw payloads still lift ---------------------------------------

#[test]
fn matching_int_payload_by_keyword_lifts() {
    assert_ok(&format!(
        "{SELECT}def main():\n    auto ok = select(int64, fallback=7)\n    print(ok)\n"
    ));
}

#[test]
fn matching_int_payload_by_position_lifts() {
    assert_ok(&format!(
        "{SELECT}def main():\n    auto ok = select(int64, 7)\n    print(ok)\n"
    ));
}

#[test]
fn matching_str_payload_lifts() {
    assert_ok(&format!(
        "{SELECT}def main():\n    auto ok = select(str, fallback=\"hi\")\n    print(ok)\n"
    ));
}

#[test]
fn matching_bool_payload_lifts() {
    assert_ok(&format!(
        "{SELECT}def main():\n    auto ok = select(bool, fallback=False)\n    print(ok)\n"
    ));
}

#[test]
fn numeric_widening_payload_lifts() {
    // A narrower integer payload lifts into a wider bound (`int32` into
    // `Optional[int64]`) — the payload lift respects ordinary widening.
    assert_ok(&format!(
        "{SELECT}def main():\n    int32 small = 3\n    auto ok = select(int64, fallback=small)\n    print(ok)\n"
    ));
}

// ---- omission and explicit None still work ----------------------------------

#[test]
fn omitted_fallback_uses_default_none() {
    assert_ok(&format!(
        "{SELECT}def main():\n    auto ok = select(int64)\n    print(ok)\n"
    ));
}

#[test]
fn explicit_none_payload_is_accepted() {
    assert_ok(&format!(
        "{SELECT}def main():\n    auto ok = select(int64, fallback=None)\n    print(ok)\n"
    ));
}

#[test]
fn explicit_none_payload_for_str_is_accepted() {
    assert_ok(&format!(
        "{SELECT}def main():\n    auto ok = select(str, fallback=None)\n    print(ok)\n"
    ));
}

// ---- explicitly wrapped payloads --------------------------------------------

#[test]
fn explicitly_wrapped_matching_payload_is_accepted() {
    assert_ok(&format!(
        "{SELECT}def main():\n    Optional[int64] w = 7\n    auto ok = select(int64, fallback=w)\n    print(ok)\n"
    ));
}

#[test]
fn explicitly_wrapped_mismatched_payload_is_rejected() {
    // An explicit `Optional[str]` cannot satisfy `Optional[int64]` once `T` is
    // bound to `int64` — the payload-aware `Optional`-to-`Optional` rule (#1452)
    // rejects the differing payload through the same consistency pass.
    assert_code(
        &format!("{SELECT}def main():\n    Optional[str] w = \"oops\"\n    auto bad = select(int64, fallback=w)\n    print(bad)\n"),
        "ZAP4021",
    );
}

// ---- keyword ordering is irrelevant -----------------------------------------

#[test]
fn reordered_keywords_raw_payload_mismatch_is_rejected() {
    assert_code(
        &format!("{SELECT}def main():\n    auto bad = select(fallback=\"oops\", kind=int64)\n    print(bad)\n"),
        "ZAP4021",
    );
}

#[test]
fn reordered_keywords_matching_payload_is_accepted() {
    assert_ok(&format!(
        "{SELECT}def main():\n    auto ok = select(fallback=7, kind=int64)\n    print(ok)\n"
    ));
}

// ---- a second primitive to prove the rule is not int-specific ---------------

#[test]
fn str_bound_rejects_raw_int_payload() {
    assert_code(
        &format!("{SELECT}def main():\n    auto bad = select(str, fallback=5)\n    print(bad)\n"),
        "ZAP4021",
    );
}

// ---- Optional[T] mixed with an ordinary T parameter -------------------------

const SELECT2: &str =
    "def pick[T](Type[T] kind, T primary, Optional[T] fallback = None) -> T:\n    return primary\n";

#[test]
fn ordinary_and_optional_t_both_checked_mismatch() {
    // The ordinary `T` is consistent but the `Optional[T]` raw payload is not.
    assert_code(
        &format!("{SELECT2}def main():\n    auto bad = pick(int64, 1, fallback=\"oops\")\n    print(bad)\n"),
        "ZAP4021",
    );
}

#[test]
fn ordinary_and_optional_t_both_consistent_accepted() {
    assert_ok(&format!(
        "{SELECT2}def main():\n    auto ok = pick(int64, 1, fallback=2)\n    print(ok)\n"
    ));
}

// ---- the Optional[T] parameter binds T on its own ---------------------------

#[test]
fn optional_payload_binds_t_without_type_value() {
    // Without a `Type[T]` argument, a raw `Optional[T]` payload still binds `T`;
    // a later ordinary `T` argument must respect that binding.
    assert_code(
        "def two[T](Optional[T] a, T b) -> T:\n    return b\n\
         def main():\n    auto bad = two(1, \"x\")\n    print(bad)\n",
        "ZAP4021",
    );
}

#[test]
fn optional_payload_binds_t_consistent_accepted() {
    assert_ok(
        "def two[T](Optional[T] a, T b) -> T:\n    return b\n\
         def main():\n    auto ok = two(1, 2)\n    print(ok)\n",
    );
}

// ---- generic return substitution through Optional[T] (#1623/#1682) ----------
//
// Preserving the payload type variable also fixed the generic-return
// substitution: `wrap[T](T x) -> Optional[T]` must type a `str` call as
// `Optional[str]` (assignable to a declared `Optional[str]`), not leave the
// result as the unsubstituted `Optional[T]`. This is the host-level analogue of
// the `generics_fn_gc_stress.zap` end-to-end fixture.

const WRAP: &str = "def wrap[T](T x) -> Optional[T]:\n    return Optional.Some(x)\n";

#[test]
fn generic_optional_return_substitutes_str_payload() {
    assert_ok(&format!(
        "{WRAP}def main():\n    str s = \"x\"\n    Optional[str] o = wrap(s)\n    print(o)\n"
    ));
}

#[test]
fn generic_optional_return_substitutes_int_payload() {
    assert_ok(&format!(
        "{WRAP}def main():\n    int64 n = 3\n    Optional[int64] q = wrap(n)\n    print(q)\n"
    ));
}

#[test]
fn generic_optional_return_payload_mismatch_is_rejected() {
    // The substituted result `Optional[str]` is not assignable to a declared
    // `Optional[int64]` binding.
    assert_code(
        &format!("{WRAP}def main():\n    str s = \"x\"\n    Optional[int64] bad = wrap(s)\n    print(bad)\n"),
        "ZAP4001",
    );
}

#[test]
fn generic_optional_return_through_struct_field() {
    // The `generics_fn_gc_stress.zap` shape: wrap a generic struct field.
    assert_ok(
        "struct Pair[A, B]:\n    A first\n    B second\n\
         def wrap[T](T x) -> Optional[T]:\n    return Optional.Some(x)\n\
         def main():\n    Pair[int64, str] p = Pair(1, \"item\")\n    \
         Optional[str] o = wrap(p.second)\n    Optional[int64] q = wrap(p.first)\n    \
         print(o)\n    print(q)\n",
    );
}

// ---- Unknown / dynamic payload never triggers a false positive --------------

#[test]
fn dynamic_payload_does_not_force_a_binding() {
    assert_ok(&format!(
        "{SELECT}def main():\n    py d = \"anything\"\n    auto ok = select(int64, fallback=d)\n    print(ok)\n"
    ));
}
