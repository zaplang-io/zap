//! Comptime build-time-cost controls (#1255, HQ5): `type_info(T)` memoization,
//! specialization dedup, and a deterministic compile-time metric.
//!
//! Intent documentation: the same `T` is computed once however many times it is
//! introspected; distinct types are each computed once; the `type_key` memo does
//! not collide distinct parameterized types; an identical `(type, backend)`
//! specialization is emitted once while distinct backends are distinct; and a
//! reflection-heavy program's memo stats are a stable, comparable
//! compile-time metric (the CI regression signal).

mod common;

use zapc_comptime::{
    type_info_of_struct, Env, Evaluator, SpecializationCache, TypeRegistry, Value,
};
use zapc_syntax::ast::Type;
use zapc_syntax::span::Span;

fn sp() -> Span {
    Span::new(0, 0)
}

fn named(name: &str, args: Vec<Type>) -> Type {
    Type::Named {
        name: name.to_string(),
        args,
        span: sp(),
    }
}

const MODULE: &str = "\
struct Point:\n\
\x20   int64 x\n\
\x20   int64 y\n\
struct Line:\n\
\x20   Point a\n\
\x20   Point b\n";

fn registry() -> TypeRegistry {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(MODULE);
    assert!(lerrs.is_empty() && perrs.is_empty(), "{lerrs:?} {perrs:?}");
    TypeRegistry::from_module(&module)
}

// ---------------------------------------------------------------------------
// type_info(T) memoization
// ---------------------------------------------------------------------------

#[test]
fn same_type_introspected_n_times_is_computed_once() {
    let reg = registry();
    for _ in 0..10 {
        reg.resolve_type(&named("Point", vec![]), sp()).unwrap();
    }
    let s = reg.memo_stats();
    assert_eq!(s.computed, 1, "Point must be computed exactly once");
    assert_eq!(s.reused, 9);
    assert_eq!(s.total(), 10);
}

#[test]
fn distinct_types_each_computed_once() {
    let reg = registry();
    for _ in 0..3 {
        reg.resolve_type(&named("Point", vec![]), sp()).unwrap();
        reg.resolve_type(&named("Line", vec![]), sp()).unwrap();
        reg.resolve_type(&named("int64", vec![]), sp()).unwrap();
    }
    let s = reg.memo_stats();
    assert_eq!(s.computed, 3, "Point, Line, int64 each computed once");
    assert_eq!(s.reused, 6);
}

#[test]
fn parameterized_types_do_not_collide_in_the_memo() {
    let reg = registry();
    // Optional[int64] and Optional[str] must be two distinct records, not one.
    reg.resolve_type(&named("Optional", vec![named("int64", vec![])]), sp())
        .unwrap();
    reg.resolve_type(&named("Optional", vec![named("str", vec![])]), sp())
        .unwrap();
    // list[int64] vs list[str] likewise.
    reg.resolve_type(&named("list", vec![named("int64", vec![])]), sp())
        .unwrap();
    reg.resolve_type(&named("list", vec![named("str", vec![])]), sp())
        .unwrap();
    assert_eq!(reg.memo_stats().computed, 4, "no key collisions");
}

#[test]
fn memo_is_shared_through_the_evaluator() {
    // Driving `field.ty` through the evaluator hits the same memo.
    let s = parse_point_struct();
    let info = type_info_of_struct(&s);
    let reg = registry();
    let mut ev = Evaluator::new().with_registry(&reg);
    for _ in 0..5 {
        let e = common::parse_expr("info.fields[0].ty.kind.name");
        let mut env = Env::new();
        env.define("info", Value::Type(std::rc::Rc::new(info.clone())));
        let v = ev.eval(&e, &mut env).unwrap();
        assert_eq!(v, Value::str("primitive")); // x: int64
    }
    // int64 (the field type) computed once across the five introspections.
    assert_eq!(reg.memo_stats().computed, 1);
    assert_eq!(reg.memo_stats().reused, 4);
}

fn parse_point_struct() -> zapc_syntax::ast::StructDef {
    common::parse_structs(MODULE)
        .into_iter()
        .find(|d| d.name == "Point")
        .unwrap()
}

// ---------------------------------------------------------------------------
// specialization dedup (mono #671 model)
// ---------------------------------------------------------------------------

#[test]
fn identical_specialization_emitted_once() {
    let mut cache = SpecializationCache::new();
    assert!(cache.record("Point", "llvm"), "first time -> emit");
    assert!(!cache.record("Point", "llvm"), "reached again -> dedup");
    assert!(!cache.record("Point", "llvm"));
    assert_eq!(cache.len(), 1);
}

#[test]
fn distinct_backends_are_distinct_instantiations() {
    let mut cache = SpecializationCache::new();
    assert!(cache.record("Point", "llvm"));
    assert!(
        cache.record("Point", "wasm"),
        "distinct backend -> new instantiation"
    );
    assert!(cache.contains("Point", "llvm"));
    assert!(cache.contains("Point", "wasm"));
    assert!(!cache.contains("Point", "js"));
    assert_eq!(cache.len(), 2);
}

// ---------------------------------------------------------------------------
// CI compile-time metric — a reflection-heavy program
// ---------------------------------------------------------------------------

#[test]
fn reflection_heavy_program_metric_is_stable() {
    // A reflection-heavy module: several structs referencing one another and
    // containers. Reflect every type K times (as a codec would across many call
    // sites); the memo computes each distinct type exactly once.
    let src = "\
struct A:\n    int64 n\n    str s\n\
struct B:\n    A a\n    list[A] xs\n\
struct C:\n    B b\n    dict[str, A] m\n    Optional[int64] o\n";
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(lerrs.is_empty() && perrs.is_empty());
    let reg = TypeRegistry::from_module(&module);

    const K: u64 = 50;
    let types = [
        named("A", vec![]),
        named("B", vec![]),
        named("C", vec![]),
        named("int64", vec![]),
        named("str", vec![]),
        named("list", vec![named("A", vec![])]),
        named("dict", vec![named("str", vec![]), named("A", vec![])]),
        named("Optional", vec![named("int64", vec![])]),
    ];
    for _ in 0..K {
        for t in &types {
            reg.resolve_type(t, sp()).unwrap();
        }
    }
    let s = reg.memo_stats();
    // The tracked metric: distinct records built == number of distinct types,
    // independent of how many times each is introspected. A regression (e.g. a
    // lost memo) would blow `computed` up to K * distinct and fail here.
    assert_eq!(
        s.computed,
        types.len() as u64,
        "metric: distinct type records"
    );
    assert_eq!(s.total(), K * types.len() as u64);
    assert_eq!(s.reused, (K - 1) * types.len() as u64);
    // Emit the metric for CI visibility.
    println!(
        "comptime-reflection-metric computed={} reused={} total={}",
        s.computed,
        s.reused,
        s.total()
    );
}

#[test]
fn many_field_struct_stays_bounded() {
    // A wide struct: reflecting it computes one record for the struct plus one
    // per distinct field type — bounded by the declaration, within the #1254
    // cap and a small compile-time budget.
    let mut src = String::from("struct Wide:\n");
    for i in 0..64 {
        src.push_str(&format!("    int64 f{i}\n"));
    }
    let (module, _, _) = zapc_syntax::parser::parse_str(&src);
    let reg = TypeRegistry::from_module(&module);
    reg.resolve_type(&named("Wide", vec![]), sp()).unwrap();
    // Reflecting each of the 64 fields' types: all int64, so after the struct
    // itself only int64 is computed once more — 2 distinct records total.
    for _ in 0..64 {
        reg.resolve_type(&named("int64", vec![]), sp()).unwrap();
    }
    let s = reg.memo_stats();
    assert_eq!(s.computed, 2, "Wide + int64, computed once each");
}
