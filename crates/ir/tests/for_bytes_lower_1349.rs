//! #1349: `for b in <bytes>` must lower to ZIR, not fail codegen.
//!
//! Before the fix, iterating a `bytes` value fell through the `for`-loop
//! lowering's type dispatch to the catch-all ZAP6001 ("only `for ... in
//! range(...)`, ... are supported by codegen yet"). The loop now lowers like a
//! `list` walk — index `0..zap_str_len`, element via `zap_bytes_index` (an
//! unsigned `int64` in 0..=255) — binding each byte to an `int64` loop
//! variable.
//!
//! These run the real lowering pass (mono + AST->ZIR) **without** sema's
//! `TypeMap`, proving the monomorphizer's own inferer (`Mono::iter_elem`) types
//! the loop variable as `int64` — the exact path the native `zap test` harness
//! takes (it lowers with an empty `TypeMap`, cf. #1308). So they guard the fix
//! wherever `cargo test` runs (host-only, no LLVM). The byte *values*
//! (0x00/0xFF unsigned, order, empty) are covered end-to-end by the
//! `tests/golden/bytes_for_loop.zap` golden and `test_bytes_iteration_1349`.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

/// Lower `src` (no sema types), asserting it parses and lowers with no errors.
#[track_caller]
fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_zir, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "expected clean lowering, got: {errs:?}");
}

#[test]
fn for_over_bytes_literal_lowers() {
    // The issue's exact reduction.
    lower_ok(
        "def main():\n\
        \x20   auto data = b\"UTC\"\n\
        \x20   int64 total = 0\n\
        \x20   for byte in data:\n\
        \x20       total += byte\n\
        \x20   assert total == 236\n",
    );
}

#[test]
fn for_over_bytes_param_lowers() {
    // The datetime-utils shape: a function iterating a `bytes` argument. The
    // loop variable must type as `int64` (it is compared/added as one) with no
    // sema `TypeMap` — i.e. from mono's inferer alone.
    lower_ok(
        "def scan(bytes token) -> int64:\n\
        \x20   int64 n = 0\n\
        \x20   for b in token:\n\
        \x20       if b < 128:\n\
        \x20           n += b\n\
        \x20   return n\n",
    );
}

#[test]
fn for_over_bytes_from_str_lowers() {
    lower_ok(
        "def main():\n\
        \x20   for b in bytes(\"hi\"):\n\
        \x20       print(b)\n",
    );
}

#[test]
fn for_over_empty_bytes_lowers() {
    lower_ok(
        "def main():\n\
        \x20   int64 n = 0\n\
        \x20   for b in bytes():\n\
        \x20       n += b\n\
        \x20   assert n == 0\n",
    );
}

#[test]
fn bytes_loop_variable_is_an_int_index_lowers() {
    // Using the loop variable as a list index exercises that it lowers as an
    // `int64` (an index must be integral), not a `str`/`char` (#1074 hazard).
    lower_ok(
        "def main():\n\
        \x20   list[int64] table = [0, 0, 0, 0, 0]\n\
        \x20   for b in b\"abc\":\n\
        \x20       print(table[b % 5])\n",
    );
}

#[test]
fn nested_bytes_loops_lower() {
    // A bytes loop nested inside another, each binding its own `int64` byte.
    lower_ok(
        "def main():\n\
        \x20   int64 total = 0\n\
        \x20   for a in b\"ab\":\n\
        \x20       for c in b\"cd\":\n\
        \x20           total += a + c\n\
        \x20   assert total > 0\n",
    );
}

#[test]
fn bytes_loop_inside_generator_lowers() {
    // A `yield` inside the bytes loop forces the index/handle/length onto the
    // generator state struct (I#453); the loop must still lower cleanly.
    lower_ok(
        "def each(bytes data):\n\
        \x20   for b in data:\n\
        \x20       yield b\n\
        def main():\n\
        \x20   int64 total = 0\n\
        \x20   for b in each(b\"UTC\"):\n\
        \x20       total += b\n\
        \x20   assert total == 236\n",
    );
}
