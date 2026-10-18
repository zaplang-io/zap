//! ZIR — Zap's typed SSA intermediate representation, plus AST -> ZIR lowering.
//!
//! This crate has no LLVM dependency, so it builds and tests on any host. The
//! backend (`zapc_codegen`) consumes [`Module`] and translates ZIR -> LLVM.
//! See `design/IR.md`.

pub mod ir;
pub mod lower;

pub use ir::{
    BinOp, Block, BlockId, CastKind, CmpPred, Dtype, Func, Inst, Module, Term, Ty, UnOp, Value,
};
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
    fn lower_exceptions() {
        // raise + try/except/finally lower to invoke/landingpad exception edges
        // and the runtime unwinding entry points (#22).
        let m = lower_ok(include_str!("../../../tests/golden/exceptions_catch.zap"));
        let text = format!("{m}");
        assert!(text.contains("invoke @zap_panic"), "text:\n{text}");
        assert!(text.contains("landingpad"), "text:\n{text}");
        assert!(text.contains("call @zap_eh_end_catch"), "text:\n{text}");
        // An exception raised in a callee reaches a `try` in `main` via an
        // `invoke` of the callee (real cross-frame unwinding).
        let m = lower_ok(include_str!(
            "../../../tests/golden/exceptions_propagate.zap"
        ));
        let text = format!("{m}");
        assert!(text.contains("invoke @boom"), "text:\n{text}");
        // An uncaught raise routes to the implicit top-level handler in `main`.
        let m = lower_ok(include_str!(
            "../../../tests/golden/exceptions_uncaught.zap"
        ));
        let text = format!("{m}");
        assert!(text.contains("call @zap_uncaught"), "text:\n{text}");
    }

    #[test]
    fn lower_arrays() {
        // Construction, element access, len, and sum lower to the array runtime
        // entry points; element-wise ops lower to the dedicated ArrayBinOp node.
        let m = lower_ok(include_str!("../../../tests/golden/array_int.zap"));
        let text = format!("{m}");
        assert!(text.contains("call @zap_array_new"), "text:\n{text}");
        assert!(text.contains("call @zap_array_set_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_array_sum_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_array_len"), "text:\n{text}");
        assert!(text.contains("call @zap_array_get_i64"), "text:\n{text}");
        assert!(text.contains("array.Add.i64"), "text:\n{text}");
        assert!(text.contains("array.Mul.i64"), "text:\n{text}");
        assert!(text.contains("array.Sub.i64"), "text:\n{text}");
        // f64 arrays select the f64-suffixed entry points.
        let m = lower_ok(include_str!("../../../tests/golden/array_f64.zap"));
        let text = format!("{m}");
        assert!(text.contains("call @zap_array_sum_f64"), "text:\n{text}");
        assert!(text.contains("array.Mul.f64"), "text:\n{text}");
    }

    #[test]
    fn array_scalar_on_left_only_commutative() {
        // `scalar + array` is accepted (reordered); `scalar - array` is not.
        let (ast, _, _) =
            parse_str("def main():\n    auto a = array[i64]([1, 2])\n    print(sum(3 - a))\n");
        let (_m, errs) = lower_module(&ast);
        assert!(!errs.is_empty(), "scalar - array should be rejected");
    }

    #[test]
    fn lower_scalars_fixture() {
        // The T9 scalar tower (ticket #19): int32/float32 slots, conversions,
        // `**`, and `//` all lower to the expected ops/casts/runtime calls.
        let m = lower_ok(include_str!("../../../tests/golden/scalars.zap"));
        let text = format!("{m}");
        // int32 / float32 values get narrow slots (not everything is i64/f64).
        assert!(text.contains("alloca i32"), "text:\n{text}");
        assert!(text.contains("alloca f32"), "text:\n{text}");
        // `scale` is typed int32 -> int32.
        let scale = m.funcs.iter().find(|f| f.name == "scale").expect("scale");
        assert_eq!(scale.params, vec![Ty::I32]);
        assert_eq!(scale.ret, Ty::I32);
        // Conversions and widening use the cast kinds.
        assert!(text.contains("Trunc"), "text:\n{text}"); // int64 -> int32
        assert!(text.contains("FPToSI"), "text:\n{text}"); // float -> int
        assert!(text.contains("SIToFP"), "text:\n{text}"); // int -> float
        assert!(text.contains("FPExt"), "text:\n{text}"); // float32 -> float64
                                                          // Power and floor-division lower to runtime helpers.
        assert!(text.contains("call @zap_ipow"), "text:\n{text}");
        assert!(text.contains("call @zap_powf"), "text:\n{text}");
        assert!(text.contains("call @zap_ifloordiv"), "text:\n{text}");
        assert!(text.contains("call @zap_ffloordiv"), "text:\n{text}");
    }

    #[test]
    fn pow_lowers_by_operand_kind() {
        // All-integer `**` -> zap_ipow; any float operand -> zap_powf.
        let m = lower_ok("def main():\n    print(2 ** 10)\n");
        assert!(format!("{m}").contains("call @zap_ipow"));
        let m = lower_ok("def main():\n    print(2.0 ** 3.0)\n");
        assert!(format!("{m}").contains("call @zap_powf"));
    }

    #[test]
    fn floordiv_lowers_by_operand_kind() {
        // `//` never lowers to a bare sdiv/fdiv — it always goes through the
        // floor-correcting runtime helper (D-0024).
        let m = lower_ok("def main():\n    print(7 // 2)\n");
        let text = format!("{m}");
        assert!(text.contains("call @zap_ifloordiv"), "text:\n{text}");
        assert!(!text.contains("= Div "), "no raw div for int //:\n{text}");
        let m = lower_ok("def main():\n    print(7.0 // 2.0)\n");
        assert!(format!("{m}").contains("call @zap_ffloordiv"));
    }

    #[test]
    fn explicit_conversion_lowers_to_cast() {
        // `int32(x)` narrows (Trunc); `float64(x)` crosses to float (SIToFP).
        let m = lower_ok("def main():\n    int64 b = 300\n    int32 c = int32(b)\n    print(c)\n");
        assert!(format!("{m}").contains("Trunc"));
        let m = lower_ok("def main():\n    int32 a = 7\n    print(float64(a))\n");
        assert!(format!("{m}").contains("SIToFP"));
    }

    #[test]
    fn unsupported_construct_errors() {
        let (ast, _p, _l) = parse_str("def main():\n    raise Oops\n");
        let (_m, errs) = lower_module(&ast);
        assert!(!errs.is_empty());
    }

    // --- decorators & annotations (ticket #16, D-0036) ---

    #[test]
    fn lower_decorators_fixture() {
        // The golden program exercises the whole feature: `@derive(Eq, Repr)`
        // generates methods, `@Table` is reflected, and `reflect_*` queries lower
        // to the runtime registry.
        let m = lower_ok(include_str!("../../../tests/golden/decorators.zap"));
        // `@derive(Eq, Repr)` produced two extra top-level functions.
        let eq = m.funcs.iter().find(|f| f.name == "User__eq").expect("eq");
        assert_eq!(eq.params, vec![Ty::Struct(0), Ty::Struct(0)]);
        assert_eq!(eq.ret, Ty::I1);
        let repr = m
            .funcs
            .iter()
            .find(|f| f.name == "User__repr")
            .expect("repr");
        assert_eq!(repr.params, vec![Ty::Struct(0)]);
        assert_eq!(repr.ret, Ty::Ptr);

        let text = format!("{m}");
        // Eq compares an int field (icmp) and a str field (runtime content eq).
        assert!(text.contains("call @zap_str_eq"), "text:\n{text}");
        // Repr stringifies the int field and concatenates.
        assert!(text.contains("call @zap_i64_to_str"), "text:\n{text}");
        // The reflection registry is populated at the top of `main`, and the
        // user annotation `@Table` (not the compile-time `@derive`) is recorded.
        assert!(
            text.contains("strconst \"Table\""),
            "the user annotation should be registered, text:\n{text}"
        );
        assert!(
            !text.contains("strconst \"derive\""),
            "the compile-time @derive must not reach the runtime registry"
        );
        assert!(text.contains("call @zap_reflect_register"), "text:\n{text}");
        assert!(text.contains("call @zap_reflect_has"), "text:\n{text}");
        assert!(text.contains("call @zap_reflect_names"), "text:\n{text}");
    }

    #[test]
    fn derive_eq_only_methods_are_callable() {
        // A struct can derive just one trait; the generated method resolves.
        let m = lower_ok(
            "@derive(Eq)\nstruct P:\n    int64 x\n\ndef main():\n    \
             auto a = P(x=1)\n    auto b = P(x=1)\n    if a.eq(b):\n        print(1)\n",
        );
        assert!(m.funcs.iter().any(|f| f.name == "P__eq"));
        assert!(!m.funcs.iter().any(|f| f.name == "P__repr"));
    }

    #[test]
    fn deferred_builtins_error() {
        // Built-ins in the locked set but not yet generated report cleanly.
        for src in [
            "@derive(Json)\nstruct S:\n    int64 x\n",
            "struct S:\n    @validate(max_len=9)\n    str x\n",
        ] {
            let (ast, _p, _l) = parse_str(src);
            let (_m, errs) = lower_module(&ast);
            assert!(
                !errs.is_empty(),
                "expected a deferred-feature error for:\n{src}"
            );
        }
    }

    #[test]
    fn derive_and_user_method_clash_errors() {
        let (ast, _p, _l) = parse_str(
            "@derive(Eq)\nstruct S:\n    int64 x\n\n    def eq(self, S other) -> bool:\n        return True\n",
        );
        let (_m, errs) = lower_module(&ast);
        assert!(
            !errs.is_empty(),
            "a user `eq` plus @derive(Eq) should clash"
        );
    }
}
