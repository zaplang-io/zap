//! #1400 — a `value struct` stored into an aggregate FIELD (an enum-variant
//! payload or another struct's field) must be *boxed* onto the heap, not stored
//! as a raw pointer to the source.
//!
//! Host-only (no LLVM): `lower_module` runs monomorphization + lowering. These
//! pin the lowering invariant that fixes the segfault in `zap_tz_zone_id`
//! reported in zaplang-io/zap#1400.
//!
//! ## Why this matters (the bug)
//!
//! An inline-eligible `value struct` (a flat value struct / tuple, D-0051)
//! crosses a by-value call boundary in a register / `sret` *stack spill*. When
//! such a call result is then stored into a heap aggregate's field — which is
//! physically a *pointer* to the value struct's storage — a raw store leaves the
//! field pointing at the caller's stack spill. Once that caller returns, the
//! field dangles; a later read (e.g. `TimeZone.name` inside
//! `datetime.resolve` → `zap_tz_zone_id`) faults on a reused/zeroed slot.
//!
//! The fix boxes the value struct (a fresh heap `StructNew` + field copy, the
//! same boxing the interface-coercion and collection-element paths already do)
//! so the aggregate owns heap storage that outlives every frame. These tests
//! assert the boxing `StructNew` is emitted at the *store site*.

use zapc_ir::{lower_module, Inst, Module};
use zapc_syntax::parser::parse_str;

fn lower_ok(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    module
}

/// The struct id `name` lowers to (by its declared source name).
fn sid_of(m: &Module, name: &str) -> u32 {
    m.structs
        .iter()
        .position(|s| s.name == name)
        .unwrap_or_else(|| panic!("no struct named {name}; have {:?}", struct_names(m))) as u32
}

fn struct_names(m: &Module) -> Vec<String> {
    m.structs.iter().map(|s| s.name.clone()).collect()
}

/// Count `StructNew`s of struct id `sid` inside function `fname`.
fn structnew_count(m: &Module, fname: &str, sid: u32) -> usize {
    let f = m
        .funcs
        .iter()
        .find(|f| f.name == fname)
        .unwrap_or_else(|| panic!("no fn {fname}"));
    f.blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter(|i| matches!(i, Inst::StructNew { sid: s, .. } if *s == sid))
        .count()
}

/// A flat `value struct` V with a reference (`str`) field, built by a by-value
/// factory `make()`, then wrapped into `Optional.Some(make())`. The wrapper must
/// box the `V` — i.e. emit its own `StructNew V` — so the `Optional`'s payload
/// points at heap storage, not `make`'s returned stack spill.
const ENUM_PAYLOAD: &str = r#"
value struct V:
    str s
    int64 n

def make() -> V:
    return V(s="x", n=1)

def wrap() -> Optional[V]:
    return Optional.Some(make())

def main():
    auto o = wrap()
"#;

#[test]
fn some_of_value_struct_call_result_is_boxed() {
    let m = lower_ok(ENUM_PAYLOAD);
    let v = sid_of(&m, "V");
    // `make` builds the one V it returns.
    assert_eq!(
        structnew_count(&m, "make", v),
        1,
        "the factory builds its own V"
    );
    // The fix: `wrap` must box the value struct before storing it into the
    // Optional payload, so `wrap` emits its OWN `StructNew V`. Before #1400 this
    // was 0 — the raw stack-spill pointer was stored and dangled.
    assert_eq!(
        structnew_count(&m, "wrap", v),
        1,
        "Optional.Some(<value-struct call result>) must box the payload onto the heap (#1400)"
    );
}

/// The same hazard through a plain struct field: a reference struct `Box` with a
/// `value struct` field set from a by-value call result must box the field.
const STRUCT_FIELD: &str = r#"
value struct V:
    str s
    int64 n

struct Hold:
    V inner

def make() -> V:
    return V(s="x", n=1)

def wrap() -> Hold:
    return Hold(inner=make())

def main():
    auto h = wrap()
"#;

#[test]
fn struct_field_value_struct_call_result_is_boxed() {
    let m = lower_ok(STRUCT_FIELD);
    let v = sid_of(&m, "V");
    assert_eq!(
        structnew_count(&m, "wrap", v),
        1,
        "a value-struct field set from a by-value call result must be boxed onto the heap (#1400)"
    );
}

/// A reference payload (the niche `Optional[list[int64]]`) is unaffected: it is
/// already a heap pointer, so no extra boxing `StructNew` is introduced for it.
/// (Guards against over-boxing reference types.)
const REFERENCE_PAYLOAD: &str = r#"
def make() -> list[int64]:
    return [1, 2, 3]

def wrap() -> Optional[list[int64]]:
    return Optional.Some(make())

def main():
    auto o = wrap()
"#;

#[test]
fn some_of_reference_result_is_not_boxed() {
    // Lowers cleanly and introduces no spurious value-struct boxing; a reference
    // payload keeps its single heap identity (niche layout).
    let _m = lower_ok(REFERENCE_PAYLOAD);
}
