//! `@derivable(Trait)` registration model — E3a (#1290, epic #1289 / #1248).
//!
//! The derivable-trait set is no longer a closed list: `@derive(X)` resolves
//! against a **registry** seeded with the compiler built-ins (D-0036) and
//! extended, order-independently, by a library `@derivable(Trait)` on a generic
//! generator function. E3a lands the model + resolution + diagnostics only — no
//! forwarding-impl synthesis and no codegen (that is E3b, #1291). These tests
//! run host-side through `zapc_sema::analyze` / `analyze_reflect`.
//!
//! Intent documentation (a walled garden, not a minimum): every built-in still
//! resolves, a registered library trait resolves in any declaration order, and
//! every malformed/ambiguous registration has a precise diagnostic.

use zapc_sema::{analyze, analyze_reflect};

/// Parse + analyze, returning the sema errors (lex/parse must be clean).
#[track_caller]
fn errors(src: &str) -> Vec<(String, String)> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    let (errs, _warnings) = analyze(&module);
    errs.into_iter()
        .map(|e| (e.code.to_string(), e.message))
        .collect()
}

#[track_caller]
fn assert_clean(src: &str) {
    let errs = errors(src);
    assert!(errs.is_empty(), "expected no sema errors, got {errs:?}");
}

/// Does any error carry this `ZAPnnnn` code?
fn has_code(errs: &[(String, String)], code: &str) -> bool {
    errs.iter().any(|(c, _)| c == code)
}

// A minimal, sema-clean generic generator carrying `@derivable(<trait>)`.
fn generator(trait_name: &str, fn_name: &str) -> String {
    format!("@derivable({trait_name})\ndef {fn_name}[T]():\n    pass\n")
}

// ---------------------------------------------------------------------------
// built-ins keep working (back-compat, D-0036)
// ---------------------------------------------------------------------------

#[test]
fn every_builtin_derivable_still_resolves() {
    for t in ["Eq", "Ord", "Json", "Repr", "Hash"] {
        assert_clean(&format!("@derive({t})\nstruct S:\n    int64 x\n"));
    }
}

#[test]
fn a_combined_builtin_derive_still_resolves() {
    assert_clean("@derive(Eq, Hash, Repr)\nstruct Point:\n    int64 x\n    int64 y\n");
}

// ---------------------------------------------------------------------------
// library registration + resolution
// ---------------------------------------------------------------------------

#[test]
fn a_registered_library_trait_resolves_in_a_derive() {
    let src = format!(
        "{}@derive(Serialize)\nstruct User:\n    int64 id\n",
        generator("Serialize", "_derive_serialize")
    );
    assert_clean(&src);
}

#[test]
fn registration_is_order_independent() {
    // `@derive(Foo)` appears BEFORE the `@derivable(Foo)` that registers it.
    let src = format!(
        "@derive(Foo)\nstruct S:\n    int64 x\n{}",
        generator("Foo", "_derive_foo")
    );
    assert_clean(&src);
}

#[test]
fn the_reflector_lists_library_registrations_with_their_generator_and_span() {
    let src = format!(
        "{}struct User:\n    int64 id\n",
        generator("Serialize", "_derive_serialize")
    );
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(&src);
    assert!(lerrs.is_empty() && perrs.is_empty());
    let (errs, _w, reflector) = analyze_reflect(&module);
    assert!(errs.is_empty(), "{errs:?}");

    let regs = reflector.library_derivables();
    assert_eq!(regs.len(), 1, "exactly one library derivable, got {regs:?}");
    let (trait_name, generator_fn, span) = regs[0];
    assert_eq!(trait_name, "Serialize");
    assert_eq!(generator_fn, "_derive_serialize");
    // The span points at the `@derivable(Serialize)` application in the source.
    let slice = &src[span.start as usize..span.end as usize];
    assert!(
        slice.contains("derivable") || slice.contains("Serialize"),
        "span should cover the @derivable application, got {slice:?}"
    );
    // A built-in derivable is NOT listed (it has no library generator).
    let with_eq = format!("@derive(Eq)\nstruct E:\n    int64 x\n{src}");
    let (m2, _p, _l) = zapc_syntax::parser::parse_str(&with_eq);
    let (_e2, _w2, r2) = analyze_reflect(&m2);
    assert!(r2.library_derivables().iter().all(|(t, _, _)| *t != "Eq"));
}

// ---------------------------------------------------------------------------
// unknown derive trait
// ---------------------------------------------------------------------------

#[test]
fn an_unregistered_derive_trait_is_unknown() {
    let errs = errors("@derive(Nope)\nstruct S:\n    int64 x\n");
    assert!(has_code(&errs, "ZAP5035"), "{errs:?}");
}

#[test]
fn a_library_trait_is_only_known_where_registered() {
    // Without the `@derivable`, `@derive(Serialize)` is unknown.
    let errs = errors("@derive(Serialize)\nstruct S:\n    int64 x\n");
    assert!(has_code(&errs, "ZAP5035"), "{errs:?}");
}

// ---------------------------------------------------------------------------
// malformed @derivable
// ---------------------------------------------------------------------------

#[test]
fn derivable_with_no_trait_name_is_rejected() {
    let errs = errors("@derivable()\ndef g[T]():\n    pass\n");
    assert!(has_code(&errs, "ZAP5057"), "{errs:?}");
}

#[test]
fn derivable_with_multiple_names_is_rejected() {
    let errs = errors("@derivable(A, B)\ndef g[T]():\n    pass\n");
    assert!(has_code(&errs, "ZAP5057"), "{errs:?}");
}

#[test]
fn derivable_with_a_keyword_argument_is_rejected() {
    let errs = errors("@derivable(name=Foo)\ndef g[T]():\n    pass\n");
    assert!(has_code(&errs, "ZAP5057"), "{errs:?}");
}

#[test]
fn derivable_with_a_non_identifier_argument_is_rejected() {
    let errs = errors("@derivable(1)\ndef g[T]():\n    pass\n");
    assert!(has_code(&errs, "ZAP5057"), "{errs:?}");
}

#[test]
fn derivable_on_a_non_generic_function_is_rejected() {
    let errs = errors("@derivable(Foo)\ndef g():\n    pass\n");
    assert!(has_code(&errs, "ZAP5057"), "{errs:?}");
}

#[test]
fn derivable_on_a_non_function_target_is_rejected() {
    // `@derivable` on a struct is a wrong-target application.
    let errs = errors("@derivable(Foo)\nstruct S:\n    int64 x\n");
    assert!(has_code(&errs, "ZAP5047"), "{errs:?}");
}

// ---------------------------------------------------------------------------
// collisions and duplicates
// ---------------------------------------------------------------------------

#[test]
fn registering_a_builtin_trait_name_is_rejected() {
    let errs = errors(&generator("Eq", "_derive_eq"));
    assert!(has_code(&errs, "ZAP5057"), "{errs:?}");
}

#[test]
fn registering_the_same_trait_twice_is_rejected() {
    let src = format!(
        "{}{}",
        generator("Serialize", "_derive_serialize_a"),
        generator("Serialize", "_derive_serialize_b")
    );
    let errs = errors(&src);
    assert!(has_code(&errs, "ZAP5057"), "{errs:?}");
}

// ---------------------------------------------------------------------------
// enums: a library trait on an enum stays unsupported (only Json, #346)
// ---------------------------------------------------------------------------

#[test]
fn a_library_trait_on_an_enum_is_rejected() {
    let src = format!(
        "{}@derive(Serialize)\nenum Color:\n    Red\n    Green\n",
        generator("Serialize", "_derive_serialize")
    );
    let errs = errors(&src);
    // Enums admit only @derive(Json) (D-0086); a library trait is a wrong-target
    // application for an enum.
    assert!(has_code(&errs, "ZAP5047"), "{errs:?}");
}
