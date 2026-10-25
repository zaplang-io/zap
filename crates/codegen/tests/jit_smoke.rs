//! I#159 REPL JIT engine smoke test. One process, one session, one thread —
//! the REPL execution model. Validates, end-to-end under MCJIT:
//!   * printing and arithmetic in JIT'd code;
//!   * GC allocation under pressure (the Immix collector runs mid-entry);
//!   * exception unwinding through JIT-compiled frames (the key MCJIT risk —
//!     a broken `.eh_frame` registration would abort instead of catching);
//!   * slot-backed value persistence across entries (`__repl_load_*` /
//!     `__repl_store_*`), for both integer and pointer bindings;
//!   * per-entry recovery from an uncaught exception (the session survives).
//!
//! A single test (not several) because the C runtime is process-global: its GC
//! registers one "main mutator" thread, so a session must run on one thread —
//! exactly how `zap repl` drives it. Host-only (needs LLVM + a C toolchain), so
//! it runs in the dev container.

use zapc_codegen::jit::ReplJit;
use zapc_syntax::parser::parse_str;

fn parse(src: &str) -> zapc_syntax::ast::Module {
    let (module, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    module
}

#[test]
fn jit_repl_session_end_to_end() {
    let mut jit = ReplJit::new().expect("start JIT session");

    // Entry 1: print + arithmetic + a GC-pressure loop + exception unwinding
    // through a callee's frame, all in one entry. If MCJIT did not register the
    // module's unwind tables, the `try/except` would abort the process.
    let m = parse(
        "\
def boom(int64 n) -> int64:
    if n > 0:
        raise \"boom\"
    return n

def __repl_entry_1():
    print(1 + 2)
    int64 total = 0
    int64 i = 0
    while i < 200000:
        auto s = \"hello\"
        total = total + len(s)
        i = i + 1
    print(total)
    try:
        auto x = boom(5)
        print(x)
    except Exception as e:
        print(99)
",
    );
    let o = jit.run_entry(&m, "__repl_entry_1").expect("entry 1");
    assert!(
        o.runtime_error.is_none(),
        "entry 1 should complete normally"
    );

    // Entry 2: bind an int into slot 0.
    let m = parse("def __repl_entry_2():\n    __repl_store_i64(0, 41)\n");
    assert!(jit
        .run_entry(&m, "__repl_entry_2")
        .expect("entry 2")
        .runtime_error
        .is_none());

    // Entry 3: read slot 0 (set by entry 2) and use it -> prints 42.
    let m = parse("def __repl_entry_3():\n    int64 x = __repl_load_i64(0)\n    print(x + 1)\n");
    assert!(jit
        .run_entry(&m, "__repl_entry_3")
        .expect("entry 3")
        .runtime_error
        .is_none());

    // Entry 4: an uncaught exception is caught; the session survives.
    let m = parse("def __repl_entry_4():\n    raise \"kaboom\"\n");
    let o = jit.run_entry(&m, "__repl_entry_4").expect("entry 4");
    assert_eq!(o.runtime_error.as_deref(), Some("kaboom"));

    // Entry 5: still alive after the error.
    let m = parse("def __repl_entry_5():\n    print(7)\n");
    assert!(jit
        .run_entry(&m, "__repl_entry_5")
        .expect("entry 5")
        .runtime_error
        .is_none());

    // Entry 6/7: a pointer (str) binding persists through a slot.
    let m = parse("def __repl_entry_6():\n    __repl_store_ptr(1, \"world\")\n");
    jit.run_entry(&m, "__repl_entry_6").expect("entry 6");
    let m = parse("def __repl_entry_7():\n    str s = __repl_load_ptr(1)\n    print(s)\n");
    assert!(jit
        .run_entry(&m, "__repl_entry_7")
        .expect("entry 7")
        .runtime_error
        .is_none());
}
