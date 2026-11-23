//! `datetime` module — sema facet (ticket #99, D-0056).
//!
//! Civil (calendar) date-time over a Unix timestamp. Values are the built-in
//! prelude `value struct DateTime` (year/month/day/hour/minute/second), built
//! from a timestamp and rendered/parsed as ISO-8601, or via `strftime` /
//! `strptime` custom formats (the AWS SigV4 / epoch / iso8601 / rfc822 forms,
//! #286 and #338). Timezone handling is **UTC + local only** in v1:
//! `from_timestamp`/`now`/`format`/`parse`/`strptime` operate in UTC, the
//! `*_local` variants in the host's local zone; a full timezone database is
//! deferred (D-0056). `import datetime` is accepted but not required, like
//! `signal`/`gc`/`runtime`.

use zapc_diag::ErrorCode;
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

/// The prelude type name for a time zone (named or fixed-offset), returned by
/// `zone`/`fixed_offset` and consumed by `astimezone` (#1045, D-0225).
const TIMEZONE: &str = "TimeZone";

/// The prelude type names for the separate date / time-of-day values (#1046,
/// D-0226): `Date { year, month, day }` and `Time { hour, minute, second }`.
const DATE: &str = "Date";
const TIME: &str = "Time";

/// The prelude `Duration` type (#1047, D-0227): the `int64`-ns duration shared
/// with `time`, consumed by `add` and returned by `diff`.
const DURATION: &str = "Duration";

/// The `DateTime` civil fields that `replace` may override, in field order.
const REPLACE_FIELDS: [&str; 6] = ["year", "month", "day", "hour", "minute", "second"];

/// The fractional-second fields `replace` may override (#1148, D-0226):
/// `microsecond=` (Python-faithful, scaled ×1000 to the stored nanoseconds) and
/// `nanosecond=` (full precision). At most ONE may appear in a call — they both
/// set the single stored fraction.
const REPLACE_FRACTION_FIELDS: [&str; 2] = ["microsecond", "nanosecond"];

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
            ErrorCode::ZAP4025,
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
            ErrorCode::ZAP4024,
            format!("datetime.{method}() expects {want}, found {}", at.display()),
            expr_span(&args[0].value),
        );
    }
}

fn is_datetime(t: &Ty) -> bool {
    matches!(t, Ty::Named(n) if n == DATETIME)
}

fn is_timezone(t: &Ty) -> bool {
    matches!(t, Ty::Named(n) if n == TIMEZONE)
}

fn is_date(t: &Ty) -> bool {
    matches!(t, Ty::Named(n) if n == DATE)
}

fn is_time(t: &Ty) -> bool {
    matches!(t, Ty::Named(n) if n == TIME)
}

fn is_duration(t: &Ty) -> bool {
    matches!(t, Ty::Named(n) if n == DURATION)
}

/// Require exactly `n` integer arguments (the `date`/`time` constructors), each
/// inferred so it is type-checked (#1046).
fn expect_ints(checker: &mut Checker, method: &str, args: &[Arg], span: Span, n: usize) {
    if args.len() != n {
        checker.error(
            ErrorCode::ZAP4025,
            format!(
                "datetime.{method}() takes {n} arguments, found {}",
                args.len()
            ),
            span,
        );
        for a in args {
            checker.infer(&a.value);
        }
        return;
    }
    for a in args {
        let t = checker.infer(&a.value);
        if !t.is_int() && !matches!(t, Ty::Unknown) {
            checker.error(
                ErrorCode::ZAP4024,
                format!(
                    "datetime.{method}() expects int fields, found {}",
                    t.display()
                ),
                expr_span(&a.value),
            );
        }
    }
}

/// Check a `(DateTime, str)` custom-format call (`strftime` / 2-arg `format`),
/// inferring both operands so each is type-checked (#286).
fn check_strftime(checker: &mut Checker, method: &str, args: &[Arg], span: Span) {
    if args.len() != 2 {
        checker.error(
            ErrorCode::ZAP4025,
            format!(
                "datetime.{method}() takes 2 arguments, found {}",
                args.len()
            ),
            span,
        );
        for a in args {
            checker.infer(&a.value);
        }
        return;
    }
    let dt = checker.infer(&args[0].value);
    if !is_datetime(&dt) && !matches!(dt, Ty::Unknown) {
        checker.error(
            ErrorCode::ZAP4024,
            format!(
                "datetime.{method}() expects a DateTime, found {}",
                dt.display()
            ),
            expr_span(&args[0].value),
        );
    }
    let fmt = checker.infer(&args[1].value);
    if !matches!(fmt, Ty::Str | Ty::Unknown) {
        checker.error(
            ErrorCode::ZAP4024,
            format!(
                "datetime.{method}() expects a str format, found {}",
                fmt.display()
            ),
            expr_span(&args[1].value),
        );
    }
}

/// Check a `(str, str)` parse call (`strptime`), inferring both operands so
/// each is type-checked (#338). Mirrors `strftime` but with the string/format
/// argument order of Python's `datetime.strptime`.
fn check_strptime(checker: &mut Checker, args: &[Arg], span: Span) {
    if args.len() != 2 {
        checker.error(
            ErrorCode::ZAP4025,
            format!(
                "datetime.strptime() takes 2 arguments, found {}",
                args.len()
            ),
            span,
        );
        for a in args {
            checker.infer(&a.value);
        }
        return;
    }
    for (idx, what) in [(0usize, "a str"), (1usize, "a str format")] {
        let t = checker.infer(&args[idx].value);
        if !matches!(t, Ty::Str | Ty::Unknown) {
            checker.error(
                ErrorCode::ZAP4024,
                format!("datetime.strptime() expects {what}, found {}", t.display()),
                expr_span(&args[idx].value),
            );
        }
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // DateTime from a Unix timestamp (seconds since the epoch). Accepts a
        // float too (AWS `unixTimestamp` may be fractional); it is truncated to
        // whole seconds, the resolution `DateTime` carries.
        "from_timestamp" | "from_timestamp_local" => {
            expect1(
                checker,
                method,
                args,
                span,
                "an int or float timestamp",
                |t| t.is_int() || matches!(t, Ty::Float64 | Ty::Float32 | Ty::UntypedFloat),
            );
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
        // DateTime -> string. One arg: ISO-8601. Two args: strftime-style
        // custom format (`format(dt, fmt)`), the overload of `strftime` (#286).
        "format" => {
            if args.len() == 2 {
                check_strftime(checker, method, args, span);
            } else {
                expect1(checker, method, args, span, "a DateTime", is_datetime);
            }
            Some(Ty::Str)
        }
        // DateTime + strftime format string -> string (#286). Python-faithful
        // directives built from the numeric fields; see the runtime.
        "strftime" => {
            check_strftime(checker, method, args, span);
            Some(Ty::Str)
        }
        // ISO-8601 string -> DateTime (UTC).
        "parse" => {
            expect1(checker, method, args, span, "a str", |t| {
                matches!(t, Ty::Str)
            });
            Some(Ty::Named(DATETIME.to_string()))
        }
        // (str, strftime format) -> DateTime (UTC). The inverse of `strftime`;
        // parses the AWS SigV4 / iso8601 / rfc822 forms (#338).
        "strptime" => {
            check_strptime(checker, args, span);
            Some(Ty::Named(DATETIME.to_string()))
        }
        // Date arithmetic + comparisons + field helpers (#974, D-0221; the
        // duration is now a `Duration`, #1047/D-0227).
        // add(dt, delta: Duration) -> DateTime. Wall-clock arithmetic (#1149);
        // the Duration's sub-second nanos are carried into the fractional second,
        // not truncated (#1148).
        "add" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!("datetime.add() takes 2 arguments, found {}", args.len()),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                let dt = checker.infer(&args[0].value);
                if !is_datetime(&dt) && !matches!(dt, Ty::Unknown) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!("datetime.add() expects a DateTime, found {}", dt.display()),
                        expr_span(&args[0].value),
                    );
                }
                let delta = checker.infer(&args[1].value);
                if !is_duration(&delta) && !matches!(delta, Ty::Unknown) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!(
                            "datetime.add() delta must be a Duration, found {}",
                            delta.display()
                        ),
                        expr_span(&args[1].value),
                    );
                }
            }
            Some(Ty::Named(DATETIME.to_string()))
        }
        // (DateTime, DateTime): diff -> Duration (a - b, #1047); the comparisons
        // -> bool.
        "diff" | "before" | "after" | "equal" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "datetime.{method}() takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                for a in args {
                    let t = checker.infer(&a.value);
                    if !is_datetime(&t) && !matches!(t, Ty::Unknown) {
                        checker.error(
                            ErrorCode::ZAP4024,
                            format!(
                                "datetime.{method}() expects a DateTime, found {}",
                                t.display()
                            ),
                            expr_span(&a.value),
                        );
                    }
                }
            }
            Some(if method == "diff" {
                Ty::Named(DURATION.to_string())
            } else {
                Ty::Bool
            })
        }
        // (DateTime) -> int64 field helpers: weekday (Mon=0), isoweekday (Mon=1),
        // day_of_year (1..=366).
        "weekday" | "isoweekday" | "day_of_year" => {
            expect1(checker, method, args, span, "a DateTime", is_datetime);
            Some(Ty::Int64)
        }
        // Timezone database (#1045, D-0225).
        // zone(name) -> TimeZone: a named-zone handle (IANA bundle).
        "zone" => {
            expect1(checker, method, args, span, "a str zone name", |t| {
                matches!(t, Ty::Str)
            });
            Some(Ty::Named(TIMEZONE.to_string()))
        }
        // local_zone() -> TimeZone (#1171): the host system's configured zone as
        // a named handle. No arguments; raises at runtime if the host zone cannot
        // be resolved to a bundled IANA key.
        "local_zone" => {
            expect_argc(checker, "datetime", method, args, 0, span);
            Some(Ty::Named(TIMEZONE.to_string()))
        }
        // fixed_offset(minutes) -> TimeZone: a constant-offset zone.
        "fixed_offset" => {
            expect1(checker, method, args, span, "an int minutes offset", |t| {
                t.is_int()
            });
            Some(Ty::Named(TIMEZONE.to_string()))
        }
        // astimezone(dt, tz) -> DateTime: convert an instant into `tz`'s wall
        // clock, attaching its offset (an aware DateTime).
        "astimezone" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "datetime.astimezone() takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                let dt = checker.infer(&args[0].value);
                if !is_datetime(&dt) && !matches!(dt, Ty::Unknown) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!(
                            "datetime.astimezone() expects a DateTime, found {}",
                            dt.display()
                        ),
                        expr_span(&args[0].value),
                    );
                }
                let tz = checker.infer(&args[1].value);
                if !is_timezone(&tz) && !matches!(tz, Ty::Unknown) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!(
                            "datetime.astimezone() expects a TimeZone, found {}",
                            tz.display()
                        ),
                        expr_span(&args[1].value),
                    );
                }
            }
            Some(Ty::Named(DATETIME.to_string()))
        }
        // isoformat(x) -> str. Overloaded on the argument type (#1046): a
        // DateTime renders RFC-3339 (offset suffix when aware), a Date renders
        // `YYYY-MM-DD`, a Time renders `HH:MM:SS`.
        "isoformat" => {
            expect1(
                checker,
                method,
                args,
                span,
                "a DateTime, Date or Time",
                |t| is_datetime(t) || is_date(t) || is_time(t),
            );
            Some(Ty::Str)
        }
        // rfc3339(dt) -> str: the offset-aware DateTime renderer (DateTime only).
        "rfc3339" => {
            expect1(checker, method, args, span, "a DateTime", is_datetime);
            Some(Ty::Str)
        }
        // replace(dt, year=, month=, day=, hour=, minute=, second=) -> DateTime
        // (#1046, D-0226): a copy with some civil fields changed, the zone
        // (offset / is_aware) preserved. The DateTime is positional; every field
        // is an optional keyword (any subset, in any order). Bespoke binding —
        // not the trailing-only seam binder (D-0204), which rejects the interior
        // holes an all-optional surface needs.
        "replace" => {
            if args.is_empty() {
                checker.error(
                    ErrorCode::ZAP4025,
                    "datetime.replace() takes the DateTime as its first argument".to_string(),
                    span,
                );
            } else {
                if args[0].name.is_some() {
                    checker.error(
                        ErrorCode::ZAP4023,
                        "datetime.replace() takes the DateTime as a positional first argument"
                            .to_string(),
                        args[0].span,
                    );
                }
                let dt = checker.infer(&args[0].value);
                if !is_datetime(&dt) && !matches!(dt, Ty::Unknown) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!(
                            "datetime.replace() expects a DateTime, found {}",
                            dt.display()
                        ),
                        expr_span(&args[0].value),
                    );
                }
                let mut seen: Vec<&str> = Vec::new();
                for a in &args[1..] {
                    let v = checker.infer(&a.value);
                    match &a.name {
                        None => checker.error(
                            ErrorCode::ZAP4023,
                            "datetime.replace() fields must be keyword arguments \
                             (year=, month=, day=, hour=, minute=, second=, \
                             microsecond=, nanosecond=)"
                                .to_string(),
                            a.span,
                        ),
                        Some(name) => {
                            let is_civil = REPLACE_FIELDS.contains(&name.as_str());
                            let is_fraction = REPLACE_FRACTION_FIELDS.contains(&name.as_str());
                            if !is_civil && !is_fraction {
                                checker.error(
                                    ErrorCode::ZAP4023,
                                    format!("datetime.replace() has no field named '{name}'"),
                                    a.span,
                                );
                            } else if seen.contains(&name.as_str()) {
                                checker.error(
                                    ErrorCode::ZAP4023,
                                    format!(
                                        "datetime.replace() got multiple values for field '{name}'"
                                    ),
                                    a.span,
                                );
                            } else if is_fraction
                                && seen.iter().any(|s| REPLACE_FRACTION_FIELDS.contains(s))
                            {
                                // microsecond= and nanosecond= both set the one
                                // stored fraction — at most one per call (#1148).
                                checker.error(
                                    ErrorCode::ZAP4023,
                                    "datetime.replace() got multiple values for the fractional \
                                     second (pass only one of microsecond= / nanosecond=)"
                                        .to_string(),
                                    a.span,
                                );
                            } else {
                                seen.push(name.as_str());
                            }
                            if !v.is_int() && !matches!(v, Ty::Unknown) {
                                checker.error(
                                    ErrorCode::ZAP4024,
                                    format!(
                                        "datetime.replace() field '{name}' must be an int, found {}",
                                        v.display()
                                    ),
                                    expr_span(&a.value),
                                );
                            }
                        }
                    }
                }
            }
            Some(Ty::Named(DATETIME.to_string()))
        }
        // date(year, month, day) -> Date / time(hour, minute, second) -> Time
        // (#1046): plain civil-field constructors (no range validation, matching
        // the lenient `parse` family).
        "date" => {
            expect_ints(checker, method, args, span, 3);
            Some(Ty::Named(DATE.to_string()))
        }
        "time" => {
            expect_ints(checker, method, args, span, 3);
            Some(Ty::Named(TIME.to_string()))
        }
        // to_date(dt) -> Date / to_time(dt) -> Time: the civil date / time-of-day
        // of a DateTime (Python `dt.date()` / `dt.time()`).
        "to_date" => {
            expect1(checker, method, args, span, "a DateTime", is_datetime);
            Some(Ty::Named(DATE.to_string()))
        }
        "to_time" => {
            expect1(checker, method, args, span, "a DateTime", is_datetime);
            Some(Ty::Named(TIME.to_string()))
        }
        // combine(date, time) -> DateTime: a naive DateTime from a Date + Time
        // (Python `datetime.combine`).
        "combine" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!("datetime.combine() takes 2 arguments, found {}", args.len()),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                let d = checker.infer(&args[0].value);
                if !is_date(&d) && !matches!(d, Ty::Unknown) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!("datetime.combine() expects a Date, found {}", d.display()),
                        expr_span(&args[0].value),
                    );
                }
                let t = checker.infer(&args[1].value);
                if !is_time(&t) && !matches!(t, Ty::Unknown) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!("datetime.combine() expects a Time, found {}", t.display()),
                        expr_span(&args[1].value),
                    );
                }
            }
            Some(Ty::Named(DATETIME.to_string()))
        }
        // date_today() -> Date: the current local calendar date.
        "date_today" => {
            expect_argc(checker, "datetime", method, args, 0, span);
            Some(Ty::Named(DATE.to_string()))
        }
        // (Date) -> int64 field helpers (#1055): weekday (Mon=0), isoweekday
        // (Mon=1), day_of_year (1..=366) — the Date analogues of the DateTime
        // helpers.
        "date_weekday" | "date_isoweekday" | "date_day_of_year" => {
            expect1(checker, method, args, span, "a Date", is_date);
            Some(Ty::Int64)
        }
        // rfc3339_parse(s) -> DateTime: parse `...(Z|±HH:MM)` to an aware value.
        "rfc3339_parse" => {
            expect1(checker, method, args, span, "a str", |t| {
                matches!(t, Ty::Str)
            });
            Some(Ty::Named(DATETIME.to_string()))
        }
        // resolve(tz, year, month, day, hour, minute, second, fold=0) -> DateTime
        // (#1149): localize a wall clock into `tz`, classifying gap / overlap and
        // selecting the instant with `fold`. The TimeZone is positional first,
        // then six int civil fields; `fold` is an optional trailing int (keyword
        // `fold=` or an 8th positional). Bespoke binding (an optional trailing
        // arg, like `replace`), so it is omitted from the seam signatures.
        "resolve" => {
            let n = args.len();
            if !(7..=8).contains(&n) {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "datetime.resolve() takes a TimeZone, 6 civil fields and an \
                         optional fold ({}–{} arguments), found {}",
                        7, 8, n
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                if args[0].name.is_some() {
                    checker.error(
                        ErrorCode::ZAP4023,
                        "datetime.resolve() takes the TimeZone as a positional first argument"
                            .to_string(),
                        args[0].span,
                    );
                }
                let tz = checker.infer(&args[0].value);
                if !is_timezone(&tz) && !matches!(tz, Ty::Unknown) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!(
                            "datetime.resolve() expects a TimeZone, found {}",
                            tz.display()
                        ),
                        expr_span(&args[0].value),
                    );
                }
                for (i, a) in args.iter().enumerate().skip(1) {
                    // The 8th argument may be named `fold`; all others positional.
                    if let Some(name) = &a.name {
                        if !(i == n - 1 && name == "fold") {
                            checker.error(
                                ErrorCode::ZAP4023,
                                format!("datetime.resolve() has no keyword argument '{name}'"),
                                a.span,
                            );
                        }
                    }
                    let v = checker.infer(&a.value);
                    if !v.is_int() && !matches!(v, Ty::Unknown) {
                        checker.error(
                            ErrorCode::ZAP4024,
                            format!(
                                "datetime.resolve() civil fields and fold must be ints, found {}",
                                v.display()
                            ),
                            expr_span(&a.value),
                        );
                    }
                }
            }
            Some(Ty::Named(DATETIME.to_string()))
        }
        // is_ambiguous(dt) / is_imaginary(dt) -> bool (#1149): does dt's wall
        // clock occur twice (fall-back) / never (spring-forward) in its retained
        // zone. Naive / fixed values are always unique, so both are false.
        "is_ambiguous" | "is_imaginary" => {
            expect1(checker, method, args, span, "a DateTime", is_datetime);
            Some(Ty::Bool)
        }
        // resolve_imaginary(dt) -> DateTime (#1149): shift a gap (imaginary) wall
        // clock forward onto the real post-transition time; other values are
        // returned unchanged (dateutil `resolve_imaginary`).
        "resolve_imaginary" => {
            expect1(checker, method, args, span, "a DateTime", is_datetime);
            Some(Ty::Named(DATETIME.to_string()))
        }
        // zone_abbreviation(dt) -> Optional[str] (#1345): the IANA abbreviation
        // of dt's retained named zone at its instant (e.g. `EDT`/`EST`, `CET`/
        // `CEST`), faithful to the bundled tz database — including numeric forms
        // (`-04`, `+0530`) and `LMT` for pre-standard intervals, matching CPython
        // `zoneinfo.tzname()`. A naive, fixed-offset, or unknown value carries no
        // named zone (`zone_id == -1`) and yields `None`. Additive: `strftime`'s
        // `%Z` still renders the numeric offset (`UTC-04:00`) unchanged.
        "zone_abbreviation" => {
            expect1(checker, method, args, span, "a DateTime", is_datetime);
            Some(Ty::Named("Optional".to_string()))
        }
        _ => None,
    }
}
