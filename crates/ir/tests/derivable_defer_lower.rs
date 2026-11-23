//! Lowering of a `@derive(<library trait>)` — E3a (#1290, epic #1289 / #1248).
//!
//! A library `@derivable(Trait)` generator registers `Trait` so sema accepts
//! `@derive(Trait)` (see the sema `derivable_registry` suite). Its code
//! generation is **E3b** (#1291), so lowering must report such a derive as
//! *deferred* — not as an "unknown derivable trait", which would contradict
//! sema and mislead the author. These are host-only ZIR-lowering checks (no
//! LLVM).

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

fn lower_errors(src: &str) -> Vec<(String, String)> {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    errs.into_iter()
        .map(|e| (e.code.to_string(), e.message))
        .collect()
}

const GENERATOR: &str = "@derivable(Serialize)\ndef _derive_serialize[T]():\n    pass\n";

#[test]
fn a_library_trait_derive_is_reported_as_deferred_not_unknown() {
    let src = format!("{GENERATOR}@derive(Serialize)\nstruct User:\n    int64 id\n");
    let errs = lower_errors(&src);
    // Exactly the deferred report — never the "unknown derivable trait" error.
    assert!(
        errs.iter()
            .any(|(_, m)| m.contains("deferred to E3b") && m.contains("#1291")),
        "expected a deferred-to-E3b lowering error, got {errs:?}"
    );
    assert!(
        !errs
            .iter()
            .any(|(_, m)| m.contains("unknown derivable trait")),
        "a registered library trait must not be reported as unknown: {errs:?}"
    );
}

#[test]
fn a_builtin_only_derive_does_not_trigger_the_deferred_path() {
    // `@derive(Eq)` on a plain struct lowers without any deferred/unknown error.
    let (_m, errs) = {
        let (ast, _p, _l) = parse_str("@derive(Eq)\nstruct S:\n    int64 x\n");
        lower_module(&ast)
    };
    assert!(
        !errs.iter().any(|e| e.message.contains("deferred to E3b")),
        "built-in derive should not be deferred: {errs:?}"
    );
}

#[test]
fn an_unregistered_derive_trait_is_still_unknown_at_lowering() {
    // Defensive: with no `@derivable`, a non-built-in name is genuinely unknown
    // (sema also rejects it; lowering keeps its honest message).
    let (ast, _p, _l) = parse_str("@derive(Nope)\nstruct S:\n    int64 x\n");
    let (_m, errs) = lower_module(&ast);
    assert!(
        errs.iter()
            .any(|e| e.message.contains("unknown derivable trait")),
        "an unregistered trait is unknown at lowering: {errs:?}"
    );
}
