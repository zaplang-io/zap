//! The `Ty -> ast::Type` bridge and the `TypeMap` carrier (#1166), the shared
//! type data monomorphization consumes to pin generic arguments from sema's
//! inferred types (epic I#1169). These assert the bridge produces the exact
//! source spelling mono mangles against, and that the composite / unbridgeable
//! cases fail closed to `None`.

use zapc_sema_types::{Ty, TypeMap};
use zapc_syntax::ast::Type;
use zapc_syntax::span::Span;

/// Render an AST `Type` the way mono names it: `name`, `name[args]`, or a
/// `(a, b)` tuple. Enough to pin the bridge's output precisely.
fn render(t: &Type) -> String {
    match t {
        Type::Named { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                let inner: Vec<String> = args.iter().map(render).collect();
                format!("{}[{}]", name, inner.join(", "))
            }
        }
        Type::Tuple { elems, .. } => {
            let inner: Vec<String> = elems.iter().map(render).collect();
            format!("({})", inner.join(", "))
        }
        Type::Func { .. } => "fn".to_string(),
    }
}

fn bridged(ty: Ty) -> String {
    render(&ty.to_ast_type().expect("expected a bridgeable type"))
}

fn list(t: Ty) -> Ty {
    Ty::List(Box::new(t))
}

// ----- scalars -----

#[test]
fn scalars_bridge_to_their_source_names() {
    assert_eq!(bridged(Ty::Int64), "int64");
    assert_eq!(bridged(Ty::Int32), "int32");
    assert_eq!(bridged(Ty::Uint8), "uint8");
    assert_eq!(bridged(Ty::Uint64), "uint64");
    assert_eq!(bridged(Ty::Usize), "usize");
    assert_eq!(bridged(Ty::Float64), "float64");
    assert_eq!(bridged(Ty::Float32), "float32");
    assert_eq!(bridged(Ty::Bool), "bool");
    assert_eq!(bridged(Ty::Str), "str");
    assert_eq!(bridged(Ty::Bytes), "bytes");
    assert_eq!(bridged(Ty::Char), "char");
    assert_eq!(bridged(Ty::Bigint), "bigint");
    assert_eq!(bridged(Ty::Complex), "complex");
    assert_eq!(bridged(Ty::Range), "range");
}

#[test]
fn untyped_literals_take_their_default() {
    // An untyped int / float literal bridges to its concrete default (D-0003),
    // so a generic constructor pins `int64` / `float64` exactly as a bare
    // literal does today.
    assert_eq!(bridged(Ty::UntypedInt), "int64");
    assert_eq!(bridged(Ty::UntypedFloat), "float64");
}

// ----- collections -----

#[test]
fn collections_bridge_with_their_elements() {
    assert_eq!(bridged(list(Ty::Int64)), "list[int64]");
    assert_eq!(bridged(Ty::Set(Box::new(Ty::Str))), "set[str]");
    assert_eq!(bridged(Ty::Heap(Box::new(Ty::Int64))), "heap[int64]");
    assert_eq!(bridged(Ty::Deque(Box::new(Ty::Int64))), "deque[int64]");
    assert_eq!(bridged(Ty::OSet(Box::new(Ty::Str))), "oset[str]");
    assert_eq!(
        bridged(Ty::Dict(Box::new(Ty::Str), Box::new(Ty::Int64))),
        "dict[str, int64]"
    );
    assert_eq!(bridged(Ty::Chan(Box::new(Ty::Int64))), "channel[int64]");
    assert_eq!(
        bridged(Ty::AtomicArray(Box::new(Ty::Int64))),
        "AtomicArray[int64]"
    );
}

#[test]
fn tuples_and_nesting_bridge() {
    assert_eq!(bridged(Ty::Tuple(vec![Ty::Int64, Ty::Str])), "(int64, str)");
    assert_eq!(
        bridged(list(Ty::Tuple(vec![Ty::Int64, Ty::Str]))),
        "list[(int64, str)]"
    );
    assert_eq!(
        bridged(Ty::Dict(Box::new(Ty::Str), Box::new(list(Ty::Int64)))),
        "dict[str, list[int64]]"
    );
}

#[test]
fn named_types_pass_through() {
    // A user type, and a mangled generic instance name, both pass through
    // verbatim (sema `Ty::Named` already carries the mangled name for an
    // instance).
    assert_eq!(bridged(Ty::Named("Box".to_string())), "Box");
    assert_eq!(
        bridged(Ty::Named("Optional$int64".to_string())),
        "Optional$int64"
    );
}

#[test]
fn synthetic_sigil_names_are_none() {
    // #1190: a `$`-prefixed `Named` is a compiler-synthesized, unspellable type
    // (the generator-expression iterator `$genexpr`, a generator function's
    // state `$gen:<name>`, an interface fat-pointer layout `$iface:<name>`).
    // These must NOT bridge, so the sema-driven backfill never monomorphizes a
    // bogus `Foo$$genexpr` instance over them — the argument stays `Unknown`
    // and the generic is rejected with the normal `ZAP6001` (#1190).
    assert!(Ty::Named("$genexpr".to_string()).to_ast_type().is_none());
    assert!(Ty::Named("$gen:make".to_string()).to_ast_type().is_none());
    assert!(Ty::Named("$iface:Display".to_string())
        .to_ast_type()
        .is_none());
    // The guard is a narrow leading-`$` check, so a synthetic type nested inside
    // a composite also makes the whole thing fail closed.
    assert!(list(Ty::Named("$genexpr".to_string()))
        .to_ast_type()
        .is_none());
    assert!(
        Ty::Tuple(vec![Ty::Int64, Ty::Named("$genexpr".to_string())])
            .to_ast_type()
            .is_none()
    );
}

#[test]
fn ordinary_and_mangled_named_types_still_bridge() {
    // Regression guard for the #1190 fix: only a *leading* `$` is synthetic.
    // An ordinary user type, and a mangled generic instance whose `$` is in the
    // middle (`Optional$int64`), must still bridge so #1167 inference is intact.
    assert_eq!(bridged(Ty::Named("Box".to_string())), "Box");
    assert_eq!(
        bridged(Ty::Named("Optional$int64".to_string())),
        "Optional$int64"
    );
    assert_eq!(
        bridged(Ty::Named("Optional$$genexpr".to_string())),
        "Optional$$genexpr",
        "a mangled instance name is not itself a synthetic type — only a \
         *leading* `$` is the synthetic sigil"
    );
}

// ----- fail-closed: unbridgeable types and composites with an unbridgeable part

#[test]
fn unbridgeable_scalars_are_none() {
    assert!(Ty::Unknown.to_ast_type().is_none());
    assert!(Ty::Py.to_ast_type().is_none());
    assert!(Ty::Unit.to_ast_type().is_none());
    assert!(Ty::Match.to_ast_type().is_none());
    assert!(Ty::StrView.to_ast_type().is_none());
    assert!(Ty::BytesView.to_ast_type().is_none());
}

#[test]
fn composite_with_unbridgeable_element_is_none() {
    assert!(list(Ty::Unknown).to_ast_type().is_none());
    assert!(Ty::Dict(Box::new(Ty::Str), Box::new(Ty::Unknown))
        .to_ast_type()
        .is_none());
    assert!(Ty::Tuple(vec![Ty::Int64, Ty::Unknown])
        .to_ast_type()
        .is_none());
}

// ----- TypeMap carrier -----

#[test]
fn type_map_looks_up_by_span() {
    let a = Span::new(0, 3);
    let b = Span::new(5, 9);
    let map = TypeMap::from_records([(a, Ty::Int64), (b, Ty::Str)]);
    assert_eq!(map.len(), 2);
    assert!(!map.is_empty());
    assert_eq!(map.type_at(a), Some(&Ty::Int64));
    assert_eq!(map.type_at(b), Some(&Ty::Str));
    assert_eq!(map.type_at(Span::new(100, 101)), None);
}

#[test]
fn type_map_last_record_for_a_span_wins() {
    // Records are folded in visitation order; a later entry for the same span
    // (a re-inference, or a user expression after a colliding prelude one) wins.
    let s = Span::new(0, 3);
    let map = TypeMap::from_records([(s, Ty::Unknown), (s, Ty::Int64)]);
    assert_eq!(map.len(), 1);
    assert_eq!(map.type_at(s), Some(&Ty::Int64));
}

#[test]
fn empty_type_map() {
    let map = TypeMap::default();
    assert!(map.is_empty());
    assert_eq!(map.len(), 0);
    assert_eq!(map.type_at(Span::default()), None);
}
