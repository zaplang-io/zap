//! Dead nominal-struct match arms in a scalar specialization (zaplang-io/zap#1326).
//!
//! A generic function that destructures a nominal `value struct` through an
//! `as` binding in a `match` arm and then reads the binding's fields used to
//! panic native codegen when instantiated with a scalar: the `Date()` arm is
//! provably false at that specialization, yet lowering still emitted its body,
//! leaving a `FieldGet` that reads struct fields off an `i64`. The backend then
//! tried to GEP into a scalar and inkwell panicked (`Found IntValue ... but
//! expected PointerValue variant`).
//!
//! The fix elides a statically-impossible arm before it is lowered, so the dead
//! body — and its invalid field reads — never reach the IR. These are host-only
//! checks (no LLVM): they lower the module through mono + AST→ZIR and inspect
//! the emitted instructions directly. The end-to-end runtime behaviour (the
//! successful `Date` case and the scalar fallback both printing the right
//! thing) is pinned by the goldens `tests/golden/match_scalar_specialization.*`
//! and the installed-package project `tests/golden_projects/
//! match_scalar_specialization_pkg/`.

use zapc_ir::{lower_module, Func, Inst, Module};
use zapc_syntax::parser::parse_str;

/// A generic that matches a prelude `Date` through an `as` binding and feeds the
/// binding to a field-reading builtin, instantiated with both a real `Date` and
/// several scalars. `combine`/`date`/`time` come from the always-available
/// `datetime` prelude module; `Date`/`DateTime` are prelude value-structs.
///
/// The specializations are requested with explicit type arguments
/// (`promote[int64](...)`) so this stays a pure lowering check driven by the
/// host-safe `lower_module` — it needs no sema `TypeMap` to pick each `T`. The
/// runtime behaviour across inferred specializations is covered by the goldens.
const SRC: &str = "\
def promote[T](T value) -> DateTime:
    match value:
        case Date() as day:
            return datetime.combine(day, datetime.time(0, 0, 0))
        case _:
            raise TypeError(\"expected Date\")

def main():
    auto d = datetime.date(2026, 11, 23)
    auto dt = promote[Date](d)
    print(dt.year)
    try:
        auto a = promote[int64](1)
        auto b = promote[float64](3.5)
        auto c = promote[str](\"x\")
        print(a.year + b.year + c.year)
    except TypeError:
        print(\"rejected\")
";

/// Lower `SRC` and assert the whole frontend (mono included) accepts it.
fn lower_src() -> Module {
    let (ast, perrs, lerrs) = parse_str(SRC);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    module
}

/// Find a monomorphized function by its mangled name (`<base>$<suffix>`), e.g.
/// `promote$int64`. Panics (with the available names) if it is absent, so a
/// renamed mangling scheme fails loudly rather than silently skipping a check.
fn func<'a>(module: &'a Module, mangled: &str) -> &'a Func {
    module
        .funcs
        .iter()
        .find(|f| f.name == mangled)
        .unwrap_or_else(|| {
            let names: Vec<&str> = module.funcs.iter().map(|f| f.name.as_str()).collect();
            panic!("no function {mangled:?}; have: {names:?}");
        })
}

/// Count `FieldGet` instructions across every block of `f`.
fn field_gets(f: &Func) -> usize {
    f.blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter(|i| matches!(i, Inst::FieldGet { .. }))
        .count()
}

#[test]
fn scalar_specializations_elide_the_impossible_date_arm() {
    let module = lower_src();
    // The three scalar specializations are instantiated (so the dead arm is
    // reachable in codegen) and each must carry zero field reads: the only
    // live arm is the `_` fallback, which reads nothing.
    for mangled in ["promote$int64", "promote$float64", "promote$str"] {
        let f = func(&module, mangled);
        assert_eq!(
            field_gets(f),
            0,
            "{mangled} must not read any struct fields (the impossible `Date()` \
             arm must be elided), IR:\n{}",
            render(f),
        );
    }
}

#[test]
fn date_specialization_keeps_the_arm_and_its_binding() {
    let module = lower_src();
    // The genuine specialization must preserve the nominal binding and its field
    // reads: `combine(day, time(...))` reads Date's three fields plus Time's
    // three, so the arm is unmistakably alive.
    let f = func(&module, "promote$Date");
    assert!(
        field_gets(f) >= 6,
        "promote$Date must keep the Date arm's field reads, IR:\n{}",
        render(f),
    );
}

#[test]
fn no_field_read_targets_a_scalar_value() {
    // Belt-and-braces guard on the exact panic shape: in every function, a
    // `FieldGet`'s base must be a value of a struct/ptr type, never a scalar.
    // This is what inkwell choked on (`i64` where a pointer was expected).
    let module = lower_module(&parse_str(SRC).0).0;
    for f in &module.funcs {
        for block in &f.blocks {
            for inst in &block.insts {
                if let Inst::FieldGet { base, .. } = inst {
                    // A struct/heap receiver lowers to a pointer; a scalar does
                    // not (`Ty::is_ptr`). A `FieldGet` whose base is a scalar is
                    // exactly the broken shape that crashed the backend.
                    let base_ty = f.value_ty(*base);
                    assert!(
                        base_ty.is_ptr(),
                        "{}: FieldGet reads fields off a non-pointer value ({base_ty:?})",
                        f.name,
                    );
                }
            }
        }
    }
}

// ----- the elision is the general mechanism, not a struct-only special case -----

/// A sequence pattern can only match a list; against a scalar it is dead.
const SEQ_SRC: &str = "\
def take[T](T value) -> int64:
    match value:
        case [a, b]:
            return a + b
        case _:
            return 0

def main():
    auto xs = [10, 20]
    print(take[list[int64]](xs))
    print(take[int64](5))
";

/// An or-pattern of two nominal structs is dead only when *every* alternative
/// is dead, so it survives for a type that matches one of them.
const OR_SRC: &str = "\
def kind[T](T value) -> int64:
    match value:
        case Date() | Time():
            return 1
        case _:
            return 0

def main():
    auto d = datetime.date(2026, 11, 23)
    print(kind[Date](d))
    print(kind[int64](5))
";

/// Lower arbitrary `src`, asserting the frontend accepts it, and return the
/// module.
fn lower(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    module
}

/// Whether `f` calls the runtime function `name` anywhere.
fn calls(f: &Func, name: &str) -> bool {
    f.blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .any(|i| matches!(i, Inst::Call { func, .. } if func == name))
}

#[test]
fn sequence_arm_is_elided_for_a_scalar_but_kept_for_a_list() {
    let module = lower(SEQ_SRC);
    // The list specialization runs the sequence arm (length probe present)…
    assert!(
        calls(func(&module, "take$list$int64"), "zap_list_len"),
        "take$list$int64 must keep the sequence arm",
    );
    // …while the scalar specialization drops it entirely (no list ops leak onto
    // a scalar scrutinee).
    assert!(
        !calls(func(&module, "take$int64"), "zap_list_len"),
        "take$int64 must elide the impossible sequence arm",
    );
}

#[test]
fn or_pattern_arm_survives_when_one_alternative_matches() {
    let module = lower(OR_SRC);
    // `Date() | Time()` is alive for a Date (one alternative matches) and dead
    // for a scalar (both impossible). A kept or-pattern expands into a chain of
    // alternative tests, so the Date specialization has strictly more blocks
    // than the scalar one, where the whole arm — tests and all — is dropped.
    let date_blocks = func(&module, "kind$Date").blocks.len();
    let scalar_blocks = func(&module, "kind$int64").blocks.len();
    assert!(
        date_blocks > scalar_blocks,
        "kind$Date ({date_blocks} blocks) must keep the or-pattern arm that \
         kind$int64 ({scalar_blocks} blocks) elides",
    );
    assert_eq!(
        field_gets(func(&module, "kind$int64")),
        0,
        "kind$int64 must emit no field reads (the or-pattern arm is elided)",
    );
}

/// Render a function's instructions for a failure message.
fn render(f: &Func) -> String {
    let mut out = format!("func {}:\n", f.name);
    for (bi, b) in f.blocks.iter().enumerate() {
        out.push_str(&format!("  block{bi}:\n"));
        for inst in &b.insts {
            out.push_str(&format!("    {inst:?}\n"));
        }
    }
    out
}
