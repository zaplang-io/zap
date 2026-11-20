//! `assert x == None` / `assert x != None` on a nullable reference must lower
//! (I#1184). The power-assert decomposition (D-0054) evaluates a comparison's
//! two operands separately so the failure message can show both values — but a
//! bare `None` literal has no standalone lowering (it exists only as the
//! null/empty-variant check the `Expr::Binary` expr lowerer intercepts), so
//! evaluating a `None` operand there bailed with
//! `error[ZAP6001]: this expression is not supported by codegen yet (Phase 1
//! slice)`. `== None` / `!= None` are now excluded from the decomposition and
//! fall through to the whole-expression null check (a real `IsNull` test),
//! exactly as `assert x is None` already did.
//!
//! These exercise the real front-end + AST→ZIR lowering pipeline, so they run
//! on the host (no LLVM). The end-to-end behaviour (the assertions actually
//! pass / fail at native runtime, under `zap run` and `zap test`) is pinned by
//! the `assert_none_field` golden and `assert_none_field_test` test-mode
//! golden.

use zapc_ir::LowerError;

/// Run parse + sema + AST→ZIR lowering, asserting the program lexes, parses, and
/// type-checks cleanly, and return the lowering errors.
fn lower_errors(src: &str) -> Vec<LowerError> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    let serrs = zapc_sema::check(&module);
    assert!(serrs.is_empty(), "unexpected sema errors: {serrs:?}");
    zapc_ir::lower_module(&module).1
}

#[track_caller]
fn assert_lowers(src: &str) {
    let errs = lower_errors(src);
    assert!(
        errs.is_empty(),
        "expected `{src}` to lower cleanly, got lowering errors: {errs:?}"
    );
}

const STRUCTS: &str = "struct Reader:\n    bytes pending\n\n\
                       struct Client:\n    Reader reader = None\n\n";

/// The exact #1184 reproducer: `assert` on an empty nullable reference field.
#[test]
fn assert_struct_field_eq_none_lowers() {
    assert_lowers(&format!(
        "{STRUCTS}def main():\n    auto client = Client()\n    assert client.reader == None\n"
    ));
}

/// `!= None` on the same field (both comparison directions share the path).
#[test]
fn assert_struct_field_ne_none_lowers() {
    assert_lowers(&format!(
        "{STRUCTS}def main():\n    auto client = Client()\n\
         \x20   client.reader = Reader(pending=b\"x\")\n    assert client.reader != None\n"
    ));
}

/// The `is None` form always lowered; keep it covered so the two stay in lockstep.
#[test]
fn assert_struct_field_is_none_lowers() {
    assert_lowers(&format!(
        "{STRUCTS}def main():\n    auto client = Client()\n    assert client.reader is None\n"
    ));
}

/// Every other nullable reference operand of `assert … == None` must lower too,
/// not just a struct field: a local `str`, `bytes`, and a collection.
#[test]
fn assert_other_nullable_operands_eq_none_lower() {
    for body in [
        "    str s = None\n    assert s == None\n",
        "    str s = \"x\"\n    assert s != None\n",
        "    bytes b = None\n    assert b == None\n",
        "    list[int64] xs = None\n    assert xs == None\n",
        "    dict[str, int64] d = None\n    assert d != None\n",
    ] {
        assert_lowers(&format!("def main():\n{body}"));
    }
}

/// An ordinary (non-`None`) comparison is still power-decomposed — the fix only
/// skips `None` operands, so a normal `assert a == b` keeps lowering, and with a
/// custom message.
#[test]
fn assert_ordinary_comparison_still_lowers() {
    assert_lowers("def main():\n    int64 a = 1\n    int64 b = 2\n    assert a == b\n");
    assert_lowers("def main():\n    int64 a = 1\n    assert a < 2\n");
    assert_lowers(
        "struct Reader:\n    bytes pending\n\nstruct Client:\n    Reader reader = None\n\n\
         def main():\n    auto client = Client()\n\
         \x20   assert client.reader == None, \"reader should be empty\"\n",
    );
}
