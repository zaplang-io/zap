//! Monomorphization can reach the shared type carrier (#1166): `TypeMap` and
//! the `Ty -> ast::Type` bridge are available through `zapc_mono` (re-exported
//! from `zapc_sema_types`), so `zapc_ir` — which depends on `zapc_mono`, not on
//! the sema checker — gets them too. This is the data-only sema->ir boundary the
//! epic (I#1169) relies on; mono threads them into inference in #1167.

use zapc_mono::{SemaTy, TypeMap};
use zapc_syntax::span::Span;

#[test]
fn bridge_reachable_from_mono() {
    // The bridge produces the AST type mono mangles against.
    let t = SemaTy::List(Box::new(SemaTy::Int64))
        .to_ast_type()
        .expect("list[int64] bridges");
    // A `list[int64]` is a Named type with one argument.
    match t {
        zapc_syntax::ast::Type::Named { name, args, .. } => {
            assert_eq!(name, "list");
            assert_eq!(args.len(), 1);
        }
        other => panic!("expected a Named type, got {other:?}"),
    }
}

#[test]
fn type_map_reachable_from_mono() {
    let s = Span::new(0, 1);
    let map = TypeMap::from_records([(s, SemaTy::Int64)]);
    assert_eq!(map.type_at(s), Some(&SemaTy::Int64));
}
