//! Sema acceptance of the comptime/reflect fragment — S2 (#1298, epic #1296).
//!
//! S1 (#1297) parsed `comptime for` / `comptime <expr>` but sema reported them
//! as not-yet-implemented (`ZAP0002`). S2 makes sema **accept and type** the
//! fragment so a real program compiles end-to-end: `reflect.type_info(T)` /
//! `reflect.field(...)` are comptime builtins (typed `Unknown`, their arguments
//! not inferred as values), a `comptime for` binds its loop variable and checks
//! its body, and `comptime <expr>` has its inner expression's type. The engine
//! unrolls/folds during monomorphization (host-tested separately); here we pin
//! that the front end no longer rejects the fragment.

use zapc_sema::analyze;

#[track_caller]
fn sema_errors(src: &str) -> Vec<(String, String)> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(lerrs.is_empty(), "lex: {lerrs:?}");
    assert!(perrs.is_empty(), "parse: {perrs:?}");
    let (errs, _warnings) = analyze(&module);
    errs.into_iter()
        .map(|e| (e.code.to_string(), e.message))
        .collect()
}

#[track_caller]
fn assert_accepts(src: &str) {
    let errs = sema_errors(src);
    assert!(errs.is_empty(), "expected sema to accept, got {errs:?}");
}

const DUMP: &str = "\
struct Point:
    int64 x
    int64 y
def emit(int64 v):
    pass
def dump(Point p):
    comptime for f in reflect.type_info(Point).fields:
        emit(reflect.field(p, comptime f.index))
";

#[test]
fn a_comptime_for_over_reflect_fields_type_checks() {
    assert_accepts(DUMP);
}

#[test]
fn no_more_zap0002_on_the_comptime_fragment() {
    // The S1 deferral (ZAP0002) must be gone.
    let errs = sema_errors(DUMP);
    assert!(
        !errs.iter().any(|(c, _)| c == "ZAP0002"),
        "comptime fragment should no longer be deferred: {errs:?}"
    );
}

#[test]
fn comptime_expr_binding_type_checks() {
    assert_accepts(
        "struct S:\n    int64 a\ndef g():\n    auto info = comptime reflect.type_info(S)\n",
    );
}

#[test]
fn comptime_for_over_enum_variants_type_checks() {
    assert_accepts(
        "enum Color:\n    Red\n    Green\ndef note(int64 i):\n    pass\ndef each():\n    comptime for v in reflect.type_info(Color).variants:\n        note(comptime v.index)\n",
    );
}

#[test]
fn reflect_calls_do_not_trigger_unknown_name_errors() {
    // `reflect` is not a declared value, but `reflect.type_info(...)` /
    // `reflect.field(...)` must not raise an unknown-name / unknown-call error.
    let errs = sema_errors(DUMP);
    assert!(
        !errs.iter().any(|(c, _)| c == "ZAP3001" || c == "ZAP3003"),
        "reflect.* must not be an unknown name/call: {errs:?}"
    );
}

#[test]
fn a_program_without_comptime_is_unaffected() {
    assert_accepts("struct P:\n    int64 x\ndef f(P p) -> int64:\n    return p.x\n");
}
