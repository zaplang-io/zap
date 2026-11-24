//! `time` module — sema facet (ticket #99, D-0056).
//!
//! Clocks, sleep, and durations. A **duration is an `int64` of nanoseconds**
//! (Go `time.Duration` style; this also matches the int-millisecond convention
//! signals' `grace=` uses, promoted here to nanoseconds). There is therefore no
//! dedicated duration type to add to the prelude: durations compose with
//! ordinary `int64` arithmetic (`a + b`, `end - start`), and the unit helpers
//! below just convert between a human unit and nanoseconds. `import time` is
//! accepted but not required (D-0056), like `signal`/`gc`/`runtime`.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "time",
    check,
};

/// The prelude type name for a duration (#1047, D-0227): shared by `time` and
/// `datetime`, an `int64`-nanosecond value struct.
const DURATION: &str = "Duration";

/// Require exactly one `Duration` argument, inferring it so it is type-checked.
fn expect_duration1(checker: &mut Checker, method: &str, args: &[Arg], span: Span) {
    if args.len() != 1 {
        checker.error(
            ErrorCode::ZAP4025,
            format!("time.{method}() takes 1 argument, found {}", args.len()),
            span,
        );
        for a in args {
            checker.infer(&a.value);
        }
        return;
    }
    let at = checker.infer(&args[0].value);
    if !matches!(&at, Ty::Named(n) if n == DURATION) && !matches!(at, Ty::Unknown) {
        checker.error(
            ErrorCode::ZAP4024,
            format!("time.{method}() expects a Duration, found {}", at.display()),
            expr_span(&args[0].value),
        );
    }
}

/// Require exactly one numeric argument (`int64` or `float64`), inferring it so
/// it is still type-checked. Used by the duration-unit helpers and `sleep`.
fn expect_numeric1(checker: &mut Checker, method: &str, args: &[Arg], span: Span) {
    if args.len() != 1 {
        checker.error(
            ErrorCode::ZAP4025,
            format!("time.{method}() takes 1 argument, found {}", args.len()),
            span,
        );
        for a in args {
            checker.infer(&a.value);
        }
        return;
    }
    let at = checker.infer(&args[0].value);
    if !at.is_numeric() && !matches!(at, Ty::Unknown) {
        checker.error(
            ErrorCode::ZAP4024,
            format!("time.{method}() expects a number, found {}", at.display()),
            expr_span(&args[0].value),
        );
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // --- clocks ---
        // monotonic clock (never goes backwards; for measuring elapsed time).
        "monotonic_ns" => {
            expect_argc(checker, "time", "monotonic_ns", args, 0, span);
            Some(Ty::Int64)
        }
        "monotonic" => {
            expect_argc(checker, "time", "monotonic", args, 0, span);
            Some(Ty::Float64)
        }
        // wall clock (Unix time since the epoch).
        "time_ns" => {
            expect_argc(checker, "time", "time_ns", args, 0, span);
            Some(Ty::Int64)
        }
        "time" => {
            expect_argc(checker, "time", "time", args, 0, span);
            Some(Ty::Float64)
        }
        // --- sleep ---
        // time.sleep(seconds): accepts int or float seconds (Python-familiar).
        "sleep" => {
            expect_numeric1(checker, "sleep", args, span);
            Some(Ty::Unit)
        }
        // time.sleep_ns(ns): nanosecond-precision sleep (raw int).
        "sleep_ns" => {
            expect_numeric1(checker, "sleep_ns", args, span);
            Some(Ty::Unit)
        }
        // time.sleep_for(d): sleep for a Duration (#1047, D-0227).
        "sleep_for" => {
            expect_duration1(checker, "sleep_for", args, span);
            Some(Ty::Unit)
        }
        // time.format(d) -> str: a human-readable Duration (Go-style, #1055).
        "format" => {
            expect_duration1(checker, "format", args, span);
            Some(Ty::Str)
        }
        // time.after(d) -> channel[int64] (#1395): a one-shot timer channel for
        // `select`. Fires once after the Duration, delivering the monotonic-ns
        // instant at which it fired (Go `time.After` shape).
        "after" => {
            expect_duration1(checker, "after", args, span);
            Some(Ty::Chan(Box::new(Ty::Int64)))
        }
        // time.ticker(d) -> Ticker (#1409): a stoppable periodic timer. Returns
        // the prelude `Ticker` handle (receive channel `chan` + `stop()`); each
        // tick delivers the monotonic-ns instant, on a fixed schedule.
        "ticker" => {
            expect_duration1(checker, "ticker", args, span);
            Some(Ty::Named("Ticker".to_string()))
        }
        // time.since(start_ns) -> Duration: elapsed since a saved monotonic_ns()
        // reading, as a typed Duration (#1055).
        "since" => {
            expect_numeric1(checker, "since", args, span);
            Some(Ty::Named(DURATION.to_string()))
        }
        // --- duration unit constructors (-> Duration, #1047) ---
        "seconds" | "millis" | "micros" | "nanos" => {
            expect_numeric1(checker, method, args, span);
            Some(Ty::Named(DURATION.to_string()))
        }
        // --- duration unit converters (Duration -> unit as float, #1047) ---
        "to_seconds" | "to_millis" | "to_micros" => {
            expect_duration1(checker, method, args, span);
            Some(Ty::Float64)
        }
        _ => None,
    }
}
