//! ZIR — Zap's typed SSA intermediate representation, plus AST -> ZIR lowering.
//!
//! This crate has no LLVM dependency, so it builds and tests on any host. The
//! backend (`zapc_codegen`) consumes [`Module`] and translates ZIR -> LLVM.
//! See `design/IR.md`.

pub mod ir;
pub mod lower;
/// Monomorphization: turn user-defined generics into concrete declarations
/// before lowering (I#154, D-0017/D-0064).
pub mod mono;

pub use ir::{
    BinOp, Block, BlockId, CastKind, CmpPred, Dtype, Elem, Func, Inst, Module, Owner, Term, Ty,
    UnOp, Value,
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
    fn lower_interface_dispatch_fixture() {
        // Interface values (D-0016, #84): a concrete struct assigned to an
        // interface-typed binding is wrapped (`ifacemake`), and an
        // interface-method call dispatches through the itable (`ifacecall`).
        let m = lower_ok(include_str!("../../../tests/golden/interface_dispatch.zap"));
        // One interface registered, with its synthetic fat-pointer layout and the
        // two method slots in declaration order.
        assert_eq!(m.interfaces.len(), 1, "one interface");
        let shape = &m.interfaces[0];
        assert_eq!(shape.name, "Shape");
        let slots: Vec<&str> = shape.methods.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(
            slots,
            vec!["area", "name"],
            "slot order is declaration order"
        );
        // The fat-pointer layout is the inline `{ptr, ptr}` aggregate.
        let fat = &m.structs[shape.fat_sid as usize];
        assert!(fat.inline, "the fat pointer is passed by value");
        assert_eq!(fat.fields, vec![Ty::Ptr, Ty::Ptr], "{{data, itable}}");
        // The concrete->interface conversion and the dynamic dispatch are emitted.
        let text = format!("{m}");
        assert!(text.contains("ifacemake"), "conversion emitted:\n{text}");
        assert!(
            text.contains("ifacecall"),
            "dynamic dispatch emitted:\n{text}"
        );
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
    fn lower_assert_power_diagnostics() {
        // A comparison assert evaluates each operand once and, in the failure
        // block, renders the source text plus both operand values.
        let m = lower_ok("def main():\n    int64 x = 2\n    assert x == 3\n");
        let text = format!("{m}");
        assert!(text.contains("br_if"), "assert should branch:\n{text}");
        assert!(
            text.contains("\\n  assert x == 3"),
            "power-assert should echo the source:\n{text}"
        );
        assert!(
            text.contains("left:") && text.contains("right:"),
            "power-assert should show both operands:\n{text}"
        );
        assert!(text.contains("zap_panic"), "failure raises:\n{text}");
    }

    #[test]
    fn lower_assert_with_message() {
        let m = lower_ok("def main():\n    assert False, \"nope\"\n");
        let text = format!("{m}");
        assert!(
            text.contains("strconst \"nope\""),
            "message literal:\n{text}"
        );
    }

    #[test]
    fn lower_with_expect_raises() {
        let m = lower_ok(
            "def boom():\n    raise \"x\"\n\
             def main():\n    with expect_raises(ValueError):\n        boom()\n",
        );
        let text = format!("{m}");
        assert!(
            text.contains("expected ValueError to be raised"),
            "miss message present:\n{text}"
        );
    }

    #[test]
    fn with_general_context_manager_is_rejected() {
        let (ast, _p, _l) =
            parse_str("def main():\n    with open(\"f\") as fh:\n        print(1)\n");
        let (_m, errs) = lower_module(&ast);
        assert!(
            !errs.is_empty(),
            "general `with` should be rejected for now"
        );
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
    fn lower_tier1_enum_and_match() {
        // A tier-1 enum is its unboxed backing int: member access is an iconst
        // (RED=0, GREEN=10, BLUE continues at 11), and a `match` is an
        // icmp-against-constant chain (D-0018/D-0040).
        let m = lower_ok(
            "enum Color:\n    RED\n    GREEN = 10\n    BLUE\n\n\
             def classify(Color c) -> int64:\n    match c:\n\
             \x20       case Color.RED:\n            return 1\n\
             \x20       case Color.GREEN:\n            return 2\n\
             \x20       case Color.BLUE:\n            return 3\n\n\
             def main():\n    print(classify(Color.BLUE))\n",
        );
        // No tagged struct is allocated for a tier-1 enum.
        assert!(
            m.structs.is_empty(),
            "tier-1 enum should not allocate a struct"
        );
        let classify = m
            .funcs
            .iter()
            .find(|f| f.name == "classify")
            .expect("classify");
        assert_eq!(classify.params, vec![Ty::I64], "tier-1 enum lowers to i64");
        let text = format!("{m}");
        // BLUE auto-continues from GREEN=10.
        assert!(text.contains("iconst 11"), "BLUE should be 11:\n{text}");
        assert!(
            text.contains("icmp.Eq"),
            "match dispatches on equality:\n{text}"
        );
        // The construction `Color.BLUE` is just the discriminant constant.
        assert!(!text.contains("structnew"), "no struct for tier-1:\n{text}");
    }

    #[test]
    fn lower_tier3_variant_construction_and_match() {
        // A tier-3 enum is a GC-allocated tagged struct `{ tag:i32, payload… }`
        // (D-0046). Construction stores the tag + payload fields; `match`
        // branches on the tag and binds fields positionally or by name.
        let m = lower_ok(
            "enum Shape:\n    Circle(f64 r)\n    Rect(f64 w, f64 h)\n    Unit\n\n\
             def area(Shape s) -> f64:\n    match s:\n\
             \x20       case Shape.Circle(r):\n            return 3.14 * r * r\n\
             \x20       case Shape.Rect(w=w, h=h):\n            return w * h\n\
             \x20       case Shape.Unit:\n            return 1.0\n\n\
             def main():\n\
             \x20   print(area(Shape.Circle(r=2.0)))\n\
             \x20   print(area(Shape.Rect(3.0, 4.0)))\n\
             \x20   print(area(Shape.Unit))\n",
        );
        // One tagged struct: tag(i32) + Circle.r + Rect.w + Rect.h (flattened).
        assert_eq!(m.structs.len(), 1, "one tagged struct for the sum type");
        assert_eq!(m.structs[0].name, "Shape");
        assert_eq!(
            m.structs[0].fields,
            vec![Ty::I32, Ty::F64, Ty::F64, Ty::F64],
            "layout is [tag, Circle.r, Rect.w, Rect.h]"
        );
        let area = m.funcs.iter().find(|f| f.name == "area").expect("area");
        assert_eq!(
            area.params,
            vec![Ty::Struct(0)],
            "scrutinee is a tagged struct"
        );
        let text = format!("{m}");
        assert!(text.contains("structnew struct#0"), "construction:\n{text}");
        // tag slot written, payload written; match reads the tag and the fields.
        assert!(
            text.contains("fieldset struct#0 %0.0 = %1"),
            "tag set:\n{text}"
        );
        assert!(
            text.contains("fieldget struct#0"),
            "match reads fields:\n{text}"
        );
    }

    #[test]
    fn lower_recursive_enum_is_pointer() {
        // A (mutually) recursive variant field — whose type is the enum itself —
        // is a pointer to a heap enum value, breaking the otherwise-infinite
        // size (D-0046). Reusing the struct machinery gives this for free: the
        // field's ZIR type is `Struct(sid)`, i.e. a pointer.
        let m = lower_ok(
            "enum Expr:\n    Num(int64 v)\n    Add(Expr l, Expr r)\n\n\
             def eval(Expr e) -> int64:\n    match e:\n\
             \x20       case Expr.Num(v):\n            return v\n\
             \x20       case Expr.Add(l, r):\n            return eval(l) + eval(r)\n\n\
             def main():\n    print(eval(Expr.Add(Expr.Num(2), Expr.Num(3))))\n",
        );
        assert_eq!(m.structs.len(), 1);
        // Layout: [tag:i32, Num.v:i64, Add.l:struct#0, Add.r:struct#0]. The
        // recursive fields point back to the same tagged struct.
        assert_eq!(
            m.structs[0].fields,
            vec![Ty::I32, Ty::I64, Ty::Struct(0), Ty::Struct(0)],
            "recursive variant fields are pointers to the enum's own struct"
        );
        let text = format!("{m}");
        assert!(
            text.contains("call @eval"),
            "recursion lowers to a call:\n{text}"
        );
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
    fn lower_heap() {
        // heap[int64] (D-0057): construction, push/pop/peek/len, and the
        // priority-order snapshot that `for x in h` iterates all lower to the
        // heap runtime entry points. A `heap[int64]` carries its element kind in
        // the ZIR type.
        let m = lower_ok(include_str!("../../../tests/golden/heap.zap"));
        let text = format!("{m}");
        assert!(text.contains("alloca heap[i64]"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_new"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_push_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_pop_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_peek_i64"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_len"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_items"), "text:\n{text}");
    }

    #[test]
    fn lower_heap_str_elements() {
        // A `heap[str]` routes through the pointer-word entry points; string
        // elements are compared by `ZapStr` bytes in the runtime.
        let m = lower_ok(
            "def main():\n    \
                 auto h = heap[str]()\n    \
                 h.push(\"pear\")\n    \
                 h.push(\"apple\")\n    \
                 print(h.pop())\n    \
                 print(h.peek())\n",
        );
        let text = format!("{m}");
        assert!(text.contains("alloca heap[ptr]"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_push_ptr"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_pop_ptr"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_peek_ptr"), "text:\n{text}");
    }

    #[test]
    fn lower_heap_max_comparator() {
        // `cmp="max"` is accepted and lowers to the same `zap_heap_new(kind, dir)`
        // construction (the direction argument carries min vs max; the runtime
        // golden exercises the resulting largest-first ordering).
        let m = lower_ok(
            "def main():\n    \
                 auto h = heap[int64](cmp=\"max\")\n    \
                 h.push(1)\n    \
                 print(h.pop())\n",
        );
        let text = format!("{m}");
        assert!(text.contains("call @zap_heap_new"), "text:\n{text}");
        assert!(text.contains("call @zap_heap_push_i64"), "text:\n{text}");
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
    fn lower_array_dtypes_and_promotion() {
        // Mixed-dtype element-wise ops promote to the higher dtype (#49): the
        // ArrayBinOp carries the per-operand dtypes and a promoted `out`.
        let m = lower_ok(include_str!("../../../tests/golden/array_promote.zap"));
        let text = format!("{m}");
        // i64 + f64 -> f64 (operands keep their own dtype in the node).
        assert!(text.contains("array.Add.f64 %"), "text:\n{text}");
        assert!(text.contains(":i64, %"), "text:\n{text}");
        // uint8 + i64 -> i64.
        assert!(text.contains("array.Add.i64 %"), "text:\n{text}");
        assert!(text.contains(":u8, %"), "text:\n{text}");

        // uint8 arrays select the u8-suffixed runtime entry points; a weak int
        // scalar keeps the dtype (`a * 2` stays u8).
        let m = lower_ok(include_str!("../../../tests/golden/array_uint8.zap"));
        let text = format!("{m}");
        assert!(text.contains("call @zap_array_set_u8"), "text:\n{text}");
        assert!(text.contains("call @zap_array_sum_u8"), "text:\n{text}");
        assert!(text.contains("call @zap_array_get_u8"), "text:\n{text}");
        assert!(text.contains("array.Add.u8"), "text:\n{text}");
        assert!(text.contains("array.Mul.u8"), "text:\n{text}");

        // bool arrays store via the bool entry point; arithmetic promotes bool
        // -> uint8 (so `m + n` / `m * n` are u8 results).
        let m = lower_ok(include_str!("../../../tests/golden/array_bool.zap"));
        let text = format!("{m}");
        assert!(text.contains("call @zap_array_set_bool"), "text:\n{text}");
        assert!(text.contains("call @zap_array_sum_bool"), "text:\n{text}");
        assert!(text.contains("array.Add.u8 %"), "text:\n{text}");
        assert!(text.contains(":bool, %"), "text:\n{text}");
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
        let eq = m
            .funcs
            .iter()
            .find(|f| f.name == "User____eq__")
            .expect("eq");
        assert_eq!(eq.params, vec![Ty::Struct(0), Ty::Struct(0)]);
        assert_eq!(eq.ret, Ty::I1);
        let repr = m
            .funcs
            .iter()
            .find(|f| f.name == "User____repr__")
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

    // --- goroutines & channels (ticket #25, D-0013/D-0031) ---

    #[test]
    fn lower_goroutines_fixture() {
        // The golden program exercises the whole core surface: channel
        // construction (unbuffered + buffered), `spawn` of a named function,
        // channel send/receive, and `close`.
        let m = lower_ok(include_str!("../../../tests/golden/goroutines.zap"));
        let text = format!("{m}");
        // `channel[int64]()` / `channel[int64](cap)` lower to the constructor.
        assert!(text.contains("call @zap_chan_new"), "text:\n{text}");
        // `spawn worker(...)` lowers to the dedicated Spawn node naming the fn.
        assert!(text.contains("spawn @worker("), "text:\n{text}");
        // send/recv pick the i64-suffixed typed entry points.
        assert!(text.contains("@zap_chan_send_i64("), "text:\n{text}");
        assert!(text.contains("@zap_chan_recv_i64("), "text:\n{text}");
        // `close(ch)` lowers to the close entry point.
        assert!(text.contains("@zap_chan_close("), "text:\n{text}");
        // A `channel[int64]` parameter and a channel value are typed `chan[i64]`.
        let worker = m.funcs.iter().find(|f| f.name == "worker").expect("worker");
        assert_eq!(worker.params, vec![Ty::Chan(Elem::I64), Ty::I64]);
    }

    #[test]
    fn spawn_is_a_dedicated_node_with_eager_args() {
        // `spawn f(a, b)` evaluates its args in the parent, then emits a Spawn
        // node carrying the callee and the evaluated argument values.
        let m = lower_ok(
            "def f(int64 a, int64 b):\n    print(a + b)\n\ndef main():\n    spawn f(1, 2)\n",
        );
        let main = m.funcs.iter().find(|f| f.name == "main").expect("main");
        let spawn = main
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .find_map(|i| match i {
                Inst::Spawn { func, args, owner } => Some((func.clone(), args.len(), *owner)),
                _ => None,
            })
            .expect("a Spawn instruction");
        // A bare `spawn` (no enclosing `scope:`) is owned by the root scope.
        assert_eq!(spawn, ("f".to_string(), 2, Owner::Root));
    }

    #[test]
    fn scope_spawn_detached_ownership() {
        // A `spawn` inside a `scope:` is owned by that scope's handle; `spawn
        // detached` is owned by nothing; a bare `spawn` by the root (D-0044).
        let m = lower_ok(
            "def f():\n    print(1)\n\ndef main():\n    scope:\n        spawn f()\n    spawn detached f()\n    spawn f()\n",
        );
        let text = format!("{m}");
        // The scope opens with a scope_enter handle and joins with scope_exit.
        assert!(text.contains("call @zap_scope_enter("), "text:\n{text}");
        assert!(text.contains("@zap_scope_exit("), "text:\n{text}");
        let main = m.funcs.iter().find(|f| f.name == "main").expect("main");
        let owners: Vec<Owner> = main
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter_map(|i| match i {
                Inst::Spawn { owner, .. } => Some(*owner),
                _ => None,
            })
            .collect();
        assert_eq!(owners.len(), 3, "three spawns");
        assert!(
            matches!(owners[0], Owner::Scope(_)),
            "the scoped spawn is owned by the scope"
        );
        assert_eq!(owners[1], Owner::Detached, "the detached spawn");
        assert_eq!(owners[2], Owner::Root, "the root-scoped spawn");
    }

    #[test]
    fn channel_of_strings_uses_ptr_entry_points() {
        // A `channel[str]` stores pointer-word elements, selecting the `_ptr`
        // typed send/receive entry points.
        let m = lower_ok(
            "def main():\n    auto ch = channel[str](1)\n    ch <- \"hi\"\n    auto s = <-ch\n    print(s)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@zap_chan_send_ptr("), "text:\n{text}");
        assert!(text.contains("@zap_chan_recv_ptr("), "text:\n{text}");
    }

    #[test]
    fn spawn_of_unknown_function_errors() {
        let (ast, _p, _l) = parse_str("def main():\n    spawn nope(1)\n");
        let (_m, errs) = lower_module(&ast);
        assert!(
            !errs.is_empty(),
            "spawn of an undefined function should error"
        );
    }

    // --- regex (ticket #23, D-0010) ---

    #[test]
    fn lower_regex_fixture() {
        // The golden program exercises the whole surface: m// match, positional
        // and named capture groups, `!~`, and first/global s/// substitution.
        let m = lower_ok(include_str!("../../../tests/golden/regex.zap"));
        let text = format!("{m}");
        // m// lowers to a match-object call; s/// to a substitution call.
        assert!(text.contains("call @zap_regex_match"), "text:\n{text}");
        assert!(text.contains("call @zap_regex_subst"), "text:\n{text}");
        // Capture-group access lowers to the positional and named accessors.
        assert!(text.contains("call @zap_regex_group("), "text:\n{text}");
        assert!(
            text.contains("call @zap_regex_group_name("),
            "text:\n{text}"
        );
        // `if mt:` / `!~` test the match object for null via the runtime helper.
        assert!(text.contains("call @zap_ptr_nonnull"), "text:\n{text}");
        // The pattern/replacement text reaches the runtime verbatim as a string
        // constant: regex escapes like `\d` are NOT string-unescaped (the ZIR
        // printer's `{:?}` doubles the backslash), and `$2 $1` is untouched.
        assert!(
            text.contains(r#"strconst "(\\d{4})-(\\d{2})-(\\d{2})""#),
            "text:\n{text}"
        );
        assert!(text.contains(r#"strconst "$2 $1""#), "text:\n{text}");
    }

    #[test]
    fn regex_match_is_a_match_object_negation_is_bool() {
        // `=~ m//` binds a match object (reference type); `!~` yields an i1.
        let m = lower_ok(
            "def main():\n    auto mt = \"x1\" =~ m/(\\d)/\n    auto no = \"x\" !~ m/\\d/\n    print(mt.group(1))\n    if no:\n        print(\"none\")\n",
        );
        let text = format!("{m}");
        assert!(text.contains("= call @zap_regex_match"), "text:\n{text}");
        // The group accessor returns a string (pointer), which `print` prints.
        assert!(text.contains("= call @zap_regex_group("), "text:\n{text}");
    }

    // --- Python interop (ticket #27, D-0012) ---

    #[test]
    fn lower_python_interop_fixture() {
        // The golden program exercises the whole surface: module import,
        // attribute+call dispatch, and scalar/str marshaling both directions.
        let m = lower_ok(include_str!("../../../tests/golden/python_interop.zap"));
        let text = format!("{m}");
        // Import, attribute read, and call all route to the Python runtime.
        assert!(text.contains("@zap_py_import("), "text:\n{text}");
        assert!(text.contains("@zap_py_getattr("), "text:\n{text}");
        assert!(text.contains("@zap_py_call("), "text:\n{text}");
        // Argument marshaling: int, float, and str cross into Python.
        assert!(text.contains("call @zap_py_from_i64("), "text:\n{text}");
        assert!(text.contains("call @zap_py_from_f64("), "text:\n{text}");
        assert!(text.contains("call @zap_py_from_str("), "text:\n{text}");
        // Result marshaling: `int64(...)` converts a Python int back, and
        // `print` of a Python handle renders it via its str().
        assert!(text.contains("@zap_py_to_i64("), "text:\n{text}");
        assert!(text.contains("call @zap_py_str("), "text:\n{text}");
        // The argument vector for a call is a `list[ptr]` of marshaled handles.
        assert!(text.contains("call @zap_list_new("), "text:\n{text}");
        assert!(text.contains("call @zap_list_push_ptr("), "text:\n{text}");
    }

    #[test]
    fn python_import_and_call_lower_to_runtime() {
        // A minimal import + call lowers to import -> getattr -> call, with the
        // int argument marshaled in and the int result marshaled back out.
        let m = lower_ok(
            "import python \"math\" as m\n\ndef main():\n    print(int64(m.gcd(12, 8)))\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@zap_py_import("), "text:\n{text}");
        assert!(text.contains("@zap_py_getattr("), "text:\n{text}");
        assert!(text.contains("@zap_py_call("), "text:\n{text}");
        assert!(text.contains("call @zap_py_from_i64("), "text:\n{text}");
        assert!(text.contains("@zap_py_to_i64("), "text:\n{text}");
    }

    #[test]
    fn python_calls_in_a_try_use_the_exception_edge() {
        // A Python call inside a `try` becomes an `invoke` to the handler, so a
        // raised Python error is catchable with `except` (D-0012 exception
        // translation routes through the #22 unwinding path).
        let m = lower_ok(
            "import python \"math\" as m\n\ndef main():\n    try:\n        print(int64(m.sqrt(2.0)))\n    except:\n        print(\"boom\")\n",
        );
        let text = format!("{m}");
        assert!(text.contains("invoke @zap_py_import"), "text:\n{text}");
        assert!(text.contains("invoke @zap_py_call"), "text:\n{text}");
        assert!(text.contains("landingpad"), "text:\n{text}");
    }

    #[test]
    fn regex_subject_group_method_is_checked() {
        // An unknown match-object method reports cleanly (not a silent miscompile).
        let (ast, _p, _l) =
            parse_str("def main():\n    auto mt = \"a\" =~ m/a/\n    print(mt.nope())\n");
        let (_m, errs) = lower_module(&ast);
        assert!(!errs.is_empty(), "unknown match method should error");
    }

    #[test]
    fn derive_eq_only_methods_are_callable() {
        // A struct can derive just one trait; the generated method resolves.
        let m = lower_ok(
            "@derive(Eq)\nstruct P:\n    int64 x\n\ndef main():\n    \
             auto a = P(x=1)\n    auto b = P(x=1)\n    if a == b:\n        print(1)\n",
        );
        assert!(m.funcs.iter().any(|f| f.name == "P____eq__"));
        assert!(!m.funcs.iter().any(|f| f.name == "P____repr__"));
    }

    #[test]
    fn deferred_builtins_error() {
        // Built-ins in the locked set but not yet generated report cleanly.
        let src = "struct S:\n    @validate(max_len=9)\n    str x\n";
        let (ast, _p, _l) = parse_str(src);
        let (_m, errs) = lower_module(&ast);
        assert!(
            !errs.is_empty(),
            "expected a deferred-feature error for:\n{src}"
        );
    }

    #[test]
    fn derive_json_generates_codec() {
        // @derive(Json) (D-0059, I#100) generates the encode method, the decode
        // associated function, and its recursive JsonValue core.
        let (ast, _p, _l) = parse_str("@derive(Json)\nstruct S:\n    int64 x\n    str y\n");
        let (m, errs) = lower_module(&ast);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
        for want in ["S__to_json", "S__from_json", "S__from_json_value"] {
            assert!(
                m.funcs.iter().any(|f| f.name == want),
                "missing generated {want}"
            );
        }
    }

    #[test]
    fn derive_yaml_generates_codec() {
        // @derive(Yaml) (D-0063, I#149) generates the encode method, the decode
        // associated function, and reuses the shared JsonValue decode core.
        let (ast, _p, _l) = parse_str("@derive(Yaml)\nstruct S:\n    int64 x\n    str y\n");
        let (m, errs) = lower_module(&ast);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
        for want in ["S__to_yaml", "S__from_yaml", "S__from_json_value"] {
            assert!(
                m.funcs.iter().any(|f| f.name == want),
                "missing generated {want}"
            );
        }
    }

    #[test]
    fn derive_json_and_yaml_share_decode_core() {
        // Deriving both must emit the shared `S__from_json_value` exactly once
        // (deduped), not twice.
        let (ast, _p, _l) =
            parse_str("@derive(Json)\n@derive(Yaml)\nstruct S:\n    int64 x\n    str y\n");
        let (m, errs) = lower_module(&ast);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
        let n = m
            .funcs
            .iter()
            .filter(|f| f.name == "S__from_json_value")
            .count();
        assert_eq!(n, 1, "shared decode core should be generated once, got {n}");
    }

    // --- operator overloading (ticket #52, D-0047) ---

    #[test]
    fn operator_desugars_to_dunder_call() {
        // `a + b` / `a == b` / `a < b` on a struct lower to calls of the mangled
        // dunder methods (`V____add__`, `V____eq__`, `V____cmp__`).
        let m = lower_ok(
            "struct V:\n    int64 x\n\n    \
             def __add__(self, V other) -> V:\n        return V(x=self.x + other.x)\n\n    \
             def __eq__(self, V other) -> bool:\n        return self.x == other.x\n\n    \
             def __cmp__(self, V other) -> int64:\n        return 0\n\n\
             def main():\n    auto a = V(x=1)\n    auto b = V(x=2)\n    \
             auto c = a + b\n    if a == b:\n        print(1)\n    if a < b:\n        print(2)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@V____add__"), "text:\n{text}");
        assert!(text.contains("@V____eq__"), "text:\n{text}");
        assert!(text.contains("@V____cmp__"), "text:\n{text}");
    }

    #[test]
    fn derive_ord_and_hash_generate_methods() {
        // @derive(Ord)/(Hash) now generate `__cmp__`/`__hash__` (in addition to
        // the existing Eq/Repr), field-wise.
        let m = lower_ok("@derive(Eq, Ord, Hash, Repr)\nstruct V:\n    int64 x\n    str s\n");
        assert!(
            m.funcs.iter().any(|f| f.name == "V____cmp__"),
            "expected a generated __cmp__"
        );
        assert!(
            m.funcs.iter().any(|f| f.name == "V____hash__"),
            "expected a generated __hash__"
        );
        let hash = m.funcs.iter().find(|f| f.name == "V____hash__").unwrap();
        assert_eq!(hash.ret, Ty::I64);
        let text = format!("{m}");
        // Hash combines per-field hashes; cmp compares the str field via the runtime.
        assert!(text.contains("@zap_hash_combine"), "text:\n{text}");
        assert!(text.contains("@zap_str_cmp"), "text:\n{text}");
    }

    #[test]
    fn struct_set_key_uses_fn_entry_points() {
        // A struct used as a set element routes to the `_fn` runtime entry points
        // and passes its `__hash__`/`__eq__` as function pointers (D-0047).
        let m = lower_ok(
            "@derive(Eq, Hash)\nstruct V:\n    int64 x\n\n\
             def main():\n    auto a = V(x=1)\n    auto s = {a}\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@zap_set_add_fn"), "text:\n{text}");
        assert!(text.contains("funcaddr @V____hash__"), "text:\n{text}");
        assert!(text.contains("funcaddr @V____eq__"), "text:\n{text}");
    }

    #[test]
    fn operator_without_dunder_lowers_to_clean_error() {
        // The ir crate runs without sema; a missing dunder is a clean error.
        let (ast, _p, _l) = parse_str(
            "struct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto b = V(x=2)\n    auto c = a + b\n",
        );
        let (_m, errs) = lower_module(&ast);
        assert!(!errs.is_empty(), "missing __add__ should error");
        assert!(errs[0].message.contains("__add__"), "{errs:?}");
    }

    #[test]
    fn derive_and_user_method_clash_errors() {
        let (ast, _p, _l) = parse_str(
            "@derive(Eq)\nstruct S:\n    int64 x\n\n    def __eq__(self, S other) -> bool:\n        return True\n",
        );
        let (_m, errs) = lower_module(&ast);
        assert!(
            !errs.is_empty(),
            "a user `__eq__` plus @derive(Eq) should clash"
        );
    }

    // --- in-place augmented assignment (ticket #61, D-0047/D-0023) ---

    #[test]
    fn aug_assign_uses_inplace_dunder_when_defined() {
        // `a += b` on a struct that defines `__iadd__` lowers to a call of the
        // in-place dunder (`Acc____iadd__`), which mutates in place — NOT the
        // binary `__add__` form.
        let m = lower_ok(
            "struct Acc:\n    int64 total\n\n    \
             def __iadd__(self, Acc other) -> Acc:\n        \
             self.total = self.total + other.total\n        return self\n\n\
             def main():\n    auto a = Acc(total=1)\n    auto b = Acc(total=2)\n    \
             a += b\n    print(a.total)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@Acc____iadd__"), "text:\n{text}");
        assert!(
            !text.contains("@Acc____add__"),
            "the in-place form must not call the binary __add__:\n{text}"
        );
    }

    #[test]
    fn aug_assign_falls_back_to_binary_dunder() {
        // A struct that defines only `__add__` (no `__iadd__`) still supports
        // `+=`, deriving it from the binary operator (`a = a.__add__(b)`).
        let m = lower_ok(
            "struct V:\n    int64 x\n\n    \
             def __add__(self, V other) -> V:\n        return V(x=self.x + other.x)\n\n\
             def main():\n    auto a = V(x=1)\n    auto b = V(x=2)\n    \
             a += b\n    print(a.x)\n",
        );
        let text = format!("{m}");
        assert!(text.contains("@V____add__"), "text:\n{text}");
        assert!(
            !text.contains("@V____iadd__"),
            "no __iadd__ is defined, so none should be called:\n{text}"
        );
    }

    #[test]
    fn aug_assign_scalar_is_a_plain_binop() {
        // Built-in scalar `+=` is unchanged: a direct `Add`, no dunder call.
        let m = lower_ok("def main():\n    auto a = 1\n    a += 2\n    print(a)\n");
        let text = format!("{m}");
        assert!(text.contains("= Add "), "scalar += is a plain Add:\n{text}");
        assert!(
            !text.contains("__iadd__") && !text.contains("__add__"),
            "a scalar must not route through an operator dunder:\n{text}"
        );
    }

    // --- tuples (ticket #54, D-0049) ---

    #[test]
    fn lower_tuple_literal_and_const_index() {
        // A tuple literal lowers to an anonymous-struct aggregate: `structnew`
        // plus one `fieldset` per element; `t[k]` is a `fieldget` at a constant
        // index.
        let m = lower_ok(include_str!("../../../tests/golden/tuple_basic.zap"));
        let text = format!("{m}");
        assert!(text.contains("structnew"), "text:\n{text}");
        assert!(text.contains("fieldset"), "text:\n{text}");
        assert!(text.contains("fieldget"), "text:\n{text}");
        // One tuple layout `(int64, int64, int64)` was interned as a struct.
        assert_eq!(m.structs.len(), 1, "one interned tuple layout");
        assert_eq!(m.structs[0].fields, vec![Ty::I64, Ty::I64, Ty::I64]);
    }

    #[test]
    fn lower_multiple_return_via_tuple_struct() {
        // `divmod -> (int64, int64)` returns a struct aggregate; the caller
        // destructures it with `fieldget`. The literal return tuple and the
        // declared return type intern to the same struct id.
        let m = lower_ok(include_str!("../../../tests/golden/tuple_multiret.zap"));
        let divmod = m.funcs.iter().find(|f| f.name == "divmod").expect("divmod");
        assert!(matches!(divmod.ret, Ty::Struct(_)), "ret: {:?}", divmod.ret);
        let text = format!("{m}");
        assert!(text.contains("fieldget"), "text:\n{text}");
    }

    #[test]
    fn lower_rest_builds_subtuple() {
        // `(first, *rest) = (10, 20, 30, 40)`: `first` is a `fieldget`, and the
        // rest sub-tuple is a fresh `structnew` copying the remaining fields.
        let m = lower_ok(include_str!("../../../tests/golden/tuple_rest.zap"));
        let text = format!("{m}");
        assert!(text.contains("structnew"), "text:\n{text}");
        // The 4-tuple and the 3-element rest sub-tuple are distinct layouts.
        assert!(
            m.structs.iter().any(|s| s.fields.len() == 4),
            "the source 4-tuple layout"
        );
        assert!(
            m.structs.iter().any(|s| s.fields.len() == 3),
            "the rest 3-tuple layout"
        );
    }

    #[test]
    fn lower_tuple_nonconstant_index_errors() {
        // A runtime tuple index is rejected at lowering too (sema also reports
        // it, but lowering must never emit a dynamic tuple access).
        let (ast, _p, _l) = parse_str(
            "def main():\n    auto t = (1, 2)\n    auto i = 0\n    auto x = t[i]\n    print(x)\n",
        );
        let (_m, errs) = lower_module(&ast);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("compile-time-constant")),
            "expected a const-index error, got {errs:?}"
        );
    }

    #[test]
    fn lower_swap_reads_before_writing() {
        // `(x, y) = (y, x)` builds the right-hand tuple fully before storing, so
        // the swap is correct. The aggregate shows up as structnew + fieldget.
        let m = lower_ok(include_str!("../../../tests/golden/tuple_swap.zap"));
        let text = format!("{m}");
        assert!(text.contains("structnew"), "text:\n{text}");
        assert!(text.contains("fieldget"), "text:\n{text}");
    }

    // --- iteration protocol (ticket #53, D-0048) ---

    const RANGE_SRC: &str = "\
struct Range:
    int64 lo
    int64 hi
    def __iter__(self) -> RangeIter:
        return RangeIter(cur=self.lo, hi=self.hi)

struct RangeIter:
    int64 cur
    int64 hi
    def __iter__(self) -> RangeIter:
        return self
    def __next__(self) -> (int64, bool):
        if self.cur >= self.hi:
            return (0, False)
        auto v = self.cur
        self.cur = self.cur + 1
        return (v, True)
";

    #[test]
    fn lower_generator_state_machine() {
        // A `def` with `yield` (I#67) lowers to a state-struct with comma-ok
        // `__iter__`/`__next__`; the call constructs the struct (no `@count`
        // function is emitted).
        let m = lower_ok(
            "\
def count(int64 n):
    auto i = 0
    while i < n:
        yield i
        i = i + 1

def main():
    auto total = 0
    for x in count(5):
        total = total + x
    print(total)
",
        );
        let text = format!("{m}");
        // The generator function itself is not emitted; its iterator methods are.
        assert!(!m.funcs.iter().any(|f| f.name == "count"), "text:\n{text}");
        assert!(
            m.funcs.iter().any(|f| f.name == "$gen:count____iter__"),
            "text:\n{text}"
        );
        let next = m
            .funcs
            .iter()
            .find(|f| f.name == "$gen:count____next__")
            .expect("__next__ emitted");
        // `__next__` returns the comma-ok `(int64, bool)` tuple.
        assert!(matches!(next.ret, Ty::Struct(_)), "ret: {:?}", next.ret);
        // The state machine dispatches on the `$state` field and suspends/returns.
        assert!(text.contains("$gen:count____next__"), "text:\n{text}");
        assert!(text.contains("icmp"), "text:\n{text}");
        // The call site constructs the state struct rather than calling a fn.
        assert!(!text.contains("call @count"), "text:\n{text}");
    }

    #[test]
    fn lower_user_iterable_commaok() {
        // `for x in Range(...)` desugars to `__iter__` then a loop over the
        // comma-ok `__next__`, binding each tuple's value field.
        let m = lower_ok(&format!(
            "{RANGE_SRC}\ndef main():\n    auto total = 0\n    for x in Range(lo=0, hi=5):\n        total = total + x\n    print(total)\n"
        ));
        let text = format!("{m}");
        assert!(text.contains("@Range____iter__"), "text:\n{text}");
        assert!(text.contains("@RangeIter____next__"), "text:\n{text}");
        // The loop reads both fields of the `(int64, bool)` result tuple.
        assert!(text.contains("fieldget struct#2 %"), "text:\n{text}");
    }

    #[test]
    fn lower_stopiteration_iterable() {
        // The Python form drives through a landing pad that checks the in-flight
        // exception against the `StopIteration` sentinel.
        let m = lower_ok(
            "\
struct Count:
    int64 n
    def __iter__(self) -> CountIter:
        return CountIter(cur=0, n=self.n)

struct CountIter:
    int64 cur
    int64 n
    def __iter__(self) -> CountIter:
        return self
    def __next__(self) -> int64:
        if self.cur >= self.n:
            raise StopIteration
        auto v = self.cur
        self.cur = self.cur + 1
        return v

def main():
    auto total = 0
    for x in Count(n=4):
        total = total + x
    print(total)
",
        );
        let text = format!("{m}");
        assert!(text.contains("strconst \"StopIteration\""), "text:\n{text}");
        assert!(text.contains("@zap_str_eq"), "text:\n{text}");
        assert!(text.contains("landingpad"), "text:\n{text}");
    }

    #[test]
    fn lower_list_comprehension_over_user_iterable() {
        let m = lower_ok(&format!(
            "{RANGE_SRC}\ndef main():\n    auto squares = [x * x for x in Range(lo=1, hi=5) if x != 3]\n    print(len(squares))\n"
        ));
        let text = format!("{m}");
        assert!(text.contains("@zap_list_new"), "text:\n{text}");
        assert!(text.contains("@zap_list_push_i64"), "text:\n{text}");
        assert!(text.contains("@RangeIter____next__"), "text:\n{text}");
    }

    #[test]
    fn lower_membership_fallback_iterates() {
        // `x in r` with no `__contains__` iterates and compares each element.
        let m = lower_ok(&format!(
            "{RANGE_SRC}\ndef main():\n    auto r = Range(lo=0, hi=5)\n    if 3 in r:\n        print(1)\n"
        ));
        let text = format!("{m}");
        assert!(text.contains("@RangeIter____next__"), "text:\n{text}");
        assert!(text.contains("icmp.Eq"), "text:\n{text}");
    }

    #[test]
    fn lower_non_iterable_for_errors() {
        let (ast, _p, _l) = parse_str(
            "struct Point:\n    int64 x\ndef main():\n    for p in Point(x=1):\n        print(1)\n",
        );
        let (_m, errs) = lower_module(&ast);
        assert!(
            !errs.is_empty(),
            "a non-iterable struct in `for` should be rejected"
        );
    }

    // --- value structs (#65, D-0051) ---

    /// Count `StructNew` instructions in a function (each is one aggregate
    /// allocation / copy).
    fn structnews(f: &Func) -> usize {
        f.blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(i, Inst::StructNew { .. }))
            .count()
    }

    const VEC2: &str = "value struct Vec2:\n    f64 x\n    f64 y\n\
                        \x20   def len2(self) -> f64:\n        return self.x * self.x + self.y * self.y\n";
    const PT: &str = "struct Pt:\n    f64 x\n    f64 y\n";

    #[test]
    fn value_struct_layout_is_flagged() {
        // The `value` modifier is tracked end-to-end onto the ZIR layout
        // (D-0051); a plain struct stays a reference type.
        let m = lower_ok(&format!(
            "{VEC2}{PT}\ndef main():\n    auto v = Vec2(x=1.0, y=2.0)\n    print(v.len2())\n"
        ));
        let vec2 = m.structs.iter().find(|s| s.name == "Vec2").expect("Vec2");
        let pt = m.structs.iter().find(|s| s.name == "Pt").expect("Pt");
        assert!(vec2.is_value, "Vec2 should be a value struct");
        assert!(!pt.is_value, "plain struct Pt should be a reference type");
        assert_eq!(vec2.fields, vec![Ty::F64, Ty::F64]);
        // A flat value struct uses the physical inline / register / sret layout
        // (D-0051); a reference struct never does (shared identity).
        assert!(
            vec2.inline,
            "flat value struct Vec2 should be inline-laid-out"
        );
        assert!(!pt.inline, "reference struct Pt must not be inline");
    }

    #[test]
    fn nested_value_struct_field_is_not_inline() {
        // A value struct with a value-struct *field* keeps the heap layout: a
        // by-value (shallow) aggregate copy would share the nested instance, so
        // the inline ABI does not apply and deep-copy semantics are preserved.
        let m = lower_ok(&format!(
            "{VEC2}value struct Seg:\n    Vec2 a\n    Vec2 b\n\
             def main():\n    auto s = Seg(a=Vec2(x=1.0, y=2.0), b=Vec2(x=3.0, y=4.0))\n    print(s.a.len2())\n"
        ));
        let seg = m.structs.iter().find(|s| s.name == "Seg").expect("Seg");
        let vec2 = m.structs.iter().find(|s| s.name == "Vec2").expect("Vec2");
        assert!(seg.is_value, "Seg is a value struct");
        assert!(
            !seg.inline,
            "a value struct with a value-struct field keeps the heap layout"
        );
        assert!(vec2.inline, "the flat Vec2 field type is itself inline");
    }

    #[test]
    fn tuple_layout_is_inline() {
        // Tuples are value-semantic aggregates and share the inline/register/sret
        // layout path (D-0051), when flat.
        let m = lower_ok(
            "def divmod(int64 a, int64 b) -> (int64, int64):\n    return a // b, a % b\n\
             def main():\n    auto (q, r) = divmod(17, 5)\n    print(q)\n    print(r)\n",
        );
        let tup = m
            .structs
            .iter()
            .find(|s| s.name == "(tuple)" && s.fields == vec![Ty::I64, Ty::I64])
            .expect("(int64, int64) tuple layout");
        assert!(tup.inline, "a flat tuple should be inline-laid-out");
    }

    #[test]
    fn value_struct_copies_on_assignment() {
        // `b = a` for a value struct copies it (extra StructNew + field copies),
        // so mutating `b` cannot escape to `a`. A reference struct shares the
        // pointer (no copy).
        let value_src = format!(
            "{VEC2}\ndef main():\n    auto a = Vec2(x=1.0, y=2.0)\n    auto b = a\n    print(b.len2())\n"
        );
        let ref_src = format!(
            "{PT}\ndef main():\n    auto a = Pt(x=1.0, y=2.0)\n    auto b = a\n    print(a.x)\n"
        );
        let vm = lower_ok(&value_src);
        let rm = lower_ok(&ref_src);
        let vmain = vm.funcs.iter().find(|f| f.name == "main").unwrap();
        let rmain = rm.funcs.iter().find(|f| f.name == "main").unwrap();
        // Value: one StructNew for the constructor + one for the copy at `b = a`.
        assert_eq!(
            structnews(vmain),
            2,
            "value-struct assignment should emit a copy"
        );
        // Reference: just the constructor; assignment copies the pointer only.
        assert_eq!(
            structnews(rmain),
            1,
            "reference-struct assignment must not copy"
        );
    }

    #[test]
    fn inline_value_struct_pass_and_return_emit_no_copy() {
        // A flat (inline-eligible) value struct is passed and returned *by value*
        // via the register/sret ABI (D-0051): the ABI copy is the value-semantics
        // copy, so lowering emits NO explicit StructNew copy at the argument or
        // the `return`. This is the heap allocation the inline layout removes.
        let m = lower_ok(&format!(
            "{VEC2}\ndef scale(Vec2 v) -> Vec2:\n    return v\n\
             def main():\n    auto a = Vec2(x=1.0, y=2.0)\n    auto b = scale(a)\n    print(b.len2())\n"
        ));
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        let scale = m.funcs.iter().find(|f| f.name == "scale").unwrap();
        // main: only the constructor for `a`. The `scale(a)` argument is passed by
        // value (no copy), and the call result is fresh so `auto b = ...` is not
        // copied either.
        assert_eq!(
            structnews(main),
            1,
            "an inline value-struct argument is copied by the ABI, not a StructNew"
        );
        // scale: returning `v` by value needs no StructNew copy.
        assert_eq!(
            structnews(scale),
            0,
            "returning an inline value struct emits no copy (returned by value)"
        );
    }

    #[test]
    fn nested_value_struct_still_deep_copies_on_pass() {
        // A non-inline value struct (one with a value-struct field) keeps the
        // deep-copy-on-pass behavior: lowering still emits an explicit copy at the
        // argument boundary, so observable value semantics are unchanged.
        let m = lower_ok(&format!(
            "{VEC2}value struct Seg:\n    Vec2 a\n    Vec2 b\n\
             def mid(Seg s) -> f64:\n    return s.a.len2()\n\
             def main():\n    auto s = Seg(a=Vec2(x=1.0, y=2.0), b=Vec2(x=3.0, y=4.0))\n    print(mid(s))\n"
        ));
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        // Construct Seg (1) + its two Vec2 fields boxed in (2) + the deep copy at
        // the `mid(s)` argument, which recursively copies Seg and its two Vec2s
        // (3). So strictly more than the 3 construction StructNews.
        assert!(
            structnews(main) > 3,
            "a non-inline value struct must be deep-copied at the argument boundary"
        );
    }

    #[test]
    fn value_struct_boxed_into_list() {
        // Storing a value struct into a collection boxes it (heap copy, D-0051),
        // so the stored element is independent of the original.
        let m = lower_ok(&format!(
            "{VEC2}\ndef main():\n    auto a = Vec2(x=1.0, y=2.0)\n    auto xs = [a]\n    print(len(xs))\n"
        ));
        let main = m.funcs.iter().find(|f| f.name == "main").unwrap();
        // construct `a` + box-copy on insertion into the list.
        assert_eq!(
            structnews(main),
            2,
            "storing a value struct in a list should box (copy) it"
        );
    }
}
