//! `datetime` module — runtime-symbol declarations (ticket #99, D-0056).
//!
//! Civil-time conversions over a Unix timestamp. The six `zap_datetime_<field>
//! (ts, is_local)` getters decompose a timestamp into calendar components (UTC
//! when `is_local == 0`, else local time) and back `datetime.from_timestamp` /
//! `now` / `parse`. `zap_datetime_to_unix(y,mo,d,h,mi,s)` recomposes a UTC
//! timestamp; `zap_datetime_format_iso(...)` renders an ISO-8601 `ZapStr*`;
//! `zap_datetime_parse_iso(ZapStr*)` parses ISO-8601 to a UTC timestamp;
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
    cg.declare_rt("zap_datetime_format_iso", &six, Some(p.into()));
    // (year, month, day, hour, minute, second, fmt: ZapStr*) -> ZapStr* (#286)
    let six_and_fmt = [
        i.into(),
        i.into(),
        i.into(),
        i.into(),
        i.into(),
        i.into(),
        p.into(),
    ];
    cg.declare_rt("zap_datetime_strftime", &six_and_fmt, Some(p.into()));
    cg.declare_rt("zap_datetime_parse_iso", &[p.into()], Some(i.into()));
}
