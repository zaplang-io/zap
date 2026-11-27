//! #1675: when a generic `match`es a value of an enum type parameter and the
//! unguarded variant arms are exhaustive, a trailing `case _:` fallback is
//! unreachable at that specialization and must be elided before its body is
//! lowered — the enum analog of #1668's single-irrefutable-arm elision.
//!
//! Before the fix, lowering emitted the dead `_` body, so a fallback that was
//! type-correct only for a different specialization (e.g. treating the value as
//! a callable) failed with ZAP4001 for the enum specialization even though the
//! arm can never run.
//!
//! Host-only (no LLVM): these lower the module through mono + AST→ZIR and either
//! assert clean lowering or inspect the emitted calls. Runtime behaviour is
//! pinned by the golden `tests/golden/generic_match_enum_exhaustive_1675.zap`.

use zapc_ir::{lower_module, Func, Inst, Module};
use zapc_syntax::parser::parse_str;

fn lower(src: &str) -> Module {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "expected clean lowering, got: {errs:?}");
    module
}

fn lower_errs(src: &str) -> Vec<zapc_ir::LowerError> {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    lower_module(&ast).1
}

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

fn calls(f: &Func, name: &str) -> bool {
    f.blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .any(|i| matches!(i, Inst::Call { func, .. } if func == name))
}

// --- The exact #1675 reproduction: an exhaustive tier-1 enum match with a
//     type-incompatible `_` arm. Lowers cleanly ONLY because the `_` is elided.

const EXHAUSTIVE: &str = "\
enum Shape:
    Circle
    Square

def pick[T](T value) -> int64:
    match value:
        case Shape.Circle:
            return 1
        case Shape.Square:
            return 2
        case _:
            fn(int64) -> int64 f = value
            return f(0)

def main():
    print(pick[Shape](Shape.Circle))
";

#[test]
fn exhaustive_enum_match_elides_incompatible_wildcard() {
    let module = lower(EXHAUSTIVE);
    func(&module, "pick$Shape");
}

#[test]
fn non_exhaustive_enum_match_keeps_the_wildcard_and_reports_zap4001() {
    // A third variant is uncovered, so the `_` arm is reachable and its
    // type-incompatible body is lowered — the ZAP4001 proves the elision above
    // fires on coverage, not unconditionally.
    let errs = lower_errs(
        "enum Shape:\n\
        \x20   Circle\n\
        \x20   Square\n\
        \x20   Triangle\n\
        def pick[T](T value) -> int64:\n\
        \x20   match value:\n\
        \x20       case Shape.Circle:\n\
        \x20           return 1\n\
        \x20       case Shape.Square:\n\
        \x20           return 2\n\
        \x20       case _:\n\
        \x20           fn(int64) -> int64 f = value\n\
        \x20           return f(0)\n\
        def main():\n\
        \x20   print(pick[Shape](Shape.Circle))\n",
    );
    assert!(
        errs.iter().any(|e| e.message.contains("is not a function")),
        "expected ZAP4001 for the still-reachable `_` arm, got: {errs:?}"
    );
}

// --- Observable elision via a helper called only from the dead `_` arm.

const HELPER: &str = "\
enum Shape:
    Circle
    Square

def helper() -> int64:
    return 7

def pick[T](T value) -> int64:
    match value:
        case Shape.Circle:
            return 1
        case Shape.Square:
            return 2
        case _:
            return helper()

def main():
    print(pick[Shape](Shape.Circle))
";

#[test]
fn exhaustive_match_drops_the_wildcard_body() {
    let module = lower(HELPER);
    assert!(
        !calls(func(&module, "pick$Shape"), "helper"),
        "an exhaustive variant set must elide the trailing `_` arm (no helper call)",
    );
}

#[test]
fn or_pattern_completing_coverage_triggers_exhaustion() {
    // `Circle | Square` in one arm covers both remaining variants.
    let module = lower(
        "enum Shape:\n\
        \x20   Circle\n\
        \x20   Square\n\
        def helper() -> int64:\n\
        \x20   return 7\n\
        def pick[T](T value) -> int64:\n\
        \x20   match value:\n\
        \x20       case Shape.Circle | Shape.Square:\n\
        \x20           return 1\n\
        \x20       case _:\n\
        \x20           return helper()\n\
        def main():\n\
        \x20   print(pick[Shape](Shape.Circle))\n",
    );
    assert!(
        !calls(func(&module, "pick$Shape"), "helper"),
        "an or-pattern covering every variant must elide the `_` arm",
    );
}

#[test]
fn guarded_variant_arm_does_not_complete_coverage() {
    // A guard may fall through, so the guarded arm does not count toward
    // coverage and the `_` arm stays live.
    let module = lower(
        "enum Shape:\n\
        \x20   Circle\n\
        \x20   Square\n\
        def helper() -> int64:\n\
        \x20   return 7\n\
        def pick[T](T value, bool flag) -> int64:\n\
        \x20   match value:\n\
        \x20       case Shape.Circle:\n\
        \x20           return 1\n\
        \x20       case Shape.Square if flag:\n\
        \x20           return 2\n\
        \x20       case _:\n\
        \x20           return helper()\n\
        def main():\n\
        \x20   print(pick[Shape](Shape.Circle, True))\n",
    );
    assert!(
        calls(func(&module, "pick$Shape"), "helper"),
        "a guarded variant arm must not count toward exhaustiveness",
    );
}

#[test]
fn tier3_enum_with_bare_binder_payloads_is_exhausted() {
    // A tier-3 (payload-carrying) enum: bare-binder payload patterns match every
    // value of their variant, so covering both variants elides the `_` arm.
    let module = lower(
        "enum Tree:\n\
        \x20   Leaf(int64 value)\n\
        \x20   Node(int64 left)\n\
        def helper() -> int64:\n\
        \x20   return 7\n\
        def pick[T](T value) -> int64:\n\
        \x20   match value:\n\
        \x20       case Tree.Leaf(v):\n\
        \x20           return v\n\
        \x20       case Tree.Node(l):\n\
        \x20           return l\n\
        \x20       case _:\n\
        \x20           return helper()\n\
        def main():\n\
        \x20   print(pick[Tree](Tree.Leaf(5)))\n",
    );
    assert!(
        !calls(func(&module, "pick$Tree"), "helper"),
        "bare-binder payloads cover their variants, so the `_` arm is elided",
    );
}

#[test]
fn refutable_payload_does_not_complete_coverage() {
    // A literal payload sub-pattern (`Leaf(0)`) is refutable, so that arm does
    // not fully cover `Leaf`; the `_` arm stays live.
    let module = lower(
        "enum Tree:\n\
        \x20   Leaf(int64 value)\n\
        \x20   Node(int64 left)\n\
        def helper() -> int64:\n\
        \x20   return 7\n\
        def pick[T](T value) -> int64:\n\
        \x20   match value:\n\
        \x20       case Tree.Leaf(0):\n\
        \x20           return 1\n\
        \x20       case Tree.Node(l):\n\
        \x20           return l\n\
        \x20       case _:\n\
        \x20           return helper()\n\
        def main():\n\
        \x20   print(pick[Tree](Tree.Leaf(5)))\n",
    );
    assert!(
        calls(func(&module, "pick$Tree"), "helper"),
        "a refutable payload must not count as covering its variant",
    );
}
