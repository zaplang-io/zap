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

// --- #1493 part A: deref of a null built-in reference VALUE (list/str/dict/…) ---
// A non-`Optional` reference field left null, then subscripted / iterated / sized.
// Each program reads the null value through a field (so a FieldRead guard also
// appears for the owner); the assertions pin the VALUE-site message specifically.

#[test]
fn index_on_null_builtin_reference_guards_index_base() {
    let src = "struct L:\n    list[int64] xs\n\ndef main():\n    L l = None\n    auto v = l.xs[0]\n    print(v)\n";
    assert!(
        messages(src)
            .iter()
            .any(|m| m == "null dereference indexing into 'list' at <unknown>:0"),
        "indexing a null list must carry the IndexBase message; got {:?}",
        messages(src)
    );
}

#[test]
fn index_on_null_str_guards_index_base() {
    let src = "struct S:\n    str name\n\ndef main():\n    S s = None\n    auto c = s.name[0]\n    print(c)\n";
    assert!(
        messages(src)
            .iter()
            .any(|m| m == "null dereference indexing into 'str' at <unknown>:0"),
        "indexing a null str must carry the IndexBase message; got {:?}",
        messages(src)
    );
}

#[test]
fn iterate_null_builtin_reference_guards_iter_base() {
    let src = "struct L:\n    list[int64] xs\n\ndef main():\n    L l = None\n    for x in l.xs:\n        print(x)\n";
    assert!(
        messages(src)
            .iter()
            .any(|m| m == "null dereference iterating over 'list' at <unknown>:0"),
        "iterating a null list must carry the IterBase message; got {:?}",
        messages(src)
    );
}

#[test]
fn len_of_null_builtin_reference_guards_builtin_len() {
    // The Epic's headline example: `len(n.name)` on a null `str`.
    let src = "struct N:\n    str name\n\ndef main():\n    N n = None\n    auto k = len(n.name)\n    print(k)\n";
    assert!(
        messages(src)
            .iter()
            .any(|m| m == "null dereference taking the length of 'str' at <unknown>:0"),
        "len() of a null str must carry the BuiltinLen message; got {:?}",
        messages(src)
    );
}

#[test]
fn len_of_null_list_guards_builtin_len() {
    let src = "struct L:\n    list[int64] xs\n\ndef main():\n    L l = None\n    auto k = len(l.xs)\n    print(k)\n";
    assert!(
        messages(src)
            .iter()
            .any(|m| m == "null dereference taking the length of 'list' at <unknown>:0"),
        "len() of a null list must carry the BuiltinLen message; got {:?}",
        messages(src)
    );
}

// --- #1493 part B: method-call receiver on a null reference ---

#[test]
fn method_call_on_null_user_struct_guards_receiver() {
    // #1541: a method call on a null user reference-struct receiver is guarded
    // eagerly at the call site.
    let src = "struct Counter:\n    int64 n\n\n    def bump(self):\n        self.n = self.n + 1\n\ndef main():\n    Counter c = None\n    c.bump()\n";
    assert!(
        messages(src)
            .iter()
            .any(|m| m == "null dereference calling method 'bump' on 'Counter' at <unknown>:0"),
        "a null user-struct method receiver must carry the MethodReceiver message; got {:?}",
        messages(src)
    );
}

#[test]
fn enum_method_receiver_is_not_guarded() {
    // #1541: a tier-3 sum enum is a heap struct whose unit variants lower to a
    // null pointer (a legal value), so an enum method receiver must NOT be
    // guarded — guarding it would make `E.Nothing.m()` spuriously raise.
    let src = "enum E:\n    Nothing\n    Wrap(int64 v)\n\n    def tag(self) -> int64:\n        return 0\n\ndef main():\n    auto e = E.Nothing\n    print(e.tag())\n";
    // Specific to the receiver: a guard would read `calling method 'tag' on 'E'`.
    // (A blanket "no null dereference" check would trip over field-read guards in
    // unrelated prelude methods that survive monomorphization.)
    assert!(
        !messages(src)
            .iter()
            .any(|m| m.contains("calling method 'tag'")),
        "an enum method receiver must not be guarded; got {:?}",
        messages(src)
    );
}

#[test]
fn optional_method_receiver_is_not_guarded() {
    // #1541: `Optional[T]` keeps its own None path; its method receiver must NOT
    // be guarded (guarding would make `opt.is_none()` spuriously raise).
    let src = "def main():\n    Optional[int64] o = None\n    print(o.is_none())\n";
    // Specific to the receiver: a guard would read `calling method 'is_none' on
    // 'Optional$…'`. (A blanket check would trip over prelude field-read guards.)
    assert!(
        !messages(src)
            .iter()
            .any(|m| m.contains("calling method 'is_none'")),
        "an Optional method receiver must not be guarded; got {:?}",
        messages(src)
    );
}

#[test]
fn method_call_on_null_builtin_list_guards_receiver() {
    let src =
        "struct L:\n    list[int64] xs = None\n\ndef main():\n    L l = L()\n    l.xs.append(5)\n";
    assert!(
        messages(src)
            .iter()
            .any(|m| m == "null dereference calling method 'append' on 'list' at <unknown>:0"),
        "method call on a null list must carry the MethodReceiver message; got {:?}",
        messages(src)
    );
}

#[test]
fn method_call_on_null_str_guards_receiver() {
    let src = "struct S:\n    str name = None\n\ndef main():\n    S s = S()\n    auto u = s.name.upper()\n    print(u)\n";
    assert!(
        messages(src)
            .iter()
            .any(|m| m == "null dereference calling method 'upper' on 'str' at <unknown>:0"),
        "method call on a null str must carry the MethodReceiver message; got {:?}",
        messages(src)
    );
}

#[test]
fn value_struct_method_call_is_not_guarded_as_receiver() {
    // A `value struct` is inline, never null; its method call must not emit a
    // null-dereference guard (and `IsNull` on an inline aggregate would be wrong).
    let src = "value struct V:\n    int64 x\n\n    def get(self) -> int64:\n        return self.x\n\ndef main():\n    V v = V(x=1)\n    print(v.get())\n";
    assert!(
        !messages(src)
            .iter()
            .any(|m| m.starts_with("null dereference")),
        "a value-struct method call must emit no null-dereference message; got {:?}",
        messages(src)
    );
}
