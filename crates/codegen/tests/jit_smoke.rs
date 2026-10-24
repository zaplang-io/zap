//! I#159 REPL JIT engine smoke test: validates that MCJIT can run real Zap
//! programs end-to-end — printing, GC allocation under pressure, and (the key
//! risk) exception unwinding through JIT-compiled frames. If MCJIT did not
//! register the module's unwind tables, the `try/except` below would abort the
//! process instead of returning 0.
//!
//! Host-only: needs LLVM + a C toolchain, so it runs in the dev container.

use zapc_codegen::jit::ReplJit;
use zapc_syntax::parser::parse_str;

fn parse(src: &str) -> zapc_syntax::ast::Module {
    let (module, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    module
}

#[test]
fn jit_runs_print_gc_and_exception_unwinding() {
    let src = "\
def boom(int64 n) -> int64:
    if n > 0:
        raise \"boom\"
    return n

def main() -> int64:
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
    return 0
";
    let module = parse(src);
    let mut jit = ReplJit::new().expect("start JIT session");
    let code = jit.run_main(&module).expect("run main under the JIT");
    assert_eq!(code, 0, "main should return 0 (exception caught, no abort)");
}
