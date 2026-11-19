//! Built-in `list[T]` methods — `pop` / `index` / `reverse` / `copy` (#1095),
//! alongside the #977 mutation set. Host-only (parse + sema): the precise return
//! types the typer assigns, the arity/type rejections that build the walled
//! garden, and the Python-parity decision that `pop`/`index` are positional-only
//! (no keyword form), so these rows are intentionally ABSENT from
//! `seam_sigs.rs` — list methods are built-in generic dispatch, not seam module
//! functions, and CPython itself rejects `l.pop(index=…)`.

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

// ---- pop -------------------------------------------------------------------

#[test]
fn pop_no_arg_and_index_yield_element_type() {
    // pop() -> elem, so it composes where an int is wanted.
    assert_ok("def f():\n    list[int64] xs = [1, 2]\n    int64 x = xs.pop()\n    print(x)\n");
    assert_ok("def f():\n    list[int64] xs = [1, 2]\n    int64 x = xs.pop(0)\n    print(x)\n");
    // A str list yields a str element.
    assert_ok("def f():\n    list[str] ws = [\"a\", \"b\"]\n    str w = ws.pop()\n    print(w)\n");
}

#[test]
fn pop_element_type_mismatch_rejected() {
    // pop() yields the element type (int64 here), so binding it to a `str`
    // fails the assignment type check — witnessing the precise return type.
    assert_code(
        "def f():\n    list[int64] xs = [1, 2]\n    str s = xs.pop()\n    print(s)\n",
        "ZAP4001",
    );
}

#[test]
fn pop_too_many_args_rejected() {
    assert_code(
        "def f():\n    list[int64] xs = [1, 2]\n    auto x = xs.pop(0, 1)\n    print(x)\n",
        "ZAP4025",
    );
}

#[test]
fn pop_non_int_index_rejected() {
    assert_code(
        "def f():\n    list[int64] xs = [1, 2]\n    auto x = xs.pop(\"no\")\n    print(x)\n",
        "ZAP4024",
    );
}

// ---- index -----------------------------------------------------------------

#[test]
fn index_yields_int_with_optional_bounds() {
    assert_ok(
        "def f():\n    list[int64] xs = [1, 2, 3]\n    int64 i = xs.index(2)\n    print(i)\n",
    );
    assert_ok(
        "def f():\n    list[int64] xs = [1, 2, 3]\n    int64 i = xs.index(2, 1)\n    print(i)\n",
    );
    assert_ok(
        "def f():\n    list[int64] xs = [1, 2, 3]\n    int64 i = xs.index(2, 0, 3)\n    print(i)\n",
    );
}

#[test]
fn index_requires_a_value() {
    assert_code(
        "def f():\n    list[int64] xs = [1, 2]\n    auto i = xs.index()\n    print(i)\n",
        "ZAP4025",
    );
}

#[test]
fn index_too_many_args_rejected() {
    assert_code(
        "def f():\n    list[int64] xs = [1, 2]\n    auto i = xs.index(1, 0, 2, 9)\n    print(i)\n",
        "ZAP4025",
    );
}

#[test]
fn index_value_type_mismatch_rejected() {
    assert_code(
        "def f():\n    list[int64] xs = [1, 2]\n    auto i = xs.index(\"no\")\n    print(i)\n",
        "ZAP4024",
    );
}

#[test]
fn index_non_int_bound_rejected() {
    assert_code(
        "def f():\n    list[int64] xs = [1, 2]\n    auto i = xs.index(1, \"no\")\n    print(i)\n",
        "ZAP4024",
    );
}

// ---- reverse ---------------------------------------------------------------

#[test]
fn reverse_takes_no_args() {
    assert_ok("def f():\n    list[int64] xs = [1, 2, 3]\n    xs.reverse()\n    print(len(xs))\n");
}

#[test]
fn reverse_rejects_args() {
    assert_code(
        "def f():\n    list[int64] xs = [1, 2]\n    xs.reverse(1)\n    print(len(xs))\n",
        "ZAP4025",
    );
}

// ---- copy ------------------------------------------------------------------

#[test]
fn copy_yields_same_list_type() {
    assert_ok(
        "def f():\n    list[int64] xs = [1, 2]\n    list[int64] ys = xs.copy()\n    print(len(ys))\n",
    );
    assert_ok(
        "def f():\n    list[str] ws = [\"a\"]\n    list[str] cp = ws.copy()\n    print(len(cp))\n",
    );
}

#[test]
fn copy_rejects_args() {
    assert_code(
        "def f():\n    list[int64] xs = [1, 2]\n    auto cp = xs.copy(1)\n    print(len(cp))\n",
        "ZAP4025",
    );
}

#[test]
fn copy_result_is_a_list_element_type_mismatch_rejected() {
    // The copy keeps the element type (list[int64]), so binding it to a
    // list[str] fails the assignment type check — witnessing the precise type.
    assert_code(
        "def f():\n    list[int64] xs = [1, 2]\n    list[str] bad = xs.copy()\n    print(len(bad))\n",
        "ZAP4001",
    );
}
