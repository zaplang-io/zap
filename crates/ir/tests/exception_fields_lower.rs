//! Structured fields on package-defined exceptions (#1432,
//! decisions/lang/1432-exception-fields.md).
//!
//! Before this change, `raise TokenEndpointError("m", "invalid_client", 401)`
//! failed lowering with `ZAP6001: TokenEndpointError() takes one positional
//! message` — a raise of an `@exception` struct could carry only a message
//! string, and a caught binding was the message string, so `failure.error`
//! failed with `ZAP6001: field access on a non-struct value`.
//!
//! The compatibility model (Option A): the catch binding STAYS the message
//! string — so `str(e)` / `print(e)` and every #1278 message-only catch are
//! unchanged — and `e.<field>` reads the declared field off a struct payload
//! attached to the exception object at the raise site. These host-only checks
//! (no LLVM) pin the lowered IR shape: a package-defined raise builds the real
//! struct, derives a message, and attaches the payload
//! (`zap_exception_set_payload`); a typed field read loads the payload
//! (`zap_exception_payload`) and does an ordinary struct `FieldGet`; a
//! message-only catch still reads `zap_exception_message` and attaches/reads no
//! payload. The end-to-end runtime VALUES (string / int64 / Optional fields
//! across a raise/catch and across module imports, catch-family dispatch, and
//! an unchanged `str(e)`) are covered by the `exception_fields*` goldens on CI,
//! since field layout and the precise collector only run under native codegen.

use zapc_ir::{lower_module, Inst};
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting it is clean, and return `main`'s flattened IR.
fn lower_main_insts(src: &str) -> Vec<Inst> {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    let main = module
        .funcs
        .iter()
        .find(|f| f.is_main)
        .expect("a lowered program has a `main`");
    main.blocks.iter().flat_map(|b| b.insts.clone()).collect()
}

/// Count `Inst::Call`s to a runtime function by name.
fn calls_to(insts: &[Inst], func: &str) -> usize {
    insts
        .iter()
        .filter(|i| matches!(i, Inst::Call { func: f, .. } if f == func))
        .count()
}

/// Count ordinary struct field reads.
fn field_gets(insts: &[Inst]) -> usize {
    insts
        .iter()
        .filter(|i| matches!(i, Inst::FieldGet { .. }))
        .count()
}

// ----- The issue's exact repro: str + int64 fields, read in a typed catch -----

#[test]
fn typed_catch_reads_string_and_int_fields() {
    // Previously this failed lowering at the raise site; now it lowers cleanly.
    let insts = lower_main_insts(
        "@exception\n\
         struct TokenEndpointError:\n\
         \x20   str message\n\
         \x20   str error\n\
         \x20   int64 status\n\
         def main():\n\
         \x20   try:\n\
         \x20       raise TokenEndpointError(\"request refused\", \"invalid_client\", 401)\n\
         \x20   except TokenEndpointError as failure:\n\
         \x20       assert failure.error == \"invalid_client\"\n\
         \x20       assert failure.status == 401\n",
    );
    // The raise constructs the real struct (a `StructNew`), derives its message,
    // and attaches it as the exception payload.
    assert!(
        insts.iter().any(|i| matches!(i, Inst::StructNew { .. })),
        "a package-defined raise must construct the declared struct instance"
    );
    assert_eq!(
        calls_to(&insts, "zap_exception_set_payload"),
        1,
        "the raise must attach the constructed struct as the exception payload"
    );
    // Each typed field read loads the payload and does a struct field read. Two
    // reads here (`failure.error`, `failure.status`).
    assert_eq!(
        calls_to(&insts, "zap_exception_payload"),
        2,
        "each `failure.<field>` read must load the payload off the exception"
    );
    // The message binding is still materialized (the catch binds the message
    // string, unchanged), so a message read is still present.
    assert!(
        calls_to(&insts, "zap_exception_message") >= 1,
        "the catch must still bind the message string (Option A, unchanged)"
    );
    // The two typed reads plus the message-field derivation at the raise site
    // are ordinary struct field reads.
    assert!(
        field_gets(&insts) >= 3,
        "two typed field reads + the raise-site message derivation are FieldGets"
    );
}

// ----- An Optional field survives the raise/catch boundary -----

#[test]
fn typed_catch_reads_optional_field() {
    // A declared `Optional[str]` field is constructed, carried on the payload,
    // and read back in the typed catch — a bare `None` default included.
    let insts = lower_main_insts(
        "@exception\n\
         struct TokenEndpointError:\n\
         \x20   str message\n\
         \x20   str error\n\
         \x20   Optional[str] description = None\n\
         def main():\n\
         \x20   try:\n\
         \x20       raise TokenEndpointError(\"refused\", \"invalid_client\", Optional.Some(\"bad client\"))\n\
         \x20   except TokenEndpointError as failure:\n\
         \x20       assert failure.description.unwrap() == \"bad client\"\n",
    );
    assert_eq!(
        calls_to(&insts, "zap_exception_set_payload"),
        1,
        "the Optional field rides on the attached struct payload"
    );
    assert!(
        calls_to(&insts, "zap_exception_payload") >= 1,
        "`failure.description` must read the Optional field off the payload"
    );
}

// ----- Optional field: Some(...) and a defaulted None both lower off payload -----

#[test]
fn optional_field_some_and_defaulted_none_lower() {
    // Reading `e.uri.is_some()` / `.unwrap()` for a supplied `Some`, and
    // `e.uri.is_none()` for a defaulted-omitted `None`, both lower cleanly off
    // the payload — the Optional field survives construction (Some and its
    // declared `None` default) and the raise/catch boundary.
    let insts = lower_main_insts(
        "@exception\n\
         struct TokenEndpointError:\n\
         \x20   str message\n\
         \x20   str error\n\
         \x20   Optional[str] uri = None\n\
         def main():\n\
         \x20   try:\n\
         \x20       raise TokenEndpointError(\"refused\", \"invalid_client\", Optional.Some(\"u\"))\n\
         \x20   except TokenEndpointError as f:\n\
         \x20       if f.uri.is_some():\n\
         \x20           print(f.uri.unwrap())\n\
         \x20   try:\n\
         \x20       raise TokenEndpointError(\"refused\", \"invalid_client\")\n\
         \x20   except TokenEndpointError as g:\n\
         \x20       if g.uri.is_none():\n\
         \x20           print(\"none\")\n",
    );
    // `f.uri` (twice: is_some + unwrap) and `g.uri` (is_none) each load the
    // payload — three payload reads in all.
    assert!(
        calls_to(&insts, "zap_exception_payload") >= 3,
        "each Optional-field access reads the payload off the exception"
    );
    assert_eq!(
        calls_to(&insts, "zap_exception_set_payload"),
        2,
        "both raises attach their struct payload"
    );
}

// ----- A message-only catch is unchanged: no payload read -----

#[test]
fn message_only_catch_binds_message_no_payload_read() {
    // `except E as e: print(e)` reads only the message string — Option A keeps
    // this identical to #1278 (no `zap_exception_payload` read).
    let insts = lower_main_insts(
        "@exception\n\
         struct DNSException:\n\
         \x20   str message\n\
         def main():\n\
         \x20   try:\n\
         \x20       raise DNSException(\"no such domain\")\n\
         \x20   except DNSException as e:\n\
         \x20       print(e)\n",
    );
    assert!(
        calls_to(&insts, "zap_exception_message") >= 1,
        "a message-only catch binds the message string"
    );
    assert_eq!(
        calls_to(&insts, "zap_exception_payload"),
        0,
        "no field is read, so the payload is never loaded in the handler"
    );
    // The raise still attaches a payload (uniform raise path), but the message
    // slot — which `print(e)` reads — is derived from the `message` field, so
    // the diagnostic/str behavior is unchanged.
    assert_eq!(
        calls_to(&insts, "zap_exception_set_payload"),
        1,
        "every package-defined raise attaches its struct payload uniformly"
    );
}

// ----- Catch-family dispatch still lowers; the concrete clause reads fields -----

#[test]
fn family_dispatch_lowers_and_concrete_clause_reads_fields() {
    // A member raised and caught by its family base still dispatches (the base
    // clause binds the message), and a concrete clause of the member reads its
    // own declared fields off the payload.
    let insts = lower_main_insts(
        "@exception\n\
         struct DNSException:\n\
         \x20   str message\n\
         @exception(DNSException)\n\
         struct NXDOMAIN:\n\
         \x20   str message\n\
         \x20   str domain\n\
         def main():\n\
         \x20   try:\n\
         \x20       raise NXDOMAIN(\"no such domain\", \"example.com\")\n\
         \x20   except DNSException as e:\n\
         \x20       print(e)\n\
         \x20   try:\n\
         \x20       raise NXDOMAIN(\"no such domain\", \"example.com\")\n\
         \x20   except NXDOMAIN as nx:\n\
         \x20       assert nx.domain == \"example.com\"\n",
    );
    // The family dispatch (is_a) is still present for the base catch.
    assert!(
        calls_to(&insts, "zap_exception_is_a") >= 1,
        "family dispatch still matches via the parent-chain walk"
    );
    // Two raises => two payload attachments.
    assert_eq!(
        calls_to(&insts, "zap_exception_set_payload"),
        2,
        "both member raises attach their struct payload"
    );
    // Exactly one typed field read — `nx.domain` in the concrete clause; the
    // base clause reads only the message.
    assert_eq!(
        calls_to(&insts, "zap_exception_payload"),
        1,
        "only the concrete clause reads a declared field off the payload"
    );
}
