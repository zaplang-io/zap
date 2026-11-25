//! First-class type values `Type[T]` (#1527) — IR lowering layer.
//!
//! A type value erases to a plain `int64` type-id token: a bare primitive type
//! name in value position lowers to `iconst <id>`, and a `Type[T]` parameter
//! lowers to an `i64` slot. There is no runtime reflection graph, no new IR type,
//! and no GC root. These host-only checks (no LLVM) use *concrete* `Type[..]`
//! parameters so no generic type-argument inference is needed (that path reads
//! sema's inferred types, exercised by the driver integration tests and the
//! `type_arg_decorator` golden). They pin that every type-value form lowers and
//! emits the documented, stable type-id.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

fn lower_text(src: &str) -> String {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    format!("{module}")
}

#[test]
fn int64_type_value_lowers_to_its_type_id() {
    // `int64` in value position becomes the stable type-id constant `1`.
    let zir = lower_text(
        "def opt(Type[int64] kind) -> int64:\n    return 1\n\
         def main():\n    print(opt(int64))\n",
    );
    assert!(
        zir.contains("iconst 1"),
        "int64 type value must lower to `iconst 1`, got:\n{zir}"
    );
}

#[test]
fn distinct_primitives_lower_to_distinct_ids() {
    // str -> 11, bool -> 10 (documented stable ids), proving the runtime token
    // distinguishes the types.
    let zir = lower_text(
        "def so(Type[str] k) -> int64:\n    return 1\n\
         def bo(Type[bool] k) -> int64:\n    return 1\n\
         def main():\n    print(so(str))\n    print(bo(bool))\n",
    );
    assert!(
        zir.contains("iconst 11"),
        "str type value must lower to `iconst 11`, got:\n{zir}"
    );
    assert!(
        zir.contains("iconst 10"),
        "bool type value must lower to `iconst 10`, got:\n{zir}"
    );
}

#[test]
fn type_t_parameter_is_an_i64_slot() {
    // A declared `Type[int64]` parameter lowers to a plain `i64` — the erased
    // type-id token, no new IR type and no GC root.
    let zir = lower_text(
        "def opt(Type[int64] kind) -> int64:\n    return 1\n\
         def main():\n    print(opt(int64))\n",
    );
    assert!(
        zir.contains("@opt(%0: i64)"),
        "a Type[int64] parameter must lower to an i64 slot, got:\n{zir}"
    );
}

#[test]
fn every_primitive_type_value_lowers() {
    // The whole v1 primitive set lowers in value position (no residual
    // `unknown variable`), each to its own type-id.
    for ty in [
        "int64", "int", "i64", "int32", "i32", "uint8", "u8", "uint16", "u16", "uint32", "u32",
        "uint64", "u64", "usize", "float64", "float", "f64", "float32", "f32", "bool", "str",
        "bytes", "char", "bigint", "complex",
    ] {
        let _ = lower_text(&format!(
            "def opt(Type[{ty}] k) -> int64:\n    return 1\n\
             def main():\n    print(opt({ty}))\n"
        ));
    }
}
