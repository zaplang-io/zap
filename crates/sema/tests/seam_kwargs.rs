//! Seam calling convention: keyword / default argument binding for built-in
//! module functions (#998, epic #967).
//!
//! Built-in (seam) module functions used to be positional-only — they tolerated
//! keyword syntax but ignored the names, so a misspelled keyword on a stdlib
//! call was *silently accepted* (a correctness footgun) and keyword reordering
//! did not work. `zapc_sema::modules::bind_seam_args` gives an opt-in method a
//! Python-shaped calling convention; `zlib`/`gzip` `compress`/`decompress`
//! (and `zlib.crc32`/`adler32`) are the first migrated methods. These tests run
//! on the host (parse + sema), no LLVM.

use zapc_sema::check_str;

/// Sema errors (parse/lex asserted empty) for a program.
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

fn prog(body: &str) -> String {
    format!("def f():\n{body}\n")
}

// ---------------------------------------------------------------------------
// Acceptance item 1 — a known keyword binds by name and permits reordering.
// ---------------------------------------------------------------------------

#[test]
fn keyword_binds_by_name() {
    assert_ok(&prog("    auto b = zlib.compress(data=\"hi\", level=9)\n"));
}

#[test]
fn keyword_reordering_works() {
    // Swapped order: the int `level` and the str `data` must still land in the
    // right slots (positionally this would put the int in the data slot).
    assert_ok(&prog("    auto b = zlib.compress(level=9, data=\"hi\")\n"));
}

#[test]
fn positional_and_keyword_mix() {
    assert_ok(&prog("    auto b = zlib.compress(\"hi\", level=9)\n"));
}

// ---------------------------------------------------------------------------
// Acceptance item 2 — an unknown keyword is rejected (the silent-accept footgun).
// ---------------------------------------------------------------------------

#[test]
fn unknown_keyword_is_rejected() {
    assert_code(
        &prog("    auto b = zlib.compress(\"hi\", bogus=9)\n"),
        "ZAP4023",
    );
}

#[test]
fn unknown_keyword_on_decompress_is_rejected() {
    assert_code(
        &prog("    auto b = zlib.decompress(payload=\"hi\")\n"),
        "ZAP4023",
    );
}

#[test]
fn unknown_keyword_on_crc32_is_rejected() {
    assert_code(&prog("    auto n = zlib.crc32(buf=\"hi\")\n"), "ZAP4023");
}

// ---------------------------------------------------------------------------
// Acceptance item 3 — a defaulted parameter can be omitted and filled by name.
// ---------------------------------------------------------------------------

#[test]
fn defaulted_level_omitted_by_name() {
    // `data` supplied by name, optional `level` omitted entirely.
    assert_ok(&prog("    auto b = zlib.compress(data=\"hi\")\n"));
}

// ---------------------------------------------------------------------------
// Binding-error diagnostics.
// ---------------------------------------------------------------------------

#[test]
fn duplicate_value_for_parameter_is_rejected() {
    // `data` supplied positionally and again by keyword.
    assert_code(
        &prog("    auto b = zlib.compress(\"hi\", data=\"bye\")\n"),
        "ZAP4023",
    );
}

#[test]
fn missing_earlier_parameter_is_rejected() {
    // `level` by name with `data` omitted → a hole, not a silent shift of
    // `level` into the `data` slot.
    assert_code(&prog("    auto b = zlib.compress(level=9)\n"), "ZAP4025");
}

// ---------------------------------------------------------------------------
// gzip shares the migrated check path.
// ---------------------------------------------------------------------------

#[test]
fn gzip_keyword_binding_shared() {
    assert_ok(&prog("    auto b = gzip.compress(data=\"hi\", level=6)\n"));
}

#[test]
fn gzip_unknown_keyword_rejected() {
    assert_code(
        &prog("    auto b = gzip.compress(\"hi\", bogus=1)\n"),
        "ZAP4023",
    );
}

// ---------------------------------------------------------------------------
// Regression — plain positional calls are unchanged.
// ---------------------------------------------------------------------------

#[test]
fn positional_calls_unchanged() {
    assert_ok(&prog("    auto a = zlib.compress(\"hi\")\n"));
    assert_ok(&prog("    auto b = zlib.compress(\"hi\", 9)\n"));
    assert_ok(&prog("    auto c = zlib.decompress(\"hi\")\n"));
    assert_ok(&prog("    auto d = zlib.crc32(\"hi\")\n"));
}

#[test]
fn wrong_type_keyword_still_type_checks() {
    // `level` bound by name but a str → the existing level type check (ZAP4024)
    // still fires on the normalized argument.
    assert_code(
        &prog("    auto b = zlib.compress(\"hi\", level=\"nine\")\n"),
        "ZAP4024",
    );
}
