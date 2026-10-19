//! ZIR — Zap's typed SSA intermediate representation, plus AST -> ZIR lowering.
//!
//! This crate has no LLVM dependency, so it builds and tests on any host. The
//! backend (`zapc_codegen`) consumes [`Module`] and translates ZIR -> LLVM.
//! See `design/IR.md`.

pub mod ir;
pub mod lower;

pub use ir::{
    BinOp, Block, BlockId, CastKind, CmpPred, Dtype, Elem, Func, Inst, Module, Term, Ty, UnOp,
    Value,
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
                Inst::Spawn { func, args } => Some((func.clone(), args.len())),
                _ => None,
            })
            .expect("a Spawn instruction");
        assert_eq!(spawn, ("f".to_string(), 2));
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
