//! #1355: operations that take a `bytes` value directly — `sum(bytes)`,
//! `sorted(bytes)`, and `x in bytes` membership — must lower, not fail codegen.
//!
//! `bytes` became iterable in #1349 (for/comprehension/genexpr/splat/set via the
//! shared D-0048 protocol), but three operations that consume a `bytes` value
//! *directly* rather than through that protocol still fell through to a codegen
//! error even though sema accepted them (sema's `sum`/`sorted`/`in` typing does
//! not gate on the operand). This closes those:
//!
//!   * `sum(data)`     -> `int64` (reduce the byte values)
//!   * `sorted(data)`  -> `list[int64]` (ascending byte values; `reverse=`/`key=`)
//!   * `b in data`     -> `bool` (int: byte-value membership; bytes: subsequence)
//!
//! Run the real lowering pass (mono + AST->ZIR) without sema's `TypeMap`, so they
//! guard the fix wherever `cargo test` runs (host-only, no LLVM). Values are
//! covered end-to-end by the `tests/golden/bytes_value_ops.zap` golden.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

#[track_caller]
fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_zir, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "expected clean lowering, got: {errs:?}");
}

// --- sum(bytes) ---

#[test]
fn sum_of_bytes_lowers() {
    lower_ok(
        "def main():\n\
        \x20   auto total = sum(b\"UTC\")\n\
        \x20   assert total == 236\n",
    );
}

#[test]
fn sum_of_bytes_with_start_lowers() {
    lower_ok(
        "def main():\n\
        \x20   auto total = sum(b\"UTC\", 4)\n\
        \x20   assert total == 240\n",
    );
}

#[test]
fn sum_of_bytes_param_lowers() {
    lower_ok(
        "def tally(bytes data) -> int64:\n\
        \x20   return sum(data)\n",
    );
}

// --- sorted(bytes) ---

#[test]
fn sorted_of_bytes_lowers() {
    lower_ok(
        "def main():\n\
        \x20   auto xs = sorted(b\"cab\")\n\
        \x20   assert xs == [97, 98, 99]\n",
    );
}

#[test]
fn sorted_of_bytes_reverse_lowers() {
    lower_ok(
        "def main():\n\
        \x20   auto xs = sorted(b\"cab\", reverse=True)\n\
        \x20   assert xs == [99, 98, 97]\n",
    );
}

// --- membership: int in bytes ---

#[test]
fn int_in_bytes_lowers() {
    lower_ok(
        "def main():\n\
        \x20   bool present = 85 in b\"UTC\"\n\
        \x20   assert present\n",
    );
}

#[test]
fn int_not_in_bytes_lowers() {
    lower_ok(
        "def main():\n\
        \x20   bool absent = 0 not in b\"UTC\"\n\
        \x20   assert absent\n",
    );
}

#[test]
fn int_in_bytes_param_lowers() {
    lower_ok(
        "def has_nul(bytes data) -> bool:\n\
        \x20   return 0 in data\n",
    );
}

// --- membership: bytes in bytes (subsequence) ---

#[test]
fn bytes_in_bytes_lowers() {
    lower_ok(
        "def main():\n\
        \x20   bool present = b\"TC\" in b\"UTC\"\n\
        \x20   assert present\n",
    );
}

#[test]
fn bytes_not_in_bytes_lowers() {
    lower_ok(
        "def main():\n\
        \x20   bool absent = b\"xy\" not in b\"UTC\"\n\
        \x20   assert absent\n",
    );
}
