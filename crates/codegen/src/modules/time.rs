//! `time` module — runtime-symbol declarations (ticket #99, D-0056).
//!
//! Clocks, sleep, and duration-unit conversions. `zap_time_monotonic_*` read
//! `CLOCK_MONOTONIC`, `zap_time_wall_*` read `CLOCK_REALTIME`; the `*_ns`
//! variants return `int64` nanoseconds and the bare variants `float64` seconds.
//! `zap_time_sleep_seconds`/`_ns` wrap `nanosleep`. The `*_to_ns`/`ns_to_*`
//! helpers convert between a human unit and the `int64`-nanosecond duration
//! representation. See `zaprt.c`.

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let f = cg.ctx.f64_type();
    // clocks
    cg.declare_rt("zap_time_monotonic_ns", &[], Some(i.into()));
    cg.declare_rt("zap_time_monotonic_seconds", &[], Some(f.into()));
    cg.declare_rt("zap_time_wall_ns", &[], Some(i.into()));
    cg.declare_rt("zap_time_wall_seconds", &[], Some(f.into()));
    // sleep
    cg.declare_rt("zap_time_sleep_seconds", &[f.into()], None);
    cg.declare_rt("zap_time_sleep_ns", &[i.into()], None);
    // duration unit constructors (-> nanoseconds)
    cg.declare_rt("zap_time_seconds_to_ns", &[i.into()], Some(i.into()));
    cg.declare_rt("zap_time_millis_to_ns", &[i.into()], Some(i.into()));
    cg.declare_rt("zap_time_micros_to_ns", &[i.into()], Some(i.into()));
    // duration unit converters (nanoseconds -> unit as float)
    cg.declare_rt("zap_time_ns_to_seconds", &[i.into()], Some(f.into()));
    cg.declare_rt("zap_time_ns_to_millis", &[i.into()], Some(f.into()));
    cg.declare_rt("zap_time_ns_to_micros", &[i.into()], Some(f.into()));
}
