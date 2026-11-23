//! `datetime` timezone surface — sema shape/arity/type checks (#1045, D-0225).
//!
//! Exhaustive type-checking for the timezone database surface: `zone`,
//! `fixed_offset`, `astimezone`, `isoformat`/`rfc3339`, and `rfc3339_parse`,
//! including their return types (the prelude `TimeZone`/`DateTime` value
//! structs) and the argument type/arity diagnostics. Host-only (parse + sema),
//! no LLVM.

use zapc_sema::check_str;

#[track_caller]
fn sema_errors(src: &str) -> Vec<(String, String)> {
    let (terrs, perrs, lerrs) = check_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    terrs
        .into_iter()
        .map(|e| (e.code.to_string(), e.message))
        .collect()
}

#[track_caller]
fn assert_ok(src: &str) {
    let errs = sema_errors(src);
    assert!(errs.is_empty(), "expected no sema errors, got: {errs:?}");
}

#[track_caller]
fn assert_code(src: &str, code: &str) {
    let errs = sema_errors(src);
    assert!(
        errs.iter().any(|(c, _)| c == code),
        "expected a {code} error, got: {errs:?}",
    );
}

fn prog(body: &str) -> String {
    format!("def f():\n{body}\n")
}

// ---------------------------------------------------------------------------
// Return types — zone/fixed_offset -> TimeZone; astimezone/rfc3339_parse ->
// DateTime; isoformat/rfc3339 -> str. Annotated locals force the check.
// ---------------------------------------------------------------------------

#[test]
fn zone_and_fixed_offset_return_timezone() {
    assert_ok(&prog("    TimeZone z = datetime.zone(\"Asia/Kolkata\")\n"));
    assert_ok(&prog("    TimeZone z = datetime.fixed_offset(330)\n"));
    assert_ok(&prog("    TimeZone z = datetime.fixed_offset(-300)\n"));
}

#[test]
fn astimezone_returns_datetime() {
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    \
         TimeZone z = datetime.fixed_offset(330)\n    \
         DateTime a = datetime.astimezone(t, z)\n",
    ));
    // The result is itself a DateTime, so it flows into the #974 helpers.
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    \
         auto z = datetime.zone(\"UTC\")\n    \
         auto a = datetime.astimezone(t, z)\n    \
         auto w = datetime.weekday(a)\n",
    ));
}

#[test]
fn isoformat_rfc3339_return_str() {
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    str s = datetime.isoformat(t)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    str s = datetime.rfc3339(t)\n",
    ));
}

#[test]
fn rfc3339_parse_returns_datetime() {
    assert_ok(&prog(
        "    DateTime d = datetime.rfc3339_parse(\"2021-06-01T05:30:00+05:30\")\n",
    ));
    assert_ok(&prog(
        "    auto d = datetime.rfc3339_parse(\"2021-06-01T00:00:00Z\")\n    \
         auto ts = datetime.to_timestamp(d)\n",
    ));
}

// ---------------------------------------------------------------------------
// Argument type diagnostics (ZAP4024).
// ---------------------------------------------------------------------------

#[test]
fn zone_rejects_non_str() {
    assert_code(&prog("    auto z = datetime.zone(42)\n"), "ZAP4024");
}

#[test]
fn fixed_offset_rejects_non_int() {
    assert_code(
        &prog("    auto z = datetime.fixed_offset(\"nope\")\n"),
        "ZAP4024",
    );
}

#[test]
fn astimezone_rejects_bad_operands() {
    // First arg must be a DateTime.
    assert_code(
        &prog(
            "    auto z = datetime.fixed_offset(0)\n    auto a = datetime.astimezone(\"x\", z)\n",
        ),
        "ZAP4024",
    );
    // Second arg must be a TimeZone (a DateTime is not one).
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto a = datetime.astimezone(t, t)\n"),
        "ZAP4024",
    );
}

#[test]
fn isoformat_rejects_non_datetime() {
    assert_code(&prog("    auto s = datetime.isoformat(\"x\")\n"), "ZAP4024");
    assert_code(&prog("    auto s = datetime.rfc3339(5)\n"), "ZAP4024");
}

#[test]
fn rfc3339_parse_rejects_non_str() {
    assert_code(&prog("    auto d = datetime.rfc3339_parse(5)\n"), "ZAP4024");
}

// ---------------------------------------------------------------------------
// Arity diagnostics (ZAP4025).
// ---------------------------------------------------------------------------

#[test]
fn arity_errors() {
    assert_code(&prog("    auto z = datetime.zone()\n"), "ZAP4025");
    assert_code(
        &prog("    auto z = datetime.zone(\"UTC\", \"extra\")\n"),
        "ZAP4025",
    );
    assert_code(&prog("    auto z = datetime.fixed_offset()\n"), "ZAP4025");
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto a = datetime.astimezone(t)\n"),
        "ZAP4025",
    );
    assert_code(
        &prog(
            "    auto t = datetime.from_timestamp(0)\n    \
             auto z = datetime.fixed_offset(0)\n    \
             auto a = datetime.astimezone(t, z, z)\n",
        ),
        "ZAP4025",
    );
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto s = datetime.isoformat(t, t)\n"),
        "ZAP4025",
    );
}

// ---------------------------------------------------------------------------
// #1046 — replace() + separate Date/Time types.
// ---------------------------------------------------------------------------

#[test]
fn replace_accepts_any_subset_of_optional_fields() {
    // No fields, one field, every field.
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    DateTime r = datetime.replace(t)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    DateTime r = datetime.replace(t, year=2020)\n",
    ));
    // The interior-hole case the trailing-only seam binder (D-0204) would reject:
    // `day` supplied, `year`/`month` omitted. Bespoke binding must accept it.
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    DateTime r = datetime.replace(t, day=1)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    \
         DateTime r = datetime.replace(t, year=2020, month=2, day=29, hour=1, minute=2, second=3)\n",
    ));
    // Any order.
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    DateTime r = datetime.replace(t, second=5, year=1999)\n",
    ));
}

#[test]
fn replace_rejects_bad_calls() {
    // No DateTime at all.
    assert_code(&prog("    auto r = datetime.replace()\n"), "ZAP4025");
    // DateTime must be positional, not a keyword.
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(dt=t, year=1)\n"),
        "ZAP4023",
    );
    // A field as a positional (must be keyword).
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(t, 2020)\n"),
        "ZAP4023",
    );
    // Unknown field.
    assert_code(
        &prog(
            "    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(t, bogus=1)\n",
        ),
        "ZAP4023",
    );
    // Duplicate field.
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(t, year=1, year=2)\n"),
        "ZAP4023",
    );
    // Non-int field value.
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(t, year=\"x\")\n"),
        "ZAP4024",
    );
    // First arg not a DateTime.
    assert_code(
        &prog("    auto r = datetime.replace(\"x\", year=1)\n"),
        "ZAP4024",
    );
}

// ---------------------------------------------------------------------------
// #1148 — subsecond precision: replace(microsecond=/nanosecond=) + local_zone.
// ---------------------------------------------------------------------------

#[test]
fn replace_accepts_fractional_fields() {
    // microsecond= and nanosecond= are optional, int, any-subset keywords, and
    // may combine with the civil fields (any order), like the other fields.
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    DateTime r = datetime.replace(t, microsecond=1)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    DateTime r = datetime.replace(t, nanosecond=1)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    \
         DateTime r = datetime.replace(t, day=2, microsecond=250000)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    \
         DateTime r = datetime.replace(t, nanosecond=123456789, year=2020)\n",
    ));
}

#[test]
fn replace_rejects_bad_fractional_fields() {
    // A non-int fractional value.
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(t, microsecond=\"x\")\n"),
        "ZAP4024",
    );
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(t, nanosecond=1.5)\n"),
        "ZAP4024",
    );
    // A repeated fractional field.
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(t, microsecond=1, microsecond=2)\n"),
        "ZAP4023",
    );
    // microsecond= and nanosecond= both set the single stored fraction — at most
    // one per call.
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(t, microsecond=1, nanosecond=2)\n"),
        "ZAP4023",
    );
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto r = datetime.replace(t, nanosecond=2, microsecond=1)\n"),
        "ZAP4023",
    );
}

#[test]
fn local_zone_returns_timezone_zero_arg() {
    // #1171 — no arguments, returns a TimeZone usable in astimezone/resolve.
    assert_ok(&prog("    TimeZone z = datetime.local_zone()\n"));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    \
         auto z = datetime.local_zone()\n    \
         DateTime a = datetime.astimezone(t, z)\n",
    ));
    // Arity: no positional / keyword arguments.
    assert_code(
        &prog("    auto z = datetime.local_zone(\"UTC\")\n"),
        "ZAP4025",
    );
}

#[test]
fn date_time_constructors_and_accessors() {
    assert_ok(&prog("    Date d = datetime.date(2020, 2, 29)\n"));
    assert_ok(&prog("    Time t = datetime.time(1, 2, 3)\n"));
    assert_ok(&prog("    Date d = datetime.date_today()\n"));
    // Keyword + reordered (seam binding).
    assert_ok(&prog(
        "    Date d = datetime.date(day=1, year=2020, month=6)\n",
    ));
    assert_ok(&prog(
        "    Time t = datetime.time(second=3, hour=1, minute=2)\n",
    ));
    // Field access on the structs.
    assert_ok(&prog(
        "    auto d = datetime.date(2020, 1, 1)\n    auto y = d.year\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.time(1, 2, 3)\n    auto s = t.second\n",
    ));
}

#[test]
fn to_date_to_time_combine() {
    assert_ok(&prog(
        "    auto dt = datetime.from_timestamp(0)\n    Date d = datetime.to_date(dt)\n",
    ));
    assert_ok(&prog(
        "    auto dt = datetime.from_timestamp(0)\n    Time t = datetime.to_time(dt)\n",
    ));
    assert_ok(&prog(
        "    auto d = datetime.date(2020, 1, 1)\n    auto t = datetime.time(1, 2, 3)\n    \
         DateTime dt = datetime.combine(d, t)\n",
    ));
    // combine arg types enforced.
    assert_code(
        &prog("    auto d = datetime.date(2020, 1, 1)\n    auto dt = datetime.combine(d, d)\n"),
        "ZAP4024",
    );
}

#[test]
fn isoformat_overloads_date_time_datetime() {
    assert_ok(&prog(
        "    auto dt = datetime.from_timestamp(0)\n    str s = datetime.isoformat(dt)\n",
    ));
    assert_ok(&prog(
        "    auto d = datetime.date(2020, 1, 1)\n    str s = datetime.isoformat(d)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.time(1, 2, 3)\n    str s = datetime.isoformat(t)\n",
    ));
    // A TimeZone is not formattable by isoformat.
    assert_code(
        &prog("    auto z = datetime.fixed_offset(0)\n    auto s = datetime.isoformat(z)\n"),
        "ZAP4024",
    );
    // rfc3339 stays DateTime-only (a Date is rejected).
    assert_code(
        &prog("    auto d = datetime.date(2020, 1, 1)\n    auto s = datetime.rfc3339(d)\n"),
        "ZAP4024",
    );
}

#[test]
fn date_time_constructor_type_and_arity() {
    assert_code(
        &prog("    auto d = datetime.date(\"x\", 1, 1)\n"),
        "ZAP4024",
    );
    assert_code(&prog("    auto d = datetime.date(2020, 1)\n"), "ZAP4025");
    assert_code(&prog("    auto t = datetime.time(1, 2, 3, 4)\n"), "ZAP4025");
    assert_code(&prog("    auto d = datetime.date_today(1)\n"), "ZAP4025");
}

// ---------------------------------------------------------------------------
// #1047 — the reconciled Duration type across time + datetime.
// ---------------------------------------------------------------------------

#[test]
fn duration_dunder_arithmetic_and_comparisons() {
    // The prelude Duration's operators (D-0047 dunders) type-check.
    assert_ok(&prog(
        "    auto a = Duration(nanos=1000)\n    auto b = Duration(nanos=500)\n    \
         Duration c = a + b\n    Duration d = a - b\n    Duration e = a * 3\n    \
         auto lt = a < b\n    auto eq = a == b\n",
    ));
}

#[test]
fn time_constructors_return_duration() {
    assert_ok(&prog("    Duration d = time.seconds(5)\n"));
    assert_ok(&prog("    Duration d = time.millis(250)\n"));
    assert_ok(&prog("    Duration d = time.micros(7)\n"));
    assert_ok(&prog("    Duration d = time.nanos(42)\n"));
    // Converters take a Duration and return float.
    assert_ok(&prog(
        "    auto d = time.seconds(2)\n    float64 s = time.to_seconds(d)\n",
    ));
    // sleep_for takes a Duration; a raw int is rejected.
    assert_ok(&prog(
        "    auto d = time.millis(10)\n    time.sleep_for(d)\n",
    ));
    assert_code(&prog("    time.sleep_for(10)\n"), "ZAP4024");
    assert_code(&prog("    auto s = time.to_seconds(10)\n"), "ZAP4024");
}

#[test]
fn datetime_add_takes_duration_diff_returns_duration() {
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    auto d = time.seconds(86400)\n    \
         DateTime t2 = datetime.add(t, d)\n",
    ));
    assert_ok(&prog(
        "    auto a = datetime.from_timestamp(0)\n    auto b = datetime.from_timestamp(1)\n    \
         Duration delta = datetime.diff(b, a)\n",
    ));
    // add with a raw int (the old seconds API) is now a type error.
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto t2 = datetime.add(t, 86400)\n"),
        "ZAP4024",
    );
    // diff flows into Duration arithmetic.
    assert_ok(&prog(
        "    auto a = datetime.from_timestamp(0)\n    auto b = datetime.from_timestamp(1)\n    \
         auto delta = datetime.diff(b, a)\n    auto doubled = delta * 2\n    float64 s = time.to_seconds(doubled)\n",
    ));
}

// ---------------------------------------------------------------------------
// #1055 — deferred refinements: Duration formatting + since, Date/Time
// comparisons + field helpers.
// ---------------------------------------------------------------------------

#[test]
fn time_format_and_since() {
    assert_ok(&prog(
        "    auto d = time.seconds(90)\n    str s = time.format(d)\n",
    ));
    assert_ok(&prog(
        "    auto start = time.monotonic_ns()\n    Duration e = time.since(start)\n",
    ));
    // format wants a Duration; since wants a number.
    assert_code(&prog("    auto s = time.format(90)\n"), "ZAP4024");
}

#[test]
fn date_time_comparisons_via_derive() {
    assert_ok(&prog(
        "    auto a = datetime.date(2020, 1, 1)\n    auto b = datetime.date(2021, 6, 1)\n    \
         bool lt = a < b\n    bool eq = a == a\n",
    ));
    assert_ok(&prog(
        "    auto a = datetime.time(1, 0, 0)\n    auto b = datetime.time(2, 30, 0)\n    \
         bool le = a <= b\n",
    ));
}

#[test]
fn date_field_helpers() {
    assert_ok(&prog(
        "    auto d = datetime.date(2021, 1, 1)\n    int64 w = datetime.date_weekday(d)\n    \
         int64 iw = datetime.date_isoweekday(d)\n    int64 doy = datetime.date_day_of_year(d)\n",
    ));
    // They want a Date, not a DateTime or int.
    assert_code(&prog("    auto w = datetime.date_weekday(5)\n"), "ZAP4024");
    assert_code(
        &prog("    auto dt = datetime.from_timestamp(0)\n    auto w = datetime.date_weekday(dt)\n"),
        "ZAP4024",
    );
}

// ---------------------------------------------------------------------------
// #1149 — named-zone retention + civil-time resolution.
// ---------------------------------------------------------------------------

#[test]
fn resolve_returns_datetime() {
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    \
         DateTime d = datetime.resolve(z, 2024, 3, 10, 2, 30, 0)\n",
    ));
    // Optional trailing fold, positional and keyword.
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    \
         DateTime d = datetime.resolve(z, 2024, 11, 3, 1, 30, 0, 1)\n",
    ));
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    \
         DateTime d = datetime.resolve(z, 2024, 11, 3, 1, 30, 0, fold=1)\n",
    ));
    // A fixed-offset zone resolves too (never a gap/overlap).
    assert_ok(&prog(
        "    auto z = datetime.fixed_offset(-300)\n    \
         auto d = datetime.resolve(z, 2024, 3, 10, 2, 30, 0)\n    \
         auto s = datetime.isoformat(d)\n",
    ));
}

#[test]
fn classifiers_return_bool() {
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    \
         auto d = datetime.resolve(z, 2024, 3, 10, 2, 30, 0)\n    \
         bool b = datetime.is_imaginary(d)\n",
    ));
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    \
         auto d = datetime.resolve(z, 2024, 11, 3, 1, 30, 0)\n    \
         bool b = datetime.is_ambiguous(d)\n",
    ));
}

#[test]
fn resolve_imaginary_returns_datetime() {
    // Result is itself a DateTime, so it flows into the other facets.
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    \
         auto d = datetime.resolve(z, 2024, 3, 10, 2, 30, 0)\n    \
         DateTime r = datetime.resolve_imaginary(d)\n    \
         auto s = datetime.isoformat(r)\n",
    ));
}

#[test]
fn resolve_rejects_bad_operands() {
    // First arg must be a TimeZone.
    assert_code(
        &prog("    auto d = datetime.resolve(42, 2024, 3, 10, 2, 30, 0)\n"),
        "ZAP4024",
    );
    // Civil fields must be ints.
    assert_code(
        &prog(
            "    auto z = datetime.zone(\"UTC\")\n    \
             auto d = datetime.resolve(z, 2024, \"x\", 10, 2, 30, 0)\n",
        ),
        "ZAP4024",
    );
    // fold must be an int.
    assert_code(
        &prog(
            "    auto z = datetime.zone(\"UTC\")\n    \
             auto d = datetime.resolve(z, 2024, 1, 1, 0, 0, 0, \"x\")\n",
        ),
        "ZAP4024",
    );
    // The TimeZone must be positional.
    assert_code(
        &prog(
            "    auto z = datetime.zone(\"UTC\")\n    \
             auto d = datetime.resolve(tz=z, 2024, 1, 1, 0, 0, 0)\n",
        ),
        "ZAP4023",
    );
    // Only `fold` may be a keyword; a stray name is rejected.
    assert_code(
        &prog(
            "    auto z = datetime.zone(\"UTC\")\n    \
             auto d = datetime.resolve(z, 2024, 1, 1, 0, 0, 0, bogus=1)\n",
        ),
        "ZAP4023",
    );
}

#[test]
fn resolve_arity_errors() {
    // Too few (missing civil fields).
    assert_code(
        &prog(
            "    auto z = datetime.zone(\"UTC\")\n    auto d = datetime.resolve(z, 2024, 1, 1)\n",
        ),
        "ZAP4025",
    );
    // Too many (beyond the optional fold).
    assert_code(
        &prog(
            "    auto z = datetime.zone(\"UTC\")\n    \
             auto d = datetime.resolve(z, 2024, 1, 1, 0, 0, 0, 0, 0)\n",
        ),
        "ZAP4025",
    );
}

#[test]
fn classifiers_and_resolve_imaginary_reject_non_datetime() {
    assert_code(&prog("    auto b = datetime.is_ambiguous(5)\n"), "ZAP4024");
    assert_code(
        &prog("    auto b = datetime.is_imaginary(\"x\")\n"),
        "ZAP4024",
    );
    assert_code(
        &prog("    auto r = datetime.resolve_imaginary(5)\n"),
        "ZAP4024",
    );
    // Arity.
    assert_code(
        &prog(
            "    auto t = datetime.from_timestamp(0)\n    auto b = datetime.is_ambiguous(t, t)\n",
        ),
        "ZAP4025",
    );
}

// ---------------------------------------------------------------------------
// zone_abbreviation(dt) -> Optional[str] (#1345).
// ---------------------------------------------------------------------------

#[test]
fn zone_abbreviation_returns_optional_str() {
    // The result is an Optional[str] (the annotated sink forces the check), and
    // it threads into the Optional surface (match / unwrap).
    assert_ok(&prog(
        "    auto z = datetime.zone(\"America/New_York\")\n    \
         auto d = datetime.resolve(z, 2024, 11, 3, 1, 30, 0)\n    \
         Optional[str] a = datetime.zone_abbreviation(d)\n",
    ));
    // A naive value is accepted too — the None case is still Optional[str].
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    \
         Optional[str] a = datetime.zone_abbreviation(t)\n",
    ));
}

#[test]
fn zone_abbreviation_rejects_non_datetime() {
    assert_code(
        &prog("    auto a = datetime.zone_abbreviation(5)\n"),
        "ZAP4024",
    );
    assert_code(
        &prog("    auto a = datetime.zone_abbreviation(\"x\")\n"),
        "ZAP4024",
    );
}

#[test]
fn zone_abbreviation_arity_errors() {
    // No argument, and more than one argument, are both rejected.
    assert_code(
        &prog("    auto a = datetime.zone_abbreviation()\n"),
        "ZAP4025",
    );
    assert_code(
        &prog(
            "    auto t = datetime.from_timestamp(0)\n    \
             auto a = datetime.zone_abbreviation(t, t)\n",
        ),
        "ZAP4025",
    );
}
