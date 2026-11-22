//! Type-contextual bare-payload `Optional[T]` wrapping (#1262).
//!
//! A bare payload value in a position whose expected type is `Optional[T]` —
//! e.g. the scalar keyword `Delta(day=1)` where `day: Optional[int64]` — used to
//! be stored UNBOXED: lowering emitted `iconst 1` straight into the Optional
//! field (`fieldset`, no `Some` allocation or discriminant tag), so a later
//! `.unwrap()` dereferenced the integer as an Optional object and SIGSEGV'd at
//! `0x1` — a memory-safety violation in a *checked* program. This is the exact
//! symmetric partner of the bare-`None` gap fixed by #1075/#1080: the same
//! `expr_coerced` / `expr_collection_context` chokepoint handled `None` but had
//! no `T -> Some(T)` wrap for the non-`None` payload half.
//!
//! Scope (see `decisions/lang/1262-optional-scalar-coercion.md`): sema accepts a
//! bare payload only in constructor fields, field assignment, index assignment,
//! `.append`, and dict-value assignment — exactly the positions that crash today
//! — so those are what the wrap fixes and what is exercised here. A bare payload
//! in a `return` / `Optional[T]` parameter / typed-binding / list-literal
//! position is still a clean sema type error (a separate implicit-widening
//! design question), so it is not asserted.
//!
//! These host-only lowering checks (no LLVM) pin that:
//!   * each sema-accepted position lowers a bare payload without error,
//!   * the emitted ZIR for the scalar-keyword repro really allocates and tags a
//!     `Some` (a `structnew` of the Optional) rather than storing a bare
//!     `iconst`, and matches what an explicit `Optional.Some(...)` emits, and
//!   * an already-`Optional[T]` source and a bare `None` are both unaffected.
//!
//! The end-to-end runtime *values* (`is_some`/`unwrap`/`unwrap_or`, cross-module
//! construction) are covered by the `optional_scalar_wrap` golden and the
//! `optional_scalar_wrap_module` golden project on CI, since the
//! scalar-vs-pointer storage bug only crashes once the native collector runs.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting no lex/parse/lower errors. Returns the ZIR
/// module text (`Display`) for structural assertions.
fn lower_ok(src: &str) -> String {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    format!("{module}")
}

/// Parse + lower `src`, asserting lowering fails with a message containing
/// `needle` (parse/lex must be clean so the failure is the lowering one we
/// mean to pin).
fn lower_err_contains(src: &str, needle: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(!errs.is_empty(), "expected a lowering error, got none");
    let joined = errs
        .iter()
        .map(|e| format!("{e:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains(needle),
        "expected a lowering error containing {needle:?}, got:\n{joined}"
    );
}

fn count(hay: &str, needle: &str) -> usize {
    hay.matches(needle).count()
}

// ----- ZIR structure: the Some allocation really exists (the crux) -----

#[test]
fn scalar_keyword_emits_some_allocation_not_bare_iconst() {
    // The issue's exact `value struct` repro. `Delta` is a *value* struct, so the
    // constructor itself does not `structnew` — meaning the ONLY `structnew` in
    // the function is the auto-wrapped `Some(1)` allocation. Before the fix the
    // payload was stored as a bare `iconst` and there was no `structnew` at all;
    // reading it then dereferenced the integer as an Optional object and crashed.
    let zir = lower_ok(
        "value struct Delta:\n\
        \x20   Optional[int64] day = None\n\
        def main():\n\
        \x20   auto d = Delta(day=1)\n\
        \x20   print(d.day.unwrap())\n",
    );
    assert!(
        zir.contains("structnew"),
        "expected a Some allocation (structnew) for the wrapped payload, got:\n{zir}"
    );
    assert!(
        zir.contains("iconst 1"),
        "expected the payload literal 1 to flow into the Some, got:\n{zir}"
    );
}

#[test]
fn scalar_keyword_wrap_matches_explicit_some() {
    // The auto-wrap must emit the same construction an explicit
    // `Optional.Some(1)` does: the symmetric-partner rationale (reuse
    // `gen_opt_some`, no fresh untracked alloc). The ZIR of the two forms carries
    // the same number of `structnew`s and the same tag discipline.
    let auto = lower_ok(
        "value struct Delta:\n\
        \x20   Optional[int64] day = None\n\
        def main():\n\
        \x20   auto d = Delta(day=1)\n\
        \x20   print(d.day.unwrap())\n",
    );
    let explicit = lower_ok(
        "value struct Delta:\n\
        \x20   Optional[int64] day = None\n\
        def main():\n\
        \x20   auto d = Delta(day=Optional.Some(1))\n\
        \x20   print(d.day.unwrap())\n",
    );
    assert_eq!(
        count(&auto, "structnew"),
        count(&explicit, "structnew"),
        "auto-wrapped Some must allocate exactly like an explicit Optional.Some\n\
         auto:\n{auto}\nexplicit:\n{explicit}"
    );
    assert!(count(&auto, "structnew") >= 1);
}

#[test]
fn omitted_default_none_has_no_payload_alloc_difference() {
    // Control: a bare-`None` default (#1075 path) still lowers — the fix leaves
    // it untouched. The `None` and the `Some(1)` forms differ only by the payload
    // `fieldset`, both going through a tagged `structnew`.
    let none = lower_ok(
        "value struct Delta:\n\
        \x20   Optional[int64] day = None\n\
        def main():\n\
        \x20   auto d = Delta()\n\
        \x20   print(d.day.is_none())\n",
    );
    assert!(
        none.contains("structnew"),
        "tagged None still allocates:\n{none}"
    );
}

// ----- Positions: constructor (positional / keyword / omitted / None / Some) --

#[test]
fn ctor_positional_scalar_payload() {
    lower_ok(
        "value struct Delta:\n\
        \x20   Optional[int64] day = None\n\
        def main():\n\
        \x20   auto d = Delta(1)\n\
        \x20   print(d.day.unwrap())\n",
    );
}

#[test]
fn ctor_keyword_scalar_payload_zero_and_nonzero() {
    // Zero is the adversarial scalar: a raw `0` is indistinguishable from a null
    // Optional pointer, so the tag is what makes `is_some()` correct.
    lower_ok(
        "value struct Delta:\n\
        \x20   Optional[int64] day = None\n\
        \x20   Optional[int64] hour = None\n\
        def main():\n\
        \x20   auto d = Delta(day=0, hour=5)\n\
        \x20   print(d.day.is_some(), d.hour.is_some())\n",
    );
}

#[test]
fn ctor_omitted_keeps_default_none() {
    lower_ok(
        "value struct Delta:\n\
        \x20   Optional[int64] day = None\n\
        def main():\n\
        \x20   auto d = Delta()\n\
        \x20   print(d.day.is_none())\n",
    );
}

#[test]
fn ctor_explicit_none_and_some_unaffected() {
    lower_ok(
        "value struct Delta:\n\
        \x20   Optional[int64] day = None\n\
        def main():\n\
        \x20   auto a = Delta(day=None)\n\
        \x20   auto b = Delta(day=Optional.Some(7))\n\
        \x20   print(a.day.is_none(), b.day.unwrap())\n",
    );
}

#[test]
fn ctor_reference_payload_str() {
    // A reference payload (`Optional[str]`) has a different physical
    // representation from a scalar; the bare `"x"` must wrap too.
    lower_ok(
        "value struct Node:\n\
        \x20   Optional[str] name = None\n\
        def main():\n\
        \x20   auto n = Node(name=\"hello\")\n\
        \x20   print(n.name.unwrap())\n",
    );
}

#[test]
fn ctor_reference_payload_struct() {
    lower_ok(
        "struct Item:\n\
        \x20   int64 value\n\
        value struct Box:\n\
        \x20   Optional[Item] slot = None\n\
        def main():\n\
        \x20   auto b = Box(slot=Item(42))\n\
        \x20   print(b.slot.unwrap().value)\n",
    );
}

#[test]
fn ctor_already_optional_source_not_double_wrapped() {
    // Passing an existing `Optional[int64]` binding (or an `Optional.Some`) for an
    // `Optional[int64]` field must NOT wrap it again into `Some(Some(..))`.
    lower_ok(
        "value struct Delta:\n\
        \x20   Optional[int64] day = None\n\
        def main():\n\
        \x20   Optional[int64] existing = Optional.Some(3)\n\
        \x20   auto d = Delta(day=existing)\n\
        \x20   print(d.day.unwrap())\n",
    );
}

// ----- Positions: field assignment, collection element -----
//
// Only positions sema ACCEPTS a bare payload in are exercised end-to-end: the
// constructor fields above, field assignment, index assignment, `.append`, and
// dict-value assignment. A bare payload in a `return` / `Optional[T]` parameter
// / typed-binding / list-literal position is a clean sema type error today (a
// separate implicit-widening design question, filed as a follow-up and recorded
// in the #1262 ADR), so those are not asserted here.

#[test]
fn field_assignment_scalar_payload() {
    lower_ok(
        "struct Box:\n\
        \x20   Optional[int64] day = None\n\
        def main():\n\
        \x20   auto b = Box()\n\
        \x20   b.day = 9\n\
        \x20   print(b.day.unwrap())\n",
    );
}

#[test]
fn field_assignment_reference_payload() {
    lower_ok(
        "struct Item:\n\
        \x20   int64 value\n\
        struct Box:\n\
        \x20   Optional[Item] slot = None\n\
        def main():\n\
        \x20   auto b = Box()\n\
        \x20   b.slot = Item(3)\n\
        \x20   print(b.slot.unwrap().value)\n",
    );
}

#[test]
fn list_index_assignment_scalar_payload() {
    lower_ok(
        "def main():\n\
        \x20   list[Optional[int64]] xs = [None]\n\
        \x20   xs[0] = 5\n\
        \x20   print(xs[0].unwrap())\n",
    );
}

#[test]
fn list_append_scalar_payload() {
    lower_ok(
        "def main():\n\
        \x20   list[Optional[int64]] xs = [None]\n\
        \x20   xs.append(6)\n\
        \x20   xs.append(Optional.Some(7))\n\
        \x20   print(xs[1].unwrap(), xs[2].unwrap())\n",
    );
}

#[test]
fn dict_value_payload() {
    lower_ok(
        "def main():\n\
        \x20   dict[str, Optional[int64]] m = {}\n\
        \x20   m[\"a\"] = 1\n\
        \x20   print(m[\"a\"].unwrap())\n",
    );
}

// ----- Negative / unaffected: bare None still rejected for a scalar dest ------

#[test]
fn bare_none_still_rejected_for_non_optional_field() {
    // The #1075 bare-`None` negative path must be unchanged by the payload wrap.
    lower_err_contains(
        "struct Box:\n\
        \x20   int64 item\n\
        def main():\n\
        \x20   auto box = Box(None)\n\
        \x20   print(box.item)\n",
        "None is not a valid",
    );
}
