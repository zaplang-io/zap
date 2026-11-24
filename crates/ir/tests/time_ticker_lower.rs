//! `time.ticker(d) -> Ticker` — lowering (ZIR) checks (#1409).
//!
//! `time.ticker` assembles a stoppable periodic timer from existing primitives:
//! two cap-1 `channel[int64]`s (`zap_chan_new`), a DETACHED spawn of the prelude
//! firing loop `__ticker_fire`, and the prelude `Ticker` handle struct wrapping
//! the two channels. The firing loop, the drop-on-slow send, and `stop()` live in
//! the Zap prelude (`PRELUDE_SRC`), so lowering a `time.ticker` program also
//! exercises that prelude source parsing and lowering cleanly.
//!
//! These checks run on the host (no LLVM): no lower errors, the detached
//! `__ticker_fire` spawn, the two channel allocations, and the `select` idiom
//! over `tk.chan` are the guards. End-to-end behavior (firing, stop, drop,
//! fd-less concurrency) is covered by the `time_ticker_*` goldens on CI.

use zapc_ir::{lower_module, Inst, Module, Owner};
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting no lex/parse/lower errors, returning the ZIR.
/// This also asserts the built-in prelude (which now carries `Ticker` and
/// `__ticker_fire`) parses and lowers cleanly, since it is prepended here.
fn lower_ok(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    module
}

/// Every runtime/user symbol named by a `Call`/`Invoke` anywhere in the module.
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

/// Every function symbol spawned (with its ownership) anywhere in the module.
fn spawns(m: &Module) -> Vec<(String, Owner)> {
    let mut out = Vec::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Spawn { func, owner, .. } = inst {
                    out.push((func.clone(), *owner));
                }
            }
        }
    }
    out
}

/// `time.ticker(d)` lowers to: two `zap_chan_new` channel allocations, a DETACHED
/// spawn of the prelude firing loop `__ticker_fire`, and the `Ticker` handle; a
/// receive from `tk.chan` and a `tk.stop()` method call both lower cleanly.
#[test]
fn ticker_lowers_to_detached_firing_loop_and_handle() {
    let m = lower_ok(
        "import time\n\
        def main():\n\
        \x20   auto tk = time.ticker(time.millis(10))\n\
        \x20   auto fired = <-tk.chan\n\
        \x20   tk.stop()\n\
        \x20   print(fired)\n",
    );
    let sp = spawns(&m);
    assert!(
        sp.iter()
            .any(|(f, o)| f == "__ticker_fire" && matches!(o, Owner::Detached)),
        "time.ticker must spawn __ticker_fire detached, spawns: {sp:?}"
    );
    let syms = called_symbols(&m);
    assert!(
        syms.iter().filter(|s| *s == "zap_chan_new").count() >= 2,
        "time.ticker must allocate the tick and done channels, symbols: {syms:?}"
    );
}

/// The `select` timeout/stop idiom over a ticker channel lowers through
/// `zap_select` alongside a `done` channel.
#[test]
fn ticker_in_select_lowers() {
    let m = lower_ok(
        "import time\n\
        def main():\n\
        \x20   auto tk = time.ticker(time.millis(10))\n\
        \x20   auto done = channel[int64](1)\n\
        \x20   select:\n\
        \x20       case t = <-tk.chan:\n\
        \x20           print(t)\n\
        \x20       case d = <-done:\n\
        \x20           print(d)\n",
    );
    assert!(
        called_symbols(&m).iter().any(|s| s == "zap_select"),
        "the ticker select idiom must lower through zap_select"
    );
    // Even in the select program, the ticker still spawns its detached loop.
    assert!(spawns(&m).iter().any(|(f, _)| f == "__ticker_fire"));
}
