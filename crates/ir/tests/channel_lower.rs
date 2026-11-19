//! `channel[T]` element read-back — lowering (ZIR) checks (#1100).
//!
//! A value received from a typed `channel[T]` must come back with `T`'s real
//! type, not an opaque pointer. The channel constructor used to classify its
//! element from a bare *name*, collapsing every reference element (`str`,
//! `bytes`, a struct, a nested collection, …) to `Elem::Ptr`. A `channel[Entry]`
//! value then read back as a bare pointer (`str`), so field access on the
//! received value failed to lower with `ZAP6001 "field access on a non-struct
//! value"` — even though the front end accepted the program. The fix routes
//! channel elements through the shared value-position classifier so every
//! element kind keeps its precise `Elem`, exactly like a list element or a dict
//! value (#625/#1074).
//!
//! These checks run on the host (no LLVM): the regression is a *lowering* error,
//! so `lower_module` returning no errors — and the received value carrying the
//! right type — is the guard. The end-to-end printed values are covered by the
//! `channel_struct*` goldens on CI.

use zapc_ir::{lower_module, Inst, Module, Ty};
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting no lex/parse/lower errors, returning the ZIR.
/// Before the #1100 fix the struct/nested-element programs here produced a
/// `field access on a non-struct value` *lower* error, so this assertion alone
/// is the core regression guard.
fn lower_ok(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    module
}

/// Parse + lower `src`, returning the lower-error messages (empty on success).
fn lower_errs(src: &str) -> Vec<String> {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_m, errs) = lower_module(&ast);
    errs.iter().map(|e| e.message.clone()).collect()
}

/// The ZIR type of the value produced by the first `Call`/`Invoke` to `symbol`
/// anywhere in the module (its `dest`), looked up through the owning function's
/// value-type table.
fn recv_result_ty(m: &Module, symbol: &str) -> Option<Ty> {
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                let dest = match inst {
                    Inst::Call { func, dest, .. } | Inst::Invoke { func, dest, .. }
                        if func == symbol =>
                    {
                        *dest
                    }
                    _ => continue,
                };
                if let Some(d) = dest {
                    return Some(f.value_ty(d));
                }
            }
        }
    }
    None
}

/// Every runtime symbol named by a `Call`/`Invoke` anywhere in the module.
fn called_symbols(m: &Module) -> Vec<String> {
    let mut out = Vec::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                match inst {
                    Inst::Call { func, .. } | Inst::Invoke { func, .. } => out.push(func.clone()),
                    _ => {}
                }
            }
        }
    }
    out
}

/// The exact issue #1100 reproducer: a struct received from `channel[Entry]`,
/// then a field read. Before the fix this failed to lower (`field access on a
/// non-struct value`). The received value must be `Ty::Struct`, delivered over
/// the pointer channel entry points.
#[test]
fn struct_channel_recv_keeps_struct_type() {
    let m = lower_ok(
        "struct Entry:\n\
        \x20   int amount\n\
        def main():\n\
        \x20   auto inbox = channel[Entry](1)\n\
        \x20   inbox <- Entry(42)\n\
        \x20   auto entry = <-inbox\n\
        \x20   print(entry.amount)\n",
    );
    // The received value is the struct, not a bare pointer.
    assert!(
        matches!(recv_result_ty(&m, "zap_chan_recv_ptr"), Some(Ty::Struct(_))),
        "channel[Entry] receive must yield Ty::Struct, got {:?}",
        recv_result_ty(&m, "zap_chan_recv_ptr"),
    );
    let syms = called_symbols(&m);
    assert!(syms.iter().any(|s| s == "zap_chan_send_ptr"));
    assert!(syms.iter().any(|s| s == "zap_chan_recv_ptr"));
}

/// A struct with mixed scalar/reference/nested-struct fields round-trips: every
/// field read lowers, proving the whole struct type (not just its first field)
/// is recovered on receive.
#[test]
fn struct_channel_mixed_fields_all_readable() {
    let m = lower_ok(
        "struct Point:\n\
        \x20   int x\n\
        \x20   int y\n\
        struct Rec:\n\
        \x20   int n\n\
        \x20   str label\n\
        \x20   Point at\n\
        def main():\n\
        \x20   auto c = channel[Rec](1)\n\
        \x20   c <- Rec(7, \"hi\", Point(1, 2))\n\
        \x20   auto r = <-c\n\
        \x20   print(r.n)\n\
        \x20   print(r.label)\n\
        \x20   print(r.at.x)\n\
        \x20   print(r.at.y)\n",
    );
    assert!(matches!(
        recv_result_ty(&m, "zap_chan_recv_ptr"),
        Some(Ty::Struct(_))
    ));
}

/// A channel of a nested collection recovers the *collection* type on receive —
/// `len()` dispatches to `zap_list_len`, not the string length path — exactly
/// the #625/#1074 read-back invariant applied to channels.
#[test]
fn nested_collection_channel_recovers_list_type() {
    let m = lower_ok(
        "def main():\n\
        \x20   auto c = channel[list[int]](1)\n\
        \x20   auto xs = [1, 2, 3]\n\
        \x20   c <- xs\n\
        \x20   auto ys = <-c\n\
        \x20   print(len(ys))\n",
    );
    assert!(matches!(
        recv_result_ty(&m, "zap_chan_recv_ptr"),
        Some(Ty::List(_))
    ));
    let syms = called_symbols(&m);
    assert!(
        syms.iter().any(|s| s == "zap_list_len"),
        "len() on a received list must use zap_list_len, symbols: {syms:?}"
    );
    assert!(
        !syms.iter().any(|s| s.starts_with("zap_str_len")),
        "a received list must not be treated as a string, symbols: {syms:?}"
    );
}

/// A `channel[str]` still reads back as a `str` (`len` uses the codepoint path),
/// so the general fix does not regress the string element.
#[test]
fn str_channel_still_reads_back_as_str() {
    let m = lower_ok(
        "def main():\n\
        \x20   auto c = channel[str](1)\n\
        \x20   c <- \"hello\"\n\
        \x20   auto s = <-c\n\
        \x20   print(len(s))\n",
    );
    assert!(matches!(
        recv_result_ty(&m, "zap_chan_recv_ptr"),
        Some(Ty::Ptr)
    ));
    assert!(called_symbols(&m).iter().any(|s| s == "zap_str_len_cp"));
}

/// A `channel[bytes]` reads back as `bytes` — the #1074 element distinction
/// reaches channels through the shared classifier (byte length, not codepoint).
#[test]
fn bytes_channel_reads_back_as_bytes() {
    let m = lower_ok(
        "def main():\n\
        \x20   auto c = channel[bytes](1)\n\
        \x20   c <- b\"hi\"\n\
        \x20   auto b = <-c\n\
        \x20   print(len(b))\n",
    );
    assert!(matches!(
        recv_result_ty(&m, "zap_chan_recv_ptr"),
        Some(Ty::Bytes)
    ));
    let syms = called_symbols(&m);
    assert!(
        syms.iter().any(|s| s == "zap_str_len"),
        "len() on received bytes must use the byte path zap_str_len, symbols: {syms:?}"
    );
}

/// A scalar `channel[int]` keeps the `i64` word entry points — the fix must not
/// disturb the working scalar case.
#[test]
fn int_channel_still_uses_i64_entry_points() {
    let m = lower_ok(
        "def main():\n\
        \x20   auto c = channel[int](1)\n\
        \x20   c <- 7\n\
        \x20   auto x = <-c\n\
        \x20   print(x)\n",
    );
    assert!(matches!(
        recv_result_ty(&m, "zap_chan_recv_i64"),
        Some(Ty::I64)
    ));
    let syms = called_symbols(&m);
    assert!(syms.iter().any(|s| s == "zap_chan_send_i64"));
    assert!(syms.iter().any(|s| s == "zap_chan_recv_i64"));
}

/// A `channel[float64]` keeps the reinterpreted-`f64` word entry points (#800).
#[test]
fn float_channel_still_uses_f64_entry_points() {
    let m = lower_ok(
        "def main():\n\
        \x20   auto c = channel[float64](1)\n\
        \x20   c <- 1.5\n\
        \x20   auto x = <-c\n\
        \x20   print(x)\n",
    );
    assert!(matches!(
        recv_result_ty(&m, "zap_chan_recv_f64"),
        Some(Ty::F64)
    ));
}

/// An unbuffered struct channel driven across coroutines lowers the same way —
/// the element typing is a property of the channel type, independent of the
/// buffered/unbuffered runtime path (mirrors the Savina sort-result transfer).
#[test]
fn unbuffered_struct_channel_across_coroutines() {
    let m = lower_ok(
        "struct Entry:\n\
        \x20   int amount\n\
        def producer(channel[Entry] ch):\n\
        \x20   ch <- Entry(99)\n\
        def main():\n\
        \x20   auto ch = channel[Entry]()\n\
        \x20   spawn producer(ch)\n\
        \x20   auto got = <-ch\n\
        \x20   print(got.amount)\n",
    );
    assert!(matches!(
        recv_result_ty(&m, "zap_chan_recv_ptr"),
        Some(Ty::Struct(_))
    ));
}

/// A tuple channel element is still rejected cleanly (never an ICE): a tuple is
/// a value aggregate whose channel-ABI boxing is a documented follow-up (#703).
#[test]
fn tuple_channel_element_still_rejected() {
    let errs = lower_errs(
        "def main():\n\
        \x20   auto c = channel[int, int]()\n\
        \x20   print(0)\n",
    );
    assert!(
        errs.iter()
            .any(|e| e.contains("tuple elements are not supported")),
        "expected a clean tuple-element rejection, got {errs:?}"
    );
}
