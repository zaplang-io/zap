//! `bool` widens to a wider integer by zero-extension — lowering check for #1344.
//!
//! A `bool` is an unsigned 0/1. Before the fix, `coerce` widened it to `int64`
//! with `SExt`, so `True` (the i1 value `1`) became `-1`, which inverted every
//! `bool` ordering comparison (`False < True` promoted to `0 < -1` = False).
//! This host-only check (no LLVM) asserts the widening is now `ZExt`, never
//! `SExt`; the end-to-end values are covered by the `bool_widening` golden.

use zapc_ir::{lower_module, CastKind, Inst, Module};
use zapc_syntax::parser::parse_str;

fn lower_ok(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    module
}

fn cast_kinds(m: &Module) -> Vec<CastKind> {
    let mut out = Vec::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Cast { kind, .. } = inst {
                    out.push(*kind);
                }
            }
        }
    }
    out
}

#[test]
fn bool_lt_widens_by_zero_extension() {
    // `a < b` on two bools widens each i1 to i64 before the integer compare. That
    // widening must be ZExt (True -> 1), never SExt (True -> -1).
    let m = lower_ok(
        "def main():\n\
        \x20   bool a = False\n\
        \x20   bool b = True\n\
        \x20   print(str(a < b))\n",
    );
    let kinds = cast_kinds(&m);
    assert!(
        kinds.iter().any(|k| matches!(k, CastKind::ZExt)),
        "bool widening must zero-extend; got {kinds:?}"
    );
    assert!(
        !kinds.iter().any(|k| matches!(k, CastKind::SExt)),
        "bool widening must not sign-extend (that made True = -1); got {kinds:?}"
    );
}

#[test]
fn bool_all_order_ops_widen_by_zext() {
    for op in ["<", "<=", ">", ">="] {
        let m = lower_ok(&format!(
            "def main():\n\
            \x20   bool a = True\n\
            \x20   bool b = False\n\
            \x20   print(str(a {op} b))\n",
        ));
        let kinds = cast_kinds(&m);
        assert!(
            kinds.iter().any(|k| matches!(k, CastKind::ZExt))
                && !kinds.iter().any(|k| matches!(k, CastKind::SExt)),
            "bool `{op}` must zero-extend its operands; got {kinds:?}"
        );
    }
}

#[test]
fn list_bool_ordering_widens_by_zext() {
    // Elements of a `list[bool]` read back as i64 words and are compared via the
    // element `<`; the same zero-extension rule applies, so `[False, True]`
    // orders before `[True, False]`.
    let m = lower_ok(
        "def main():\n\
        \x20   list[bool] a = [False, True]\n\
        \x20   list[bool] b = [True, False]\n\
        \x20   print(str(a < b))\n",
    );
    let kinds = cast_kinds(&m);
    assert!(
        kinds.iter().any(|k| matches!(k, CastKind::ZExt))
            && !kinds.iter().any(|k| matches!(k, CastKind::SExt)),
        "list[bool] ordering must zero-extend bool elements; got {kinds:?}"
    );
}
