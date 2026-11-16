//! Sliced assignment `xs[lo:hi:step] = ys` (#1005, D-0030). Host-only (parse +
//! sema); end-to-end behavior (splice / extended-slice / raise) is in the
//! `slice_assign` golden.

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

/// `def f(list[int64] xs, list[int64] ys):` + `body` (indented 4).
fn prog(body: &str) -> String {
    format!("def f(list[int64] xs, list[int64] ys):\n{body}")
}

#[test]
fn step1_slice_assign() {
    assert_ok(&prog("    xs[0:2] = ys\n"));
}

#[test]
fn full_slice_assign() {
    assert_ok(&prog("    xs[:] = ys\n"));
}

#[test]
fn open_bound_slice_assign() {
    assert_ok(&prog("    xs[1:] = ys\n"));
    assert_ok(&prog("    xs[:2] = ys\n"));
}

#[test]
fn extended_slice_assign() {
    assert_ok(&prog("    xs[0:4:2] = ys\n"));
}

#[test]
fn single_index_assign_still_ok() {
    // Regression: the non-slice `xs[i] = v` path is unchanged.
    assert_ok(&prog("    xs[0] = 5\n"));
}
