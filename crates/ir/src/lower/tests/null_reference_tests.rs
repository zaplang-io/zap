//! `NullReferenceError` type id + located message shape (I#1492, Epic I#1489,
//! decisions/lang/1489-null-reference-error.md).
//!
//! Host-only: these pin the two compile-time halves of #1492 — the stable
//! runtime id that lowering assigns the built-in name, and the single
//! source-of-truth message builder #1493's deref lowering will call. The catch
//! behaviour (own type / `Exception` / re-raise / non-matching clause) is pinned
//! end-to-end by the `null_reference_error_*` goldens; sema acceptance in a
//! typed `except`/`raise` is pinned by `crates/sema/tests/null_reference_error.rs`.

use crate::lower::{exception_type_id_name, null_reference_error_message, NullDerefSite};

#[test]
fn null_reference_error_has_stable_id_22() {
    // Locked by the ADR (Q2) and mirrored in zaprt.c's `ZAP_EXC_NULL_REFERENCE_ERROR`.
    assert_eq!(exception_type_id_name("NullReferenceError"), Some(22));
}

#[test]
fn null_reference_error_id_is_distinct_from_neighbours() {
    // Guard the enum against a future insertion silently renumbering it: id 22
    // sits immediately after `ResponseTooLargeError = 21`, and no other built-in
    // shares it.
    assert_eq!(exception_type_id_name("ResponseTooLargeError"), Some(21));
    assert_eq!(exception_type_id_name("NullReferenceError"), Some(22));
    for other in [
        "Exception",
        "RuntimeError",
        "OSError",
        "FileNotFoundError",
        "MemoryError",
        "ValueError",
    ] {
        assert_ne!(
            exception_type_id_name(other),
            Some(22),
            "{other} must not collide with NullReferenceError's id"
        );
    }
}

#[test]
fn message_matches_locked_adr_example_exactly() {
    // The one concrete example the maintainer locked (ADR Q2): a field read with
    // both the field name and its owning static type known.
    assert_eq!(
        null_reference_error_message(
            NullDerefSite::FieldRead,
            Some("name"),
            Some("Node"),
            "foo.zap",
            7,
        ),
        "null dereference reading field 'name' of 'Node' at foo.zap:7"
    );
}

#[test]
fn message_always_carries_the_required_location() {
    // The source location is REQUIRED in every spelling, including when neither
    // the field nor the owner is known at the site.
    for site in [
        NullDerefSite::FieldRead,
        NullDerefSite::FieldStore,
        NullDerefSite::MethodReceiver,
        NullDerefSite::IndexBase,
        NullDerefSite::IterBase,
    ] {
        let msg = null_reference_error_message(site, None, None, "a/b/c.zap", 123);
        assert!(
            msg.starts_with("null dereference"),
            "every message starts with the kind: {msg:?}"
        );
        assert!(
            msg.ends_with(" at a/b/c.zap:123"),
            "every message ends with the required location: {msg:?}"
        );
    }
}

#[test]
fn field_sites_include_field_and_owner_when_known() {
    assert_eq!(
        null_reference_error_message(NullDerefSite::FieldRead, Some("x"), Some("P"), "f.zap", 1),
        "null dereference reading field 'x' of 'P' at f.zap:1"
    );
    assert_eq!(
        null_reference_error_message(NullDerefSite::FieldStore, Some("x"), Some("P"), "f.zap", 1),
        "null dereference writing field 'x' of 'P' at f.zap:1"
    );
    assert_eq!(
        null_reference_error_message(
            NullDerefSite::MethodReceiver,
            Some("run"),
            Some("Job"),
            "f.zap",
            9,
        ),
        "null dereference calling method 'run' on 'Job' at f.zap:9"
    );
}

#[test]
fn field_sites_drop_owner_clause_when_owner_unknown() {
    // Field known, owning type not: keep the field, drop the ` of '…'` tail.
    assert_eq!(
        null_reference_error_message(NullDerefSite::FieldRead, Some("name"), None, "f.zap", 2),
        "null dereference reading field 'name' at f.zap:2"
    );
    assert_eq!(
        null_reference_error_message(NullDerefSite::FieldStore, Some("name"), None, "f.zap", 2),
        "null dereference writing field 'name' at f.zap:2"
    );
    assert_eq!(
        null_reference_error_message(NullDerefSite::MethodReceiver, Some("run"), None, "f.zap", 2,),
        "null dereference calling method 'run' at f.zap:2"
    );
}

#[test]
fn field_sites_collapse_to_bare_kind_when_nothing_known() {
    // Defensive: a field/store site with neither field nor owner degrades to the
    // bare located form rather than emitting empty quotes.
    assert_eq!(
        null_reference_error_message(NullDerefSite::FieldRead, None, Some("Node"), "f.zap", 3),
        "null dereference at f.zap:3"
    );
    assert_eq!(
        null_reference_error_message(NullDerefSite::FieldStore, None, None, "f.zap", 3),
        "null dereference at f.zap:3"
    );
    // The method site keeps a generic phrasing (and its owner, if known) because
    // "calling a method" reads naturally without a name.
    assert_eq!(
        null_reference_error_message(NullDerefSite::MethodReceiver, None, Some("Job"), "f.zap", 3),
        "null dereference calling a method on 'Job' at f.zap:3"
    );
    assert_eq!(
        null_reference_error_message(NullDerefSite::MethodReceiver, None, None, "f.zap", 3),
        "null dereference calling a method at f.zap:3"
    );
}

#[test]
fn index_and_iter_sites_use_their_own_verbs() {
    assert_eq!(
        null_reference_error_message(NullDerefSite::IndexBase, None, Some("Grid"), "f.zap", 4),
        "null dereference indexing into 'Grid' at f.zap:4"
    );
    assert_eq!(
        null_reference_error_message(NullDerefSite::IndexBase, None, None, "f.zap", 4),
        "null dereference indexing at f.zap:4"
    );
    assert_eq!(
        null_reference_error_message(NullDerefSite::IterBase, None, Some("Seq"), "f.zap", 5),
        "null dereference iterating over 'Seq' at f.zap:5"
    );
    assert_eq!(
        null_reference_error_message(NullDerefSite::IterBase, None, None, "f.zap", 5),
        "null dereference iterating at f.zap:5"
    );
}
