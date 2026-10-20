//! `datetime` module — sema facet (ticket #99, D-0056).
//!
//! Civil (calendar) date-time over a Unix timestamp. Values are the built-in
//! prelude `value struct DateTime` (year/month/day/hour/minute/second), built
//! from a timestamp and rendered/parsed as ISO-8601. Timezone handling is
//! **UTC + local only** in v1: `from_timestamp`/`now`/`format`/`parse` operate
//! in UTC, the `*_local` variants in the host's local zone; a full timezone
//! database is deferred (D-0056). `import datetime` is accepted but not
//! required, like `signal`/`gc`/`runtime`.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "datetime",
    check,
};

/// The prelude type name for a civil date-time. Kept in step with
/// `zapc_syntax::parser::PRELUDE_SRC`.
const DATETIME: &str = "DateTime";

/// Require exactly one argument of the named kind, inferring it so it is still
/// type-checked. `want` is a human description for the diagnostic; `ok` decides
/// whether the inferred type is acceptable.
fn expect1(
    checker: &mut Checker,
    method: &str,
    args: &[Arg],
    span: Span,
    want: &str,
    ok: impl Fn(&Ty) -> bool,
) {
    if args.len() != 1 {
        checker.error(
            format!("datetime.{method}() takes 1 argument, found {}", args.len()),
            span,
        );
        for a in args {
            checker.infer(&a.value);
        }
        return;
    }
    let at = checker.infer(&args[0].value);
    if !ok(&at) && !matches!(at, Ty::Unknown) {
        checker.error(
            format!("datetime.{method}() expects {want}, found {}", at.display()),
            expr_span(&args[0].value),
        );
    }
}

fn is_datetime(t: &Ty) -> bool {
    matches!(t, Ty::Named(n) if n == DATETIME)
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // DateTime from a Unix timestamp (seconds since the epoch).
        "from_timestamp" | "from_timestamp_local" => {
            expect1(checker, method, args, span, "an int timestamp", Ty::is_int);
            Some(Ty::Named(DATETIME.to_string()))
        }
        // Current civil time.
        "now" | "now_local" => {
            expect_argc(checker, "datetime", method, args, 0, span);
            Some(Ty::Named(DATETIME.to_string()))
        }
        // DateTime -> Unix timestamp (seconds since the epoch).
        "to_timestamp" => {
            expect1(checker, method, args, span, "a DateTime", is_datetime);
            Some(Ty::Int64)
        }
        // DateTime -> ISO-8601 string.
        "format" => {
            expect1(checker, method, args, span, "a DateTime", is_datetime);
            Some(Ty::Str)
        }
        // ISO-8601 string -> DateTime (UTC).
        "parse" => {
            expect1(checker, method, args, span, "a str", |t| {
                matches!(t, Ty::Str)
            });
            Some(Ty::Named(DATETIME.to_string()))
        }
        _ => None,
    }
}
