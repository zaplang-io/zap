//! `datetime` module — runtime-symbol declarations (ticket #99, D-0056).
//!
//! Civil-time conversions over a Unix timestamp. The six `zap_datetime_<field>
//! (ts, is_local)` getters decompose a timestamp into calendar components (UTC
//! when `is_local == 0`, else local time) and back `datetime.from_timestamp` /
//! `now` / `parse`. `zap_datetime_to_unix(y,mo,d,h,mi,s)` recomposes a UTC
//! timestamp; `zap_datetime_format_iso(...)` renders an ISO-8601 `ZapStr*`;
//! `zap_datetime_parse_iso(ZapStr*)` parses ISO-8601 to a UTC timestamp;
//! `zap_datetime_strptime(ZapStr* s, ZapStr* fmt)` parses `s` against a
//! strftime-style `fmt` to a UTC timestamp — the inverse of `strftime`, and the
//! basis for the AWS SigV4 / iso8601 / rfc822 parse forms (#338);
//! `zap_datetime_now_unix()` is the current wall-clock time in whole seconds.
//! See `zaprt.c`.

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(inkwell::AddressSpace::default());
    // component getters: (ts, is_local) -> field
    for g in [
        "zap_datetime_year",
        "zap_datetime_month",
        "zap_datetime_day",
        "zap_datetime_hour",
        "zap_datetime_minute",
        "zap_datetime_second",
    ] {
        cg.declare_rt(g, &[i.into(), i.into()], Some(i.into()));
    }
    cg.declare_rt("zap_datetime_now_unix", &[], Some(i.into()));
    // (year, month, day, hour, minute, second) -> unix seconds
    let six = [i.into(), i.into(), i.into(), i.into(), i.into(), i.into()];
    cg.declare_rt("zap_datetime_to_unix", &six, Some(i.into()));
    // Field helpers (#974, D-0221): (y,mo,d,h,mi,s) -> int64 weekday / day-of-year.
    cg.declare_rt("zap_datetime_weekday", &six, Some(i.into()));
    cg.declare_rt("zap_datetime_day_of_year", &six, Some(i.into()));
    cg.declare_rt("zap_datetime_format_iso", &six, Some(p.into()));
    // (year, month, day, hour, minute, second, fmt: ZapStr*, offset, is_aware)
    // -> ZapStr* (#286; offset/aware feed %z/%Z, #1045).
    let six_fmt_tz = [
        i.into(),
        i.into(),
        i.into(),
        i.into(),
        i.into(),
        i.into(),
        p.into(),
        i.into(),
        i.into(),
    ];
    cg.declare_rt("zap_datetime_strftime", &six_fmt_tz, Some(p.into()));
    cg.declare_rt("zap_datetime_parse_iso", &[p.into()], Some(i.into()));
    // (str: ZapStr*, fmt: ZapStr*) -> unix seconds (#338)
    cg.declare_rt(
        "zap_datetime_strptime",
        &[p.into(), p.into()],
        Some(i.into()),
    );

    // Timezone database (#1045, D-0225).
    // Host UTC offset (seconds) at a given instant — for the `*_local` paths.
    cg.declare_rt("zap_datetime_local_offset", &[i.into()], Some(i.into()));
    // (y,mo,d,h,mi,s, offset, is_aware) -> RFC-3339 ZapStr* (offset suffix when
    // aware, bare civil ISO when naive).
    let six_and_tz = [
        i.into(),
        i.into(),
        i.into(),
        i.into(),
        i.into(),
        i.into(),
        i.into(),
        i.into(),
    ];
    cg.declare_rt("zap_datetime_isoformat", &six_and_tz, Some(p.into()));
    // RFC-3339 trailing offset (`Z`/`±HH:MM`) of a string, in seconds (0 if none).
    cg.declare_rt("zap_datetime_rfc3339_offset", &[p.into()], Some(i.into()));
    // Validate a named zone against the bundled IANA index; returns the name
    // back, raising a catchable error for an unknown zone (throwing).
    cg.declare_rt("zap_tz_zone_require", &[p.into()], Some(p.into()));
    // Resolve a zone's offset at an instant: (name: ZapStr*, fixed_offset,
    // is_fixed, instant) -> offset seconds. Named zones binary-search the
    // bundled tables; an unknown name raises a catchable error (throwing).
    cg.declare_rt(
        "zap_tz_resolve",
        &[p.into(), i.into(), i.into(), i.into()],
        Some(i.into()),
    );

    // Separate date / time-of-day renderers (#1046, D-0226).
    // (year, month, day) -> "YYYY-MM-DD"; (hour, minute, second) -> "HH:MM:SS".
    let three = [i.into(), i.into(), i.into()];
    cg.declare_rt("zap_date_isoformat", &three, Some(p.into()));
    cg.declare_rt("zap_time_isoformat", &three, Some(p.into()));
}
