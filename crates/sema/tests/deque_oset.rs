//! `deque[T]` / `oset[T]` as re-skinned generic collection types (#983, D-0208).
//!
//! Host-only (parse + sema): constructor forms, element bounds, method typing,
//! membership, iteration, and the rejections that build the walled garden. The
//! old int64-only `deque.*`/`oset.*` module API is gone, so those calls must now
//! fail as unknown-module calls rather than silently work.

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

// ---- deque -----------------------------------------------------------------

#[test]
fn deque_ctor_int_and_str_and_float() {
    assert_ok("def f():\n    auto d = deque[int64]()\n    print(len(d))\n");
    assert_ok("def f():\n    auto d = deque[str]()\n    d.append(\"x\")\n");
    assert_ok("def f():\n    auto d = deque[float64]()\n    d.append(1.5)\n");
}

#[test]
fn deque_maxlen_keyword() {
    assert_ok("def f():\n    auto d = deque[int64](maxlen=4)\n    d.append(1)\n");
}

#[test]
fn deque_maxlen_must_be_int() {
    assert_code(
        "def f():\n    auto d = deque[int64](maxlen=\"no\")\n",
        "ZAP5101",
    );
}

#[test]
fn deque_unknown_keyword_rejected() {
    assert_code("def f():\n    auto d = deque[int64](cap=4)\n", "ZAP5102");
}

#[test]
fn deque_positional_arg_rejected() {
    assert_code("def f():\n    auto d = deque[int64](4)\n", "ZAP5103");
}

#[test]
fn deque_methods_type_check() {
    // append/appendleft take one element; pop/popleft/get yield the element type.
    assert_ok(
        "def f():\n    auto d = deque[int64]()\n    d.append(1)\n    d.appendleft(0)\n    \
         int64 x = d.pop()\n    int64 y = d.popleft()\n    int64 z = d.get(0)\n    d.clear()\n",
    );
}

#[test]
fn deque_index_and_iteration() {
    assert_ok(
        "def f():\n    auto d = deque[int64]()\n    d.append(1)\n    int64 a = d[0]\n    \
         for x in d:\n        print(x)\n",
    );
}

#[test]
fn deque_append_wrong_element_rejected() {
    assert_code(
        "def f():\n    auto d = deque[int64]()\n    d.append(\"no\")\n",
        "ZAP4024",
    );
}

#[test]
fn deque_unknown_method_rejected() {
    assert_code(
        "def f():\n    auto d = deque[int64]()\n    d.frobnicate(1)\n",
        "ZAP4080",
    );
}

// ---- oset ------------------------------------------------------------------

#[test]
fn oset_ctor_int_and_str() {
    assert_ok("def f():\n    auto s = oset[int64]()\n    s.add(1)\n");
    assert_ok("def f():\n    auto s = oset[str]()\n    s.add(\"x\")\n");
}

#[test]
fn oset_from_iterable() {
    assert_ok("def f():\n    auto s = oset[int64]([1, 2, 1, 3])\n    print(len(s))\n");
}

#[test]
fn oset_membership_and_iteration() {
    assert_ok(
        "def f():\n    auto s = oset[str]()\n    s.add(\"a\")\n    bool b = \"a\" in s\n    \
         for x in s:\n        print(x)\n",
    );
}

#[test]
fn oset_methods_type_check() {
    assert_ok(
        "def f():\n    auto s = oset[int64]()\n    s.add(1)\n    bool b = s.contains(1)\n    \
         int64 x = s.get(0)\n    print(len(s))\n",
    );
}

#[test]
fn oset_add_wrong_element_rejected() {
    assert_code(
        "def f():\n    auto s = oset[int64]()\n    s.add(\"no\")\n",
        "ZAP4024",
    );
}

#[test]
fn oset_float_element_rejected() {
    // v1 `oset` supports i64/str; a float element is a documented follow-up.
    assert_code("def f():\n    auto s = oset[float64]()\n", "ZAP4024");
}

#[test]
fn oset_unknown_method_rejected() {
    assert_code(
        "def f():\n    auto s = oset[int64]()\n    s.discard(1)\n",
        "ZAP4080",
    );
}

// ---- old module API is gone ------------------------------------------------

#[track_caller]
fn assert_some_error(src: &str) {
    let errs = sema_errors(src);
    assert!(
        !errs.is_empty(),
        "expected the removed module API to be rejected, but it type-checked: {src:?}"
    );
}

#[test]
fn old_deque_module_api_is_gone() {
    // `deque.new()` was the int64-only seam module; it no longer resolves now
    // that `deque` is a collection type rather than a module.
    assert_some_error("def f():\n    auto d = deque.new()\n");
}

#[test]
fn old_oset_module_api_is_gone() {
    assert_some_error("def f():\n    auto s = oset.new()\n");
}
