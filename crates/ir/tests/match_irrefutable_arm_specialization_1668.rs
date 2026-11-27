//! #1668: an arm whose pattern is statically irrefutable for the scrutinee type
//! (and carries no guard) is a catch-all, so every later arm is unreachable at
//! that specialization and must be elided before its body is lowered.
//!
//! This is the dual of the statically-dead elision added for #1326: there the
//! arm could never match; here a *preceding* arm always matches. It matters for
//! a generic that dispatches on its own type parameter — `build[Inner]` with
//! `case Command() as composed:` then a `case _:` that treats `previous` as a
//! callable. For `Inner = Command` the struct arm always matches, so the `_` arm
//! is dead; before the fix its body (`fn(..) handler = previous`) was still
//! lowered against a `Command` and failed `check_fn_coercion` with ZAP4001
//! ("value of type `Command` is not a function, but `fn(..) -> void` is
//! expected"). For `Inner = fn(..)` the struct arm is statically dead (#1326) and
//! the `_` arm is kept, so one generic body serves both specializations.
//!
//! These tests run the real lowering pass (mono + AST→ZIR) **without** sema's
//! `TypeMap`, so they guard the fix wherever `cargo test` runs (host-only, no
//! LLVM). Runtime behaviour is pinned by the golden
//! `tests/golden/generic_match_specialize_1668.zap`.

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

fn lower_raw(src: &str) -> Vec<zapc_ir::LowerError> {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    lower_module(&ast).1
}

/// Find a monomorphized function by mangled name, panicking (with the available
/// names) if absent so a changed mangling scheme fails loudly.
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

/// Whether `f` calls the (user or runtime) function `name` anywhere.
fn calls(f: &Func, name: &str) -> bool {
    f.blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .any(|i| matches!(i, Inst::Call { func, .. } if func == name))
}

/// The exact #1668 minimal reproduction: a generic that matches the composed
/// object through a nominal struct pattern, else treats the capture as a
/// callable. Driven through inference from a typed local (`handler`) and a
/// composed `Command` so it needs only #1668, not the sibling #1669.
const REPRO: &str = "\
struct Context:
    str name

struct Command:
    str name
    fn(Context, Namespace) handler

def build[Inner](Inner previous, str label) -> Command:
    match previous:
        case Command() as composed:
            return composed
        case _:
            fn(Context, Namespace) handler = previous
            return Command(name=label, handler=handler)

def prepare[Inner](str label, Inner previous) -> Command:
    return build(previous, label)

def callback(Context ctx, Namespace ns):
    assert ctx.name == \"test\"

def main():
    fn(Context, Namespace) handler = callback
    auto first = prepare(\"inner\", handler)
    auto second = prepare(\"outer\", first)
    assert second.name == \"inner\"
";

#[test]
fn repro_lowers_with_both_specializations() {
    let module = lower(REPRO);
    // The callable specialization (innermost `prepare("inner", handler)`): the
    // struct arm is statically dead, the `_` arm is kept and builds the command.
    func(&module, "build$fn$Context$Namespace$to$void");
    // The composed specialization (outer `prepare("outer", first)`): the struct
    // arm is irrefutable and the incompatible `_` arm is elided — before the fix
    // this whole module failed to lower with ZAP4001.
    func(&module, "build$Command");
}

/// A self-contained generic whose `_` arm is type-correct ONLY for a non-struct
/// specialization: `fn(int64) -> int64 f = value` cannot coerce a `Wrapped`.
const INCOMPAT_WILDCARD: &str = "\
struct Wrapped:
    int64 n

def pick[T](T value) -> int64:
    match value:
        case Wrapped() as w:
            return w.n
        case _:
            fn(int64) -> int64 f = value
            return f(0)

def ident(int64 x) -> int64:
    return x

def main():
    print(pick[Wrapped](Wrapped(n=5)))
    fn(int64) -> int64 g = ident
    print(pick[fn(int64) -> int64](g))
";

#[test]
fn irrefutable_struct_arm_elides_type_incompatible_wildcard() {
    // Lowers cleanly ONLY because the `_` arm is never lowered for the `Wrapped`
    // specialization (its `fn f = value` would otherwise be ZAP4001), while the
    // callable specialization keeps and runs it.
    let module = lower(INCOMPAT_WILDCARD);
    func(&module, "pick$Wrapped");
    func(&module, "pick$fn$int64$to$int64");
}

#[test]
fn unfixed_shape_would_report_the_fn_coercion_error_if_lowered() {
    // Guard the exact diagnostic the fix prevents: force the `_` body to be
    // lowered for `Wrapped` by making the struct arm refutable (a field literal
    // the elision cannot prove), and confirm the ZAP4001 fires. This proves the
    // test above passes because of the elision, not because the body is somehow
    // type-valid for `Wrapped`.
    let errs = lower_raw(
        "struct Wrapped:\n\
        \x20   int64 n\n\
        def pick[T](T value) -> int64:\n\
        \x20   match value:\n\
        \x20       case Wrapped(n=0) as w:\n\
        \x20           return 1\n\
        \x20       case _:\n\
        \x20           fn(int64) -> int64 f = value\n\
        \x20           return f(0)\n\
        def main():\n\
        \x20   print(pick[Wrapped](Wrapped(n=5)))\n",
    );
    assert!(
        errs.iter().any(|e| e.message.contains("is not a function")),
        "expected the ZAP4001 fn-coercion error, got: {errs:?}"
    );
}

/// Observable elision: a named helper called only from the `_` arm is absent
/// from the struct specialization and present in a specialization that keeps it.
const HELPER_PROBE: &str = "\
struct Wrapped:
    int64 n

def helper() -> int64:
    return 7

def pick[T](T value) -> int64:
    match value:
        case Wrapped() as w:
            return w.n
        case _:
            return helper()

def main():
    print(pick[Wrapped](Wrapped(n=5)))
    print(pick[int64](3))
";

#[test]
fn wildcard_body_is_dropped_for_struct_and_kept_for_scalar() {
    let module = lower(HELPER_PROBE);
    assert!(
        !calls(func(&module, "pick$Wrapped"), "helper"),
        "pick$Wrapped must elide the unreachable `_` arm (no call to helper)",
    );
    assert!(
        calls(func(&module, "pick$int64"), "helper"),
        "pick$int64 must keep the `_` arm — the struct arm is statically dead \
         there, not irrefutable (the #1326 path must stay intact)",
    );
}

#[test]
fn value_struct_irrefutable_arm_elides_following() {
    // Generality: a `value struct` catch-all elides the following arm too.
    let module = lower(
        "value struct Point:\n\
        \x20   int64 x\n\
        def helper() -> int64:\n\
        \x20   return 9\n\
        def pick[T](T value) -> int64:\n\
        \x20   match value:\n\
        \x20       case Point() as p:\n\
        \x20           return p.x\n\
        \x20       case _:\n\
        \x20           return helper()\n\
        def main():\n\
        \x20   print(pick[Point](Point(x=1)))\n",
    );
    assert!(
        !calls(func(&module, "pick$Point"), "helper"),
        "a value-struct catch-all must elide the following arm",
    );
}

#[test]
fn guarded_irrefutable_arm_keeps_following_arm() {
    // A guard makes an otherwise-irrefutable arm refutable: it may fall through,
    // so the following arm stays live and its helper call must survive.
    let module = lower(
        "struct Wrapped:\n\
        \x20   int64 n\n\
        def helper() -> int64:\n\
        \x20   return 7\n\
        def pick[T](T value, bool flag) -> int64:\n\
        \x20   match value:\n\
        \x20       case Wrapped() as w if flag:\n\
        \x20           return w.n\n\
        \x20       case _:\n\
        \x20           return helper()\n\
        def main():\n\
        \x20   print(pick[Wrapped](Wrapped(n=5), True))\n",
    );
    assert!(
        calls(func(&module, "pick$Wrapped"), "helper"),
        "a guarded catch-all must NOT suppress the following arm",
    );
}

#[test]
fn field_binding_struct_arm_still_elides() {
    // A struct pattern whose field sub-patterns are themselves irrefutable
    // (a bare capture) is still a catch-all.
    let module = lower(
        "struct Wrapped:\n\
        \x20   int64 n\n\
        def helper() -> int64:\n\
        \x20   return 7\n\
        def pick[T](T value) -> int64:\n\
        \x20   match value:\n\
        \x20       case Wrapped(n=k):\n\
        \x20           return k\n\
        \x20       case _:\n\
        \x20           return helper()\n\
        def main():\n\
        \x20   print(pick[Wrapped](Wrapped(n=5)))\n",
    );
    assert!(
        !calls(func(&module, "pick$Wrapped"), "helper"),
        "a struct arm binding a field by capture is irrefutable and elides `_`",
    );
}

#[test]
fn literal_field_struct_arm_keeps_following_arm() {
    // A struct pattern with a refutable field test (`n=0`) is NOT a catch-all,
    // so the following arm stays live.
    let module = lower(
        "struct Wrapped:\n\
        \x20   int64 n\n\
        def helper() -> int64:\n\
        \x20   return 7\n\
        def pick[T](T value) -> int64:\n\
        \x20   match value:\n\
        \x20       case Wrapped(n=0):\n\
        \x20           return 1\n\
        \x20       case _:\n\
        \x20           return helper()\n\
        def main():\n\
        \x20   print(pick[Wrapped](Wrapped(n=5)))\n",
    );
    assert!(
        calls(func(&module, "pick$Wrapped"), "helper"),
        "a struct arm with a refutable field literal must keep the `_` arm",
    );
}

#[test]
fn enum_variant_arm_is_not_irrefutable() {
    // A bare enum-member pattern tests a discriminant — a sum type has other
    // members — so it is refutable and the following arm is kept.
    let module = lower(
        "enum Color:\n\
        \x20   RED\n\
        \x20   GREEN\n\
        def helper() -> int64:\n\
        \x20   return 7\n\
        def pick[T](T value) -> int64:\n\
        \x20   match value:\n\
        \x20       case Color.RED:\n\
        \x20           return 1\n\
        \x20       case _:\n\
        \x20           return helper()\n\
        def main():\n\
        \x20   print(pick[Color](Color.RED))\n",
    );
    assert!(
        calls(func(&module, "pick$Color"), "helper"),
        "an enum-variant arm must not be treated as irrefutable",
    );
}
