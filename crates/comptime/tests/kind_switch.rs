//! `type_info(T).kind` switch + container recursion (#1253): comptime branching
//! on `.kind` and resolving a container's element/key/value/inner/elems type
//! back to a nested `TypeInfo`, so a kind-switch handles container families
//! without any conformance machinery.
//!
//! Intent documentation: each `.kind` leaf; each container recursing one level;
//! nested containers; declaration-order recursion (dict walks key-then-value
//! structurally, never by iterating a map — determinism, #1066); a depth-bounded
//! walk terminating on a recursive type; and the override hook
//! (`reflect.has_override`) short-circuiting.

mod common;

use common::{parse_expr, parse_structs};
use std::rc::Rc;
use zapc_comptime::{
    seed_kind_constants, type_info_of_struct, Env, Evaluator, TypeInfo, TypeRegistry, Value,
};

const MODULE: &str = "\
struct P:\n\
\x20   int64 x\n\
\x20   int64 y\n\
struct Shapes:\n\
\x20   int64 n\n\
\x20   P p\n\
\x20   list[int64] xs\n\
\x20   dict[str, int64] m\n\
\x20   Optional[int64] opt\n\
\x20   (int64, str) pair\n\
\x20   list[P] ps\n\
\x20   list[list[P]] pps\n\
\x20   dict[str, list[P]] dps\n";

fn registry() -> TypeRegistry {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(MODULE);
    assert!(lerrs.is_empty() && perrs.is_empty(), "{lerrs:?} {perrs:?}");
    TypeRegistry::from_module(&module)
}

fn shapes_info() -> TypeInfo {
    let s = parse_structs(MODULE)
        .into_iter()
        .find(|d| d.name == "Shapes")
        .unwrap();
    type_info_of_struct(&s)
}

/// Evaluate `expr_src` with `info` = Shapes, the registry attached, and the
/// kind constants seeded.
fn eval_k(reg: &TypeRegistry, info: &TypeInfo, expr_src: &str) -> Value {
    let e = parse_expr(expr_src);
    let mut env = Env::new();
    env.define("info", Value::Type(Rc::new(info.clone())));
    seed_kind_constants(&mut env);
    let mut ev = Evaluator::new().with_registry(reg);
    ev.eval(&e, &mut env)
        .unwrap_or_else(|err| panic!("eval `{expr_src}` failed: {err:?}"))
}

// ---------------------------------------------------------------------------
// each .kind leaf
// ---------------------------------------------------------------------------

#[test]
fn field_kinds() {
    let (reg, info) = (registry(), shapes_info());
    // n:int64 Primitive, p:P Struct, xs:list List, m:dict Dict, opt:Optional,
    // pair:(..) Tuple.
    let k = |i: usize| eval_k(&reg, &info, &format!("info.fields[{i}].ty.kind.name"));
    assert_eq!(k(0), Value::str("primitive"));
    assert_eq!(k(1), Value::str("struct"));
    assert_eq!(k(2), Value::str("list"));
    assert_eq!(k(3), Value::str("dict"));
    assert_eq!(k(4), Value::str("optional"));
    assert_eq!(k(5), Value::str("tuple"));
}

#[test]
fn kind_equality_against_seeded_constants() {
    let (reg, info) = (registry(), shapes_info());
    assert_eq!(
        eval_k(&reg, &info, "info.fields[1].ty.kind == Struct"),
        Value::Bool(true)
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[2].ty.kind == List"),
        Value::Bool(true)
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[2].ty.kind == Dict"),
        Value::Bool(false)
    );
    assert_eq!(
        eval_k(&reg, &info, "info.kind == Struct"),
        Value::Bool(true)
    );
}

#[test]
fn enum_kind() {
    let (module, _, _) = zapc_syntax::parser::parse_str("enum Color:\n    RED\n    GREEN\n");
    let reg = TypeRegistry::from_module(&module);
    // Reflect the enum directly through the registry's resolve_type.
    let info = reg
        .resolve_type(
            &zapc_syntax::ast::Type::Named {
                name: "Color".to_string(),
                args: vec![],
                span: zapc_syntax::span::Span::new(0, 0),
            },
            zapc_syntax::span::Span::new(0, 0),
        )
        .unwrap();
    assert_eq!(info.kind(), zapc_comptime::TypeKind::Enum);
    assert_eq!(info.variants().len(), 2);
}

// ---------------------------------------------------------------------------
// container recursion — one level
// ---------------------------------------------------------------------------

#[test]
fn list_recurses_to_element() {
    let (reg, info) = (registry(), shapes_info());
    assert_eq!(
        eval_k(&reg, &info, "info.fields[2].ty.elem.kind.name"),
        Value::str("primitive")
    );
    // list[P] -> elem is the struct P, recurse into its fields.
    assert_eq!(
        eval_k(&reg, &info, "info.fields[6].ty.elem.kind.name"),
        Value::str("struct")
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[6].ty.elem.name"),
        Value::str("P")
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[6].ty.elem.fields[0].name"),
        Value::str("x")
    );
}

#[test]
fn dict_recurses_key_then_value_structurally() {
    let (reg, info) = (registry(), shapes_info());
    // Walk is key-then-value structurally (NOT by iterating a map) — determinism.
    assert_eq!(
        eval_k(&reg, &info, "info.fields[3].ty.key.kind.name"),
        Value::str("primitive")
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[3].ty.key.name"),
        Value::str("str")
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[3].ty.value.kind.name"),
        Value::str("primitive")
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[3].ty.value.name"),
        Value::str("int64")
    );
}

#[test]
fn optional_recurses_to_inner() {
    let (reg, info) = (registry(), shapes_info());
    assert_eq!(
        eval_k(&reg, &info, "info.fields[4].ty.inner.kind.name"),
        Value::str("primitive")
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[4].ty.inner.name"),
        Value::str("int64")
    );
}

#[test]
fn tuple_recurses_over_elems() {
    let (reg, info) = (registry(), shapes_info());
    assert_eq!(
        eval_k(&reg, &info, "len(info.fields[5].ty.elems)"),
        Value::Int(2)
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[5].ty.elems[0].name"),
        Value::str("int64")
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[5].ty.elems[1].name"),
        Value::str("str")
    );
}

// ---------------------------------------------------------------------------
// nested containers
// ---------------------------------------------------------------------------

#[test]
fn nested_list_of_list_of_struct() {
    let (reg, info) = (registry(), shapes_info());
    // pps: list[list[P]]
    assert_eq!(
        eval_k(&reg, &info, "info.fields[7].ty.kind.name"),
        Value::str("list")
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[7].ty.elem.kind.name"),
        Value::str("list")
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[7].ty.elem.elem.kind.name"),
        Value::str("struct")
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[7].ty.elem.elem.name"),
        Value::str("P")
    );
}

#[test]
fn nested_dict_of_str_to_list_of_struct() {
    let (reg, info) = (registry(), shapes_info());
    // dps: dict[str, list[P]]
    assert_eq!(
        eval_k(&reg, &info, "info.fields[8].ty.value.kind.name"),
        Value::str("list")
    );
    assert_eq!(
        eval_k(&reg, &info, "info.fields[8].ty.value.elem.name"),
        Value::str("P")
    );
}

// ---------------------------------------------------------------------------
// kind-switch expressed as a comptime function
// ---------------------------------------------------------------------------

#[test]
fn kind_switch_in_a_comptime_function() {
    // A comptime kind-switch using the seeded `Kind` constants.
    let classify = "\
def classify(str info):\n\
\x20   if info.kind == List:\n\
\x20       return \"list\"\n\
\x20   if info.kind == Struct:\n\
\x20       return \"struct\"\n\
\x20   if info.kind == Primitive:\n\
\x20       return \"primitive\"\n\
\x20   return \"other\"\n";
    let fns = common::parse_fns(classify);
    let reg = registry();
    let mut ev = Evaluator::with_functions(&fns).with_registry(&reg);
    // classify the struct P (a Struct), a list field's type, and a primitive.
    let p = reg.resolve_type(&named("P"), sp()).unwrap();
    assert_eq!(
        ev.call("classify", vec![Value::Type(Rc::new(p))], sp())
            .unwrap(),
        Value::str("struct")
    );
    let list_ty = Rc::new(
        reg.resolve_ty(
            &zapc_sema_types::Ty::List(Box::new(zapc_sema_types::Ty::Int64)),
            sp(),
        )
        .unwrap(),
    );
    assert_eq!(
        ev.call("classify", vec![Value::Type(list_ty)], sp())
            .unwrap(),
        Value::str("list")
    );
    let prim_ti = reg.resolve_ty(&zapc_sema_types::Ty::Int64, sp()).unwrap();
    assert_eq!(
        ev.call("classify", vec![Value::Type(Rc::new(prim_ti))], sp())
            .unwrap(),
        Value::str("primitive")
    );
}

// ---------------------------------------------------------------------------
// override hook
// ---------------------------------------------------------------------------

#[test]
fn override_hook_short_circuits() {
    let src = "\
struct Custom:\n\
\x20   int64 n\n\
\x20   def serialize(self):\n\
\x20       return \"x\"\n\
struct Plain:\n\
\x20   int64 n\n";
    let (module, _, _) = zapc_syntax::parser::parse_str(src);
    let reg = TypeRegistry::from_module(&module);
    assert!(reg.has_method("Custom", "serialize"));
    assert!(!reg.has_method("Plain", "serialize"));

    let encode = "\
def encode(str info):\n\
\x20   if reflect.has_override(info, \"serialize\"):\n\
\x20       return \"override\"\n\
\x20   return \"reflect\"\n";
    let fns = common::parse_fns(encode);
    let mut ev = Evaluator::with_functions(&fns).with_registry(&reg);
    let custom = reg.resolve_type(&named("Custom"), sp()).unwrap();
    let plain = reg.resolve_type(&named("Plain"), sp()).unwrap();
    assert_eq!(
        ev.call("encode", vec![Value::Type(Rc::new(custom))], sp())
            .unwrap(),
        Value::str("override")
    );
    assert_eq!(
        ev.call("encode", vec![Value::Type(Rc::new(plain))], sp())
            .unwrap(),
        Value::str("reflect")
    );
}

// ---------------------------------------------------------------------------
// depth / cycle guard — a recursive type shape terminates
// ---------------------------------------------------------------------------

#[test]
fn recursive_type_is_observable_and_walk_can_be_bounded() {
    // `struct Node: list[Node] kids` — a self-referential shape.
    let src = "struct Node:\n    int64 v\n    list[Node] kids\n";
    let (module, _, _) = zapc_syntax::parser::parse_str(src);
    let reg = TypeRegistry::from_module(&module);
    let node = reg.resolve_type(&named("Node"), sp()).unwrap();

    // A depth-bounded structural walk: Node -> field kids (list[Node]) -> elem
    // (Node) -> ... A fixed cap terminates it with a diagnostic (the centralized
    // cap is #1254); here the walk itself bounds the recursion.
    fn walk(info: &TypeInfo, reg: &TypeRegistry, depth: usize) -> Result<usize, String> {
        if depth > 8 {
            return Err("comptime type recursion exceeded depth cap".to_string());
        }
        let mut leaves = 0;
        match info.kind() {
            zapc_comptime::TypeKind::Struct => {
                for f in info.fields() {
                    let fty = reg.resolve_type(&f.decl_ty, sp()).map_err(|e| e.message)?;
                    leaves += walk(&fty, reg, depth + 1)?;
                }
            }
            zapc_comptime::TypeKind::List => {
                if let zapc_comptime::TypeShape::List { elem } = &info.shape {
                    let ety = reg.resolve_ty(elem, sp()).map_err(|e| e.message)?;
                    leaves += walk(&ety, reg, depth + 1)?;
                }
            }
            _ => leaves += 1,
        }
        Ok(leaves)
    }

    let err = walk(&node, &reg, 0).unwrap_err();
    assert!(err.contains("depth cap"), "{err}");
    // The cycle is observable: Node's kids element resolves back to a Node struct.
    let info = node.clone();
    let reg2 = reg.clone();
    let elem = eval_cycle(&reg2, &info);
    assert_eq!(elem, "Node");
}

fn eval_cycle(reg: &TypeRegistry, node: &TypeInfo) -> String {
    let e = parse_expr("info.fields[1].ty.elem.name");
    let mut env = Env::new();
    env.define("info", Value::Type(Rc::new(node.clone())));
    let mut ev = Evaluator::new().with_registry(reg);
    match ev.eval(&e, &mut env).unwrap() {
        Value::Str(s) => s.to_string(),
        other => panic!("{other:?}"),
    }
}

fn named(name: &str) -> zapc_syntax::ast::Type {
    zapc_syntax::ast::Type::Named {
        name: name.to_string(),
        args: vec![],
        span: sp(),
    }
}

fn sp() -> zapc_syntax::span::Span {
    zapc_syntax::span::Span::new(0, 0)
}
