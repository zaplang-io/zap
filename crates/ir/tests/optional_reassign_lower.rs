//! Type-contextual bare-payload / bare-`None` lowering for a **plain local (or
//! module-global) reassignment** `name = value` (#1362).
//!
//! `Optional.Some(payload)` boxing is applied at the single type-contextual
//! chokepoint (`expr_coerced` -> `expr_collection_context`) that constructor
//! fields, field assignment, index/key assignment, `.append`, call arguments
//! and returns all flow through (#1262, #1075, #1080, #1195). The plain
//! `Stmt::Assign` to a `Name` target lowered its right-hand side with a bare
//! `self.expr(value)` + `coerce`, so it never reached that chokepoint: a bare
//! payload `T` reassigned into an `Optional[T]` local stored the **raw**
//! scalar/pointer straight into the Optional slot (no discriminant tag, no
//! allocation), and a later `.is_some()` / `.unwrap()` read those raw bits as an
//! Optional object and SIGSEGV'd — a memory-safety violation in a checked
//! program (the issue's `0xffffffffffffd5d0` = the bit pattern of `-10800`).
//!
//! Sema already accepts `offset = seconds` in exactly the positions #1262
//! accepts the bare payload, so the defect was purely in lowering. These
//! host-only checks (no LLVM) pin the *shape* of the lowered IR: every store
//! into the Optional local must store a whole **`Optional`-typed** value (the
//! `Some`/`None` box), never a raw payload word. The end-to-end runtime
//! *values* (`is_some`/`is_none`/`unwrap`) are covered by the
//! `optional_reassign` golden on CI, since the tagged-vs-raw representation only
//! matters once the native collector runs.

use zapc_ir::{lower_module, Inst, Ty};
use zapc_syntax::parser::parse_str;

/// Parse + lower `src` (asserting it is clean) and return its `main` function's
/// lowered IR together with the struct id of the `Optional[...]` whose mangled
/// name is `opt_name` (e.g. `"Optional$int64"`).
fn lower_main(src: &str, opt_name: &str) -> (Vec<Inst>, Vec<Ty>, u32) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    let opt_sid = module
        .structs
        .iter()
        .position(|s| s.name == opt_name)
        .unwrap_or_else(|| panic!("no monomorphized struct named {opt_name}"))
        as u32;
    let main = module
        .funcs
        .iter()
        .find(|f| f.is_main)
        .expect("a lowered program has a `main`");
    let insts: Vec<Inst> = main.blocks.iter().flat_map(|b| b.insts.clone()).collect();
    (insts, main.value_types.clone(), opt_sid)
}

/// How many `Store` instructions in `main` store a value whose ZIR type is the
/// whole `Optional[...]` struct `opt_sid`. A correctly-boxed reassignment stores
/// the `Some`/`None` aggregate (an `Optional`-typed value); the #1362 bug stored
/// the raw payload word (an `i64`, a `ptr`, ...) into the same slot, which is
/// *not* counted here.
fn optional_typed_stores(insts: &[Inst], value_types: &[Ty], opt_sid: u32) -> usize {
    insts
        .iter()
        .filter(|i| match i {
            Inst::Store { val, .. } => value_types[*val as usize] == Ty::Struct(opt_sid),
            _ => false,
        })
        .count()
}

/// How many `Store` instructions in `main` store a raw scalar `i64` value. Used
/// to pin that the buggy raw-payload store is gone for a scalar payload.
fn i64_stores(insts: &[Inst], value_types: &[Ty]) -> usize {
    insts
        .iter()
        .filter(|i| match i {
            Inst::Store { val, .. } => value_types[*val as usize] == Ty::I64,
            _ => false,
        })
        .count()
}

// ----- The issue's exact minimal repro -----

#[test]
fn scalar_reassign_boxes_some_not_raw() {
    // `offset = seconds` must box `Some(seconds)`: the `None` initializer and the
    // reassignment each store a whole `Optional$int64` value (two Optional-typed
    // stores), never the raw `i64`. Before the fix the reassignment stored the
    // raw `i64` straight into the slot, so there was only ONE Optional-typed
    // store (the `None` init) and an extra raw-`i64` store into that slot.
    let (insts, vts, opt) = lower_main(
        "def main():\n\
        \x20   int64 seconds = -10800\n\
        \x20   Optional[int64] offset = None\n\
        \x20   offset = seconds\n\
        \x20   assert offset.is_some()\n\
        \x20   assert offset.unwrap() == -10800\n",
        "Optional$int64",
    );
    assert!(
        optional_typed_stores(&insts, &vts, opt) >= 2,
        "the `None` init and the `offset = seconds` reassignment must each store \
         a boxed `Optional$int64` value; the raw-payload store means the Some-wrap \
         was skipped (#1362)"
    );
    // The scalar `seconds` is still stored once (into its own `int64` slot), but
    // it must NOT also be stored raw into the Optional slot: exactly one i64
    // store, not two.
    assert_eq!(
        i64_stores(&insts, &vts),
        1,
        "the only raw-`i64` store should be `seconds`'s own binding; a second one \
         is the raw payload leaking into the Optional slot (#1362)"
    );
}

// ----- Zero payload: a raw 0 must not read back as None -----

#[test]
fn scalar_reassign_zero_payload_boxes_some() {
    // `None -> Some(0)`: the discriminant tag is the only thing distinguishing a
    // `Some(0)` from a `None` for a scalar payload, so a raw `0` store would be
    // silently mis-read as `None`.
    let (insts, vts, opt) = lower_main(
        "def main():\n\
        \x20   int64 zero = 0\n\
        \x20   Optional[int64] slot = None\n\
        \x20   slot = zero\n\
        \x20   assert slot.is_some()\n\
        \x20   assert slot.unwrap() == 0\n",
        "Optional$int64",
    );
    assert!(
        optional_typed_stores(&insts, &vts, opt) >= 2,
        "a zero payload must still box as a tagged `Some(0)` (#1362)"
    );
}

// ----- Reassignment back to None, then to Some again -----

#[test]
fn scalar_reassign_none_then_some_each_boxed() {
    // Start Some, reassign to a bare `None`, then back to a bare payload: every
    // store into the local must be a whole-`Optional` value.
    let (insts, vts, opt) = lower_main(
        "def main():\n\
        \x20   int64 n = 5\n\
        \x20   Optional[int64] slot = Optional.Some(1)\n\
        \x20   slot = None\n\
        \x20   assert slot.is_none()\n\
        \x20   slot = n\n\
        \x20   assert slot.unwrap() == 5\n",
        "Optional$int64",
    );
    // Some(1) init + `= None` + `= n` = three Optional-typed stores.
    assert!(
        optional_typed_stores(&insts, &vts, opt) >= 3,
        "each of the `Some(1)` init, the `= None`, and the `= n` reassignment \
         must store a whole `Optional$int64` value (#1362)"
    );
}

// ----- INT_MIN payload (the negative-extreme end of the issue's `-10800`) -----

#[test]
fn scalar_reassign_int_min_boxes_some() {
    let (insts, vts, opt) = lower_main(
        "def main():\n\
        \x20   int64 lo = -9223372036854775808\n\
        \x20   Optional[int64] slot = None\n\
        \x20   slot = lo\n\
        \x20   assert slot.unwrap() == -9223372036854775808\n",
        "Optional$int64",
    );
    assert!(
        optional_typed_stores(&insts, &vts, opt) >= 2,
        "INT_MIN must box as `Some` like any other payload (#1362)"
    );
}

// ----- Reference (pointer) payload: a raw pointer store hides the same bug -----

#[test]
fn reference_str_reassign_boxes_some_not_raw() {
    // `Optional[str]` is a pointer-payload (niche) Optional: the `Some` value is
    // the payload pointer reinterpreted as the Optional type, and `None` is the
    // null pointer. A raw `ptr` store (the bug) would be indistinguishable from a
    // `None` for a null string and mis-typed for `is_some`/`unwrap`. Both stores
    // must be `Optional$str`-typed.
    let (insts, vts, opt) = lower_main(
        "def main():\n\
        \x20   str s = \"hi\"\n\
        \x20   Optional[str] name = None\n\
        \x20   name = s\n\
        \x20   assert name.is_some()\n\
        \x20   assert name.unwrap() == \"hi\"\n",
        "Optional$str",
    );
    assert!(
        optional_typed_stores(&insts, &vts, opt) >= 2,
        "the `None` init and the `name = s` reassignment must each store a whole \
         `Optional$str` value, never the raw `str` pointer (#1362)"
    );
}

// ----- Reference struct payload -----

#[test]
fn reference_struct_reassign_boxes_some_not_raw() {
    let (insts, vts, opt) = lower_main(
        "struct Item:\n\
        \x20   int64 value\n\
        def main():\n\
        \x20   Item it = Item(42)\n\
        \x20   Optional[Item] slot = None\n\
        \x20   slot = it\n\
        \x20   assert slot.is_some()\n\
        \x20   assert slot.unwrap().value == 42\n",
        "Optional$Item",
    );
    assert!(
        optional_typed_stores(&insts, &vts, opt) >= 2,
        "a bare struct payload reassigned into an `Optional[Item]` local must box \
         as `Some(Item)` (#1362)"
    );
}

// ----- A module-level mutable global target takes the same path -----

#[test]
fn global_reassign_boxes_some_not_raw() {
    // `Stmt::Assign` to a `Name` also covers a module-level mutable global's slot
    // (D-0112). It must box identically.
    let (insts, vts, opt) = lower_main(
        "Optional[int64] slot = None\n\
        def main():\n\
        \x20   int64 n = -10800\n\
        \x20   slot = n\n\
        \x20   assert slot.unwrap() == -10800\n",
        "Optional$int64",
    );
    assert!(
        optional_typed_stores(&insts, &vts, opt) >= 1,
        "reassigning a bare payload into a global `Optional[int64]` must box as \
         `Some` (#1362)"
    );
    assert_eq!(
        i64_stores(&insts, &vts),
        1,
        "`n`'s own binding is the only raw-`i64` store; the payload must not also \
         be stored raw into the global's Optional slot (#1362)"
    );
}

// ----- An already-Optional source is not double-wrapped -----

#[test]
fn reassign_from_optional_source_not_double_wrapped() {
    // Reassigning from another `Optional[int64]` value must copy the Optional
    // through unchanged — never wrap it into `Some(Some(..))`. The source load is
    // already `Optional$int64`-typed, so no extra nested-Optional struct is
    // created.
    let (ast, perrs, lerrs) = parse_str(
        "def main():\n\
        \x20   Optional[int64] a = Optional.Some(6)\n\
        \x20   Optional[int64] b = None\n\
        \x20   b = a\n\
        \x20   assert b.unwrap() == 6\n",
    );
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    // No `Optional$Optional$int64` (a double-wrap) is ever monomorphized.
    assert!(
        !module
            .structs
            .iter()
            .any(|s| s.name.contains("Optional$Optional")),
        "an Optional-typed source must not be re-wrapped into a nested Optional \
         on reassignment (#1362)"
    );
}
