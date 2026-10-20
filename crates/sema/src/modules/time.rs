//! `time` module — sema facet (ticket #99, D-0056).
//!
//! Clocks, sleep, and durations. A **duration is an `int64` of nanoseconds**
//! (Go `time.Duration` style; this also matches the int-millisecond convention
//! signals' `grace=` uses, promoted here to nanoseconds). There is therefore no
//! dedicated duration type to add to the prelude: durations compose with
//! ordinary `int64` arithmetic (`a + b`, `end - start`), and the unit helpers
//! below just convert between a human unit and nanoseconds. `import time` is
//! accepted but not required (D-0056), like `signal`/`gc`/`runtime`.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "time",
    check,
};

/// Require exactly one numeric argument (`int64` or `float64`), inferring it so
/// it is still type-checked. Used by the duration-unit helpers and `sleep`.
fn expect_numeric1(checker: &mut Checker, method: &str, args: &[Arg], span: Span) {
    if args.len() != 1 {
        checker.error(
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
        // time.sleep_ns(ns): nanosecond-precision sleep.
        "sleep_ns" => {
            expect_numeric1(checker, "sleep_ns", args, span);
            Some(Ty::Unit)
        }
        // --- duration unit constructors (-> nanoseconds) ---
        "seconds" | "millis" | "micros" | "nanos" => {
            expect_numeric1(checker, method, args, span);
            Some(Ty::Int64)
        }
        // --- duration unit converters (nanoseconds -> unit as float) ---
        "to_seconds" | "to_millis" | "to_micros" => {
            expect_numeric1(checker, method, args, span);
            Some(Ty::Float64)
        }
        _ => None,
    }
}
