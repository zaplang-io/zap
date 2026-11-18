//! `uuid` module — sema (type-check) facet (ticket #975, D-0232/D-0233).
//!
//! RFC 4122 / RFC 9562 UUIDs plus NanoID. The generators and `parse` return the
//! prelude `Uuid` value struct (`Ty::Named("Uuid")`); `nanoid` returns `str`.
//! The `_format`/`_hex`/`_urn`/`_version`/`_variant` helpers are the compiler-
//! internal seam calls the `Uuid` struct's accessor methods delegate to (they
//! take the raw 16 bytes and are never written by user code directly).
//!
//! `nanoid` is positional (no `seam_sigs` row): its three forms are `nanoid()`,
//! `nanoid(size)` and `nanoid(alphabet, size)`, so the single-argument form
//! binds to a *different* parameter (`size`) than the first of the two-argument
//! form (`alphabet`) — which the positional seam keyword binder cannot express.
//! Dispatched here by arity + type.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{assignable, expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "uuid",
    check,
};

/// The prelude `Uuid` value struct type.
fn uuid_ty() -> Ty {
    Ty::Named("Uuid".to_string())
}

/// A `str`/`bytes` value (they share the `ZapStr` runtime layout). Used by
/// `uuid3`/`uuid5` names and the internal accessors' raw-bytes argument.
fn expect_data(checker: &mut Checker, func: &str, arg: &Arg) {
    let t = checker.infer(&arg.value);
    if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
        checker.error(
            ErrorCode::ZAP4024,
            format!(
                "{func}() expects a str or bytes argument, found {}",
                t.display()
            ),
            expr_span(&arg.value),
        );
    }
}

/// A `Uuid` value (the namespace argument of `uuid3`/`uuid5`).
fn expect_uuid(checker: &mut Checker, func: &str, arg: &Arg) {
    let t = checker.infer(&arg.value);
    if !assignable(&t, &uuid_ty()) {
        checker.error(
            ErrorCode::ZAP4024,
            format!("{func}() expects a Uuid namespace, found {}", t.display()),
            expr_span(&arg.value),
        );
    }
}

/// An integer-valued argument (NanoID size). Lowering coerces to i64.
fn expect_int(checker: &mut Checker, func: &str, arg: &Arg) {
    let t = checker.infer(&arg.value);
    if !matches!(t, Ty::Int32 | Ty::Int64 | Ty::UntypedInt | Ty::Unknown) {
        checker.error(
            ErrorCode::ZAP4024,
            format!("{func}() expects an int size, found {}", t.display()),
            expr_span(&arg.value),
        );
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // Zero-argument generators + constants → Uuid.
        "uuid4" | "uuid7" | "uuid1" | "nil" | "max" | "namespace_dns" | "namespace_url"
        | "namespace_oid" | "namespace_x500" => {
            expect_argc(checker, "uuid", method, args, 0, span);
            Some(uuid_ty())
        }
        // uuid3(ns, name) / uuid5(ns, name) → Uuid. ns is a Uuid namespace, name
        // a str|bytes.
        "uuid3" | "uuid5" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "uuid.{method}(ns, name) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_uuid(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(uuid_ty())
        }
        // parse(s) → Uuid (raises ValueError at runtime on a malformed string).
        "parse" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!("uuid.parse(s) takes 1 argument, found {}", args.len()),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, "parse", &args[0]);
            }
            Some(uuid_ty())
        }
        // nanoid() / nanoid(size) / nanoid(alphabet, size) → str (positional).
        "nanoid" => {
            match args.len() {
                0 => {}
                1 => expect_int(checker, "nanoid", &args[0]),
                2 => {
                    expect_data(checker, "nanoid", &args[0]);
                    expect_int(checker, "nanoid", &args[1]);
                }
                n => {
                    checker.error(
                        ErrorCode::ZAP4025,
                        format!(
                            "uuid.nanoid() takes 0, 1 (size), or 2 (alphabet, size) \
                             arguments, found {n}"
                        ),
                        span,
                    );
                    for a in args {
                        checker.infer(&a.value);
                    }
                }
            }
            Some(Ty::Str)
        }
        // Internal accessors the `Uuid` prelude methods delegate to: each takes
        // the raw 16 bytes. Not user surface.
        "_format" | "_hex" | "_urn" => {
            expect_argc(checker, "uuid", method, args, 1, span);
            Some(Ty::Str)
        }
        "_version" | "_variant" => {
            expect_argc(checker, "uuid", method, args, 1, span);
            Some(Ty::Int64)
        }
        _ => None,
    }
}
