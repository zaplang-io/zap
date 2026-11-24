//! #1446 — reassigning a *returned* `value struct` to an ALREADY-EXISTING
//! aggregate field (`obj.field = f()`) must *box* the value struct onto the
//! heap, not store the raw pointer to the producer's stack spill.
//!
//! Host-only (no LLVM): `lower_module` runs monomorphization + lowering. These
//! pin the lowering invariant for the one field-store site that #1400's
//! `copy_field_value` fix did NOT reach — the field-assignment statement arm in
//! `crates/ir/src/lower/stmts.rs` (as opposed to construction `Struct(f=...)`,
//! enum-variant payloads, and the `#1262` `Optional` auto-wrap, which #1400
//! already boxed).
//!
//! ## Why this matters (the bug)
//!
//! An inline-eligible `value struct` (a flat value struct / tuple, D-0051)
//! crosses a by-value call/return boundary in a register / `sret` *stack spill*.
//! A field physically holds a *pointer* to its value struct's storage
//! (`basic(Ty::Struct) == ptr`). Reassigning a call result straight into an
//! existing field stored that spill's pointer; once the producing frame
//! returned the field *dangled*, and a later read came back corrupted (the
//! zaplang-io/zap#1446 `_ProbeCursor` repro: a cached returned `DateTime` seed
//! read back as `0000-00-00T02:00:00`).
//!
//! The field-assignment arm used the lvalue-only [`copy_if_value`] helper, which
//! leaves a freshly produced (non-lvalue) call result uncopied. Switching it to
//! the #1400 `copy_field_value` chokepoint boxes an inline-eligible value struct
//! regardless of whether the source is an lvalue read, matching how construction
//! / enum-payload / Optional-wrap field stores already behave. These tests
//! assert the boxing `StructNew` is emitted in the *reassigning* function.

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

fn fn_names(m: &Module) -> Vec<String> {
    m.funcs.iter().map(|f| f.name.clone()).collect()
}

/// Count `StructNew`s of struct id `sid` inside function `fname`.
fn structnew_count(m: &Module, fname: &str, sid: u32) -> usize {
    let f = m
        .funcs
        .iter()
        .find(|f| f.name == fname)
        .unwrap_or_else(|| panic!("no fn {fname}; have {:?}", fn_names(m)));
    f.blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter(|i| matches!(i, Inst::StructNew { sid: s, .. } if *s == sid))
        .count()
}

/// The issue's shape: a method reassigns an existing field from the result of
/// *another* method that returns a `value struct` by value
/// (`self.seed = self._make_seed()`). The reassigning method must box the value
/// struct — emit its OWN `StructNew` of the payload — so the field owns heap
/// storage that outlives `_make_seed`'s returned frame. Before #1446 this was 0:
/// the raw stack-spill pointer was stored and dangled, corrupting a later read.
const METHOD_REASSIGN: &str = r#"
value struct Seed:
    str tag
    int64 n

struct Cursor:
    Seed seed
    int64 index = 0

    def fresh(self) -> Seed:
        return Seed(tag="x", n=self.index)

    def step(self):
        self.seed = self.fresh()
"#;

#[test]
fn method_field_reassignment_from_returned_value_struct_is_boxed() {
    let m = lower_ok(METHOD_REASSIGN);
    let seed = sid_of(&m, "Seed");
    // The producer builds the one Seed it returns.
    assert_eq!(
        structnew_count(&m, "Cursor__fresh", seed),
        1,
        "the producer builds its own Seed"
    );
    // The fix: the reassigning method must box the returned value struct before
    // `FieldSet`, so `step` emits its OWN `StructNew Seed`. Before #1446 (field
    // arm used `copy_if_value`, lvalue-only) this was 0 — a dangling store.
    assert_eq!(
        structnew_count(&m, "Cursor__step", seed),
        1,
        "`self.field = self.method()` must box the returned value struct onto the heap (#1446)"
    );
}

/// The same hazard through a FREE function (not a method): reassigning an
/// existing field from a free-function call result returning a value struct.
const FREE_FN_REASSIGN: &str = r#"
value struct Seed:
    str tag
    int64 n

struct Holder:
    Seed slot

def make() -> Seed:
    return Seed(tag="x", n=1)

def run(Holder h):
    h.slot = make()

def main():
    auto h = Holder(Seed(tag="a", n=0))
    run(h)
"#;

#[test]
fn free_fn_field_reassignment_from_returned_value_struct_is_boxed() {
    let m = lower_ok(FREE_FN_REASSIGN);
    let seed = sid_of(&m, "Seed");
    // `make` builds its single returned Seed (not counted toward `run`).
    assert_eq!(
        structnew_count(&m, "make", seed),
        1,
        "the factory builds a Seed"
    );
    // `run` reassigns `h.slot = make()` — a non-lvalue call result — so it must
    // box the value struct itself.
    assert_eq!(
        structnew_count(&m, "run", seed),
        1,
        "`h.field = make()` must box the returned value struct onto the heap (#1446)"
    );
}

/// Value semantics are preserved and unregressed: reassigning a field from an
/// *lvalue read* of an existing value struct still copies (boxes) it, so the
/// field is an independent instance. This case already worked via `copy_if_value`
/// and must keep working via `copy_field_value`.
const LVALUE_REASSIGN: &str = r#"
value struct Seed:
    str tag
    int64 n

struct Holder:
    Seed slot

def run(Holder h, Seed other):
    h.slot = other

def main():
    auto h = Holder(Seed(tag="a", n=0))
    run(h, Seed(tag="b", n=2))
"#;

#[test]
fn lvalue_field_reassignment_still_copies_value_struct() {
    let m = lower_ok(LVALUE_REASSIGN);
    let seed = sid_of(&m, "Seed");
    // `h.slot = other` reads an existing value struct (`other`), so the field
    // store copies it — one boxing `StructNew` in `run`.
    assert_eq!(
        structnew_count(&m, "run", seed),
        1,
        "`h.field = <lvalue value struct>` copies for value semantics (preserved)"
    );
}

/// Guard against over-boxing: reassigning a *reference* struct field from a call
/// result must NOT introduce a spurious copy — a reference struct already has a
/// single heap identity and must keep it (store-by-pointer is correct here).
const REFERENCE_REASSIGN: &str = r#"
struct Ref:
    int64 n

struct Holder:
    Ref slot

def make() -> Ref:
    return Ref(n=1)

def run(Holder h):
    h.slot = make()

def main():
    auto h = Holder(Ref(n=0))
    run(h)
"#;

#[test]
fn reference_struct_field_reassignment_is_not_boxed() {
    let m = lower_ok(REFERENCE_REASSIGN);
    let refsid = sid_of(&m, "Ref");
    // `make` builds the one Ref it returns; `run` must NOT add another.
    assert_eq!(
        structnew_count(&m, "make", refsid),
        1,
        "the factory builds a Ref"
    );
    assert_eq!(
        structnew_count(&m, "run", refsid),
        0,
        "a reference-struct field reassignment keeps single heap identity (no over-boxing, #1446)"
    );
}
