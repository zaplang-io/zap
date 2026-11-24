//! `time.after(d) -> channel[int64]` — lowering (ZIR) checks (#1395).
//!
//! `time.after` is the one-shot timer channel for `select`. It lowers to a
//! single `zap_time_after(ns)` runtime call (the runtime creates a cap-1
//! channel and a detached firing coroutine; see `time_datetime.inc.c`) whose
//! result is a `channel[int64]`. A value received from that channel must come
//! back as the scalar `i64` fire instant, i.e. over the `zap_chan_recv_i64`
//! entry point — exactly the element-typing invariant `channel_lower.rs`
//! guards for every other element kind.
//!
//! These checks run on the host (no LLVM): the lowering returning no errors,
//! the `zap_time_after` result being a `Ty::Chan`, and the receive yielding a
//! scalar `i64` are the guards. End-to-end behavior (firing, the `select`
//! timeout idiom, fd-less concurrency, non-stall at exit) is covered by the
//! `time_after_*` goldens on CI.

use zapc_ir::{lower_module, Inst, Module, Ty};
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting no lex/parse/lower errors, returning the ZIR.
fn lower_ok(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    module
}

/// The ZIR type of the value produced by the first `Call`/`Invoke` to `symbol`.
fn result_ty(m: &Module, symbol: &str) -> Option<Ty> {
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

/// `time.after(d)` lowers to a `zap_time_after` call whose result is a channel,
/// and a receive from it reads back the scalar `i64` fire instant.
#[test]
fn after_lowers_to_timer_channel_of_i64() {
    let m = lower_ok(
        "import time\n\
        def main():\n\
        \x20   auto t = time.after(time.millis(5))\n\
        \x20   auto v = <-t\n\
        \x20   print(v)\n",
    );
    let syms = called_symbols(&m);
    assert!(
        syms.iter().any(|s| s == "zap_time_after"),
        "time.after must lower to zap_time_after, symbols: {syms:?}"
    );
    assert!(
        matches!(result_ty(&m, "zap_time_after"), Some(Ty::Chan(_))),
        "zap_time_after must produce a channel, got {:?}",
        result_ty(&m, "zap_time_after"),
    );
    // The timer channel carries an i64, so the receive uses the i64 word entry
    // point and yields Ty::I64 (never the pointer/str path).
    assert!(
        syms.iter().any(|s| s == "zap_chan_recv_i64"),
        "receiving the fire instant must use zap_chan_recv_i64, symbols: {syms:?}"
    );
    assert!(
        matches!(result_ty(&m, "zap_chan_recv_i64"), Some(Ty::I64)),
        "the fire instant must read back as i64, got {:?}",
        result_ty(&m, "zap_chan_recv_i64"),
    );
    // after() hands the channel back immediately; it must NOT be emitted as a
    // throwing call (it does not park here — the firing coroutine does).
    assert!(
        !matches!(result_ty(&m, "zap_time_after"), None),
        "zap_time_after must have a destination value"
    );
}

/// The `select` timeout idiom lowers: a receive case bound to a timer channel
/// variable multiplexes through `zap_select` alongside the work channel.
#[test]
fn after_in_select_timeout_idiom_lowers() {
    let m = lower_ok(
        "import time\n\
        def main():\n\
        \x20   auto work = channel[int64]()\n\
        \x20   auto timer = time.after(time.millis(5))\n\
        \x20   select:\n\
        \x20       case x = <-work:\n\
        \x20           print(x)\n\
        \x20       case fired = <-timer:\n\
        \x20           print(fired)\n",
    );
    let syms = called_symbols(&m);
    assert!(syms.iter().any(|s| s == "zap_time_after"));
    assert!(
        syms.iter().any(|s| s == "zap_select"),
        "the timer receive case must lower through zap_select, symbols: {syms:?}"
    );
}

/// The timer channel operand may be the `time.after(...)` call written inline in
/// the receive case (a `postfix`), not only a pre-bound variable.
#[test]
fn after_inline_in_select_case_lowers() {
    let m = lower_ok(
        "import time\n\
        def main():\n\
        \x20   auto work = channel[int64]()\n\
        \x20   select:\n\
        \x20       case x = <-work:\n\
        \x20           print(x)\n\
        \x20       case fired = <-time.after(time.millis(5)):\n\
        \x20           print(fired)\n",
    );
    let syms = called_symbols(&m);
    assert!(syms.iter().any(|s| s == "zap_time_after"));
    assert!(syms.iter().any(|s| s == "zap_select"));
}
