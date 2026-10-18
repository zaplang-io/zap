//! ZIR — Zap's typed SSA intermediate representation, plus AST -> ZIR lowering.
//!
//! This crate has no LLVM dependency, so it builds and tests on any host. The
//! backend (`zapc_codegen`) consumes [`Module`] and translates ZIR -> LLVM.
//! See `design/IR.md`.

pub mod ir;
pub mod lower;

pub use ir::{BinOp, Block, BlockId, CastKind, CmpPred, Func, Inst, Module, Term, Ty, UnOp, Value};
pub use lower::{lower_module, LowerError};

#[cfg(test)]
mod tests {
    use super::*;
    use zapc_syntax::parser::parse_str;

    fn lower_ok(src: &str) -> Module {
        let (ast, perrs, lerrs) = parse_str(src);
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        let (module, errs) = lower_module(&ast);
        assert!(errs.is_empty(), "lower errors: {errs:?}");
        module
    }

    #[test]
    fn lower_fib_fixture() {
        let m = lower_ok(include_str!("../../../tests/golden/fib.zap"));
        assert_eq!(m.funcs.len(), 2);
        let fib = m.funcs.iter().find(|f| f.name == "fib").expect("fib");
        assert_eq!(fib.params, vec![Ty::I64]);
        assert_eq!(fib.ret, Ty::I64);
        let main = m.funcs.iter().find(|f| f.name == "main").expect("main");
        assert!(main.is_main);
        // Textual form mentions the recursive call and a comparison.
        let text = format!("{m}");
        assert!(text.contains("call @fib"), "text was:\n{text}");
        assert!(text.contains("icmp"), "text was:\n{text}");
    }

    #[test]
    fn lower_loop_sum_fixture() {
        let m = lower_ok(include_str!("../../../tests/golden/loop_sum.zap"));
        assert_eq!(m.funcs.len(), 1);
        let text = format!("{m}");
        assert!(text.contains("print("), "text was:\n{text}");
        assert!(text.contains("Add"), "text was:\n{text}");
    }

    #[test]
    fn print_and_arithmetic() {
        let m = lower_ok("def main():\n    print(1 + 2 * 3)\n");
        let main = &m.funcs[0];
        let has_print = main
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .any(|i| matches!(i, Inst::Print { .. }));
        assert!(has_print);
    }

    #[test]
    fn lower_strings() {
        let m = lower_ok(include_str!("../../../tests/golden/hello.zap"));
        let text = format!("{m}");
        assert!(text.contains("strconst"), "text:\n{text}");
        assert!(text.contains("call @zap_str_concat"), "text:\n{text}");
        // `str` params/returns are pointers.
        let greet = m.funcs.iter().find(|f| f.name == "greet").expect("greet");
        assert_eq!(greet.params, vec![Ty::Ptr]);
        assert_eq!(greet.ret, Ty::Ptr);
    }

    #[test]
    fn lower_structs() {
        let m = lower_ok(include_str!("../../../tests/golden/point.zap"));
        assert_eq!(m.structs.len(), 1);
        assert_eq!(m.structs[0].name, "Point");
        assert_eq!(m.structs[0].fields, vec![Ty::F64, Ty::F64]);
        assert!(m.funcs.iter().any(|f| f.name == "Point__sumsq"));
        let text = format!("{m}");
        assert!(text.contains("structnew"), "{text}");
        assert!(text.contains("fieldget"), "{text}");
        assert!(text.contains("fieldset"), "{text}");
    }

    #[test]
    fn lower_sets() {
        let m = lower_ok(include_str!("../../../tests/golden/set_int.zap"));
        let text = format!("{m}");
        // Literal construction, add, membership, len, and snapshot-iteration all
        // lower to the set runtime entry points.
        assert!(text.contains("call @zap_set_new"), "text:\n{text}");
        assert!(text.contains("call @zap_set_add_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_set_contains_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_set_len"), "text:\n{text}");
        assert!(text.contains("call @zap_set_items"), "text:\n{text}");
    }

    #[test]
    fn unsupported_construct_errors() {
        let (ast, _p, _l) = parse_str("def main():\n    raise Oops\n");
        let (_m, errs) = lower_module(&ast);
        assert!(!errs.is_empty());
    }
}
