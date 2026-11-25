//! Null-dereference guards at the field read/store sites (#1489 Epic, #1493).
//!
//! Host-only (AST→ZIR): these pin that a plain (non-`Optional`) reference field
//! read/store lowers an explicit null check (`Inst::IsNull`) branching to a
//! raising `zap_panic_typed` with the `NullReferenceError` id (22), carrying the
//! located message shape from #1492 — and that a `value struct` access (inline,
//! never null) emits NO such guard. The end-to-end catch behaviour and the
//! resolved source line are pinned by the `none_access_*` / `null_deref_field_*`
//! goldens on the LLVM CI matrix; `lower_module` here threads no source, so the
//! located message degrades to `<unknown>:0`.

use crate::lower::lower_module;
use zapc_ir_core::Inst;

/// Every `StrConst` message string lowered in the module.
fn messages(src: &str) -> Vec<String> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    let (m, errs) = lower_module(&module);
    assert!(errs.is_empty(), "unexpected lowering errors: {errs:?}");
    m.funcs
        .iter()
        .flat_map(|f| f.blocks.iter())
        .flat_map(|b| b.insts.iter())
        .filter_map(|i| match i {
            Inst::StrConst { bytes, .. } => Some(String::from_utf8_lossy(bytes).into_owned()),
            _ => None,
        })
        .collect()
}

/// Whether the module raises `zap_panic_typed` (a `Call` or throwing `Invoke`)
/// AND emits at least one `IsNull` test AND an `iconst 22` (the
/// `ZAP_EXC_NULL_REFERENCE_ERROR` id) — the three pieces of the explicit guard.
fn guard_pieces(src: &str) -> (bool, bool, bool) {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    let (m, errs) = lower_module(&module);
    assert!(errs.is_empty(), "unexpected lowering errors: {errs:?}");
    let mut raises = false;
    let mut is_null = false;
    let mut id_22 = false;
    for f in &m.funcs {
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Call { func, .. } | Inst::Invoke { func, .. }
                        if func == "zap_panic_typed" =>
                    {
                        raises = true;
                    }
                    Inst::IsNull { .. } => is_null = true,
                    Inst::IConst { val, .. } if *val == 22 => id_22 = true,
                    _ => {}
                }
            }
        }
    }
    (raises, is_null, id_22)
}

const POINT: &str = "struct Point:\n    int64 x\n    int64 y\n\n";
const BOX: &str = "struct Box:\n    int64 v\n\n";

#[test]
fn field_read_on_null_reference_guards_with_located_nullreferenceerror() {
    let src = format!("{POINT}def main():\n    Point p = None\n    auto v = p.x\n    print(v)\n");
    let (raises, is_null, id_22) = guard_pieces(&src);
    assert!(raises, "field read must raise zap_panic_typed");
    assert!(is_null, "field read must emit an IsNull check");
    assert!(
        id_22,
        "field read must raise the NullReferenceError id (22)"
    );
    assert!(
        messages(&src)
            .iter()
            .any(|m| m == "null dereference reading field 'x' of 'Point' at <unknown>:0"),
        "field read must carry the located read message; got {:?}",
        messages(&src)
    );
}

#[test]
fn field_store_on_null_reference_guards_with_located_nullreferenceerror() {
    let src = format!("{BOX}def main():\n    Box b = None\n    b.v = 5\n");
    let (raises, is_null, id_22) = guard_pieces(&src);
    assert!(raises, "field store must raise zap_panic_typed");
    assert!(is_null, "field store must emit an IsNull check");
    assert!(
        id_22,
        "field store must raise the NullReferenceError id (22)"
    );
    assert!(
        messages(&src)
            .iter()
            .any(|m| m == "null dereference writing field 'v' of 'Box' at <unknown>:0"),
        "field store must carry the located write message; got {:?}",
        messages(&src)
    );
}

#[test]
fn value_struct_field_access_is_not_guarded() {
    // A `value struct` is inline and never null, so neither a read nor a store
    // emits a null-deref guard (and `IsNull` on an inline aggregate would be
    // wrong).
    let read = "value struct V:\n    int64 x\n\ndef main():\n    V v = V(x=1)\n    auto a = v.x\n    print(a)\n";
    let (raises, _is_null, id_22) = guard_pieces(read);
    assert!(
        !raises,
        "a value-struct read must not raise zap_panic_typed"
    );
    assert!(
        !id_22,
        "a value-struct read must not raise NullReferenceError"
    );
    assert!(
        !messages(read)
            .iter()
            .any(|m| m.starts_with("null dereference")),
        "a value-struct read must emit no null-dereference message"
    );

    let store = "value struct V:\n    int64 x\n\ndef main():\n    V v = V(x=1)\n    v.x = 2\n    print(v.x)\n";
    let (raises, _is_null, id_22) = guard_pieces(store);
    assert!(
        !raises,
        "a value-struct store must not raise zap_panic_typed"
    );
    assert!(
        !id_22,
        "a value-struct store must not raise NullReferenceError"
    );
}

#[test]
fn non_null_reference_still_emits_the_guard_once_per_access() {
    // The guard is unconditional on the static reference type (null-ness is a
    // runtime property), so even a provably non-null construction emits one
    // check per field access — correctness floor over cleverness.
    let src = format!(
        "{POINT}def main():\n    Point p = Point(x=1, y=2)\n    auto v = p.x\n    print(v)\n"
    );
    let (raises, is_null, _id) = guard_pieces(&src);
    assert!(
        raises && is_null,
        "a reference field read is always guarded"
    );
}
