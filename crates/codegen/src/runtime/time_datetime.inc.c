/* zap runtime fragment: time_datetime.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * time & datetime (ticket #99, D-0056). Clocks, sleep, duration-unit helpers,
 * and civil-time conversion. Durations are plain int64 nanoseconds (Go
 * time.Duration style), so arithmetic needs no runtime support; the helpers
 * here just convert between a unit and nanoseconds. Civil time uses the C
 * library's gmtime_r/localtime_r/timegm, so timezone support is UTC + local
 * only (a full tz database is a follow-up, see #99). <time.h>/<unistd.h>/
 * <errno.h> are included near the top of this file.
 * =========================================================================== */

/* --- clocks --- */
int64_t zap_time_monotonic_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}
double zap_time_monotonic_seconds(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}
int64_t zap_time_wall_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}
double zap_time_wall_seconds(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}

/* --- sleep ---
 * This is the plain blocking nanosleep (EINTR-resuming): it blocks the current
 * OS thread, so under the M:N scheduler it pins that thread rather than yielding
 * cooperatively. It is the FALLBACK path used on a native thread (`main`, which
 * is not on an M, so there is nothing to free) or where the cooperative timer is
 * unavailable (non-Linux v1). Every COROUTINE sleep instead takes
 * `zap_time_sleep_cancellable_ns` below, which parks on the fd-less scheduler
 * timer and frees its M (#1357 for in-scope cancellable sleeps, generalized to
 * every coroutine sleep in #1360).
 *
 * Precise GC (#48/#188): the nanosleep is bracketed in zap_enter_blocking/
 * zap_leave_blocking so a sleeping mutator is marked GC-safe (roots flushed to
 * its shadow chain) and the collector need not wait on it to reach a safepoint.
 * Without this, a coroutine asleep in nanosleep stays ACTIVE, so when several
 * coroutines sit in tight time.sleep_ns loops a stop-the-world collection can
 * never catch them all at a safepoint at once and STW stalls indefinitely
 * (gc-immix hang, #188 bug 2). Same discipline as the netpoller (#80), the
 * timed park (#175) and every other blocking primitive. */
void zap_time_sleep_ns(int64_t ns) {
    if (ns <= 0) {
        return;
    }
    struct timespec req;
    req.tv_sec = (time_t)(ns / 1000000000LL);
    req.tv_nsec = (long)(ns % 1000000000LL);
    struct timespec rem;
    zap_enter_blocking();
    while (nanosleep(&req, &rem) != 0 && errno == EINTR) {
        req = rem;
    }
    zap_leave_blocking();
}
void zap_time_sleep_seconds(double s) {
    if (s <= 0.0) {
        return;
    }
    zap_time_sleep_ns((int64_t)(s * 1e9));
}

/* --- cancellable, scheduler-aware sleep (#1357, generalized #1360) ---
 * A coroutine sleep parks on the fd-less scheduler timer (`zap_time_timer_wait`)
 * until the monotonic deadline, which (a) FREES its worker M so sibling
 * coroutines on that worker keep running (unlike the blocking `nanosleep` above,
 * which pins the OS thread for the whole duration), and (b) when the coroutine
 * has an owning `scope:`, observes caller-scope cancellation — a
 * `scope(timeout=)` deadline, a signal, or a sibling failure wakes the wait at
 * once and raises `Cancelled` (surfaced to the caller as `ScopeTimeout` under a
 * timed scope, per #825 sever rules). #1360 widened the park to EVERY coroutine
 * sleep: a sleep with no owning scope now parks and frees its M too (it simply
 * has nothing to cancel), fd-lessly so many concurrent sleepers cannot exhaust
 * file descriptors.
 *
 * The wake rides the poller's epoll tick, so resolution is MILLISECOND-class.
 * The blocking `zap_time_sleep_ns` path stays nanosecond-exact and is the
 * fallback where there is no M to free or no cooperative timer: a native thread
 * (`main`), or a platform without the epoll poller (non-Linux v1). `ns <= 0` on
 * a coroutine is a cooperative checkpoint (observe a pending cancel, then yield
 * once), not a silent no-op, so a tight `sleep(0)` loop both cancels promptly
 * and lets siblings progress. */
void zap_time_sleep_cancellable_ns(int64_t ns) {
    M *m = cur_m;
    G *g = m ? m->current : NULL;
    if (!g) {
        /* Native thread (`main`): not on an M, so there is nothing to free and
         * no scheduler to park on; keep the precise blocking sleep. */
        zap_time_sleep_ns(ns);
        return;
    }
    if (ns <= 0) {
        zap_checkpoint(); /* cancel-or-yield; never a no-op on a coroutine */
        return;
    }
    int64_t now = np_now_ns();
    /* Absolute CLOCK_MONOTONIC deadline; clamp so a very long duration cannot
     * overflow to a past (or negative) deadline and wake early. */
    int64_t deadline = (ns > INT64_MAX - now) ? INT64_MAX : now + ns;
    int64_t st = zap_time_timer_wait(deadline);
    if (st == ZAP_POLL_CANCELLED) {
        zap_raise(cancelled_exc());
    }
    if (st == ZAP_POLL_CLOSED) {
        /* No cooperative timer (non-Linux, or the poller could not start):
         * observe a pending cancel, then take the blocking fallback. */
        zap_check_cancel();
        zap_time_sleep_ns(ns);
    }
    /* ZAP_POLL_TIMEOUT: the deadline passed — normal wake, return. */
}

/* Seconds entry for the cancellable sleep (mirrors `zap_time_sleep_seconds`).
 * A non-positive or non-finite request routes through the ns path so it still
 * acts as a checkpoint inside a scope; a huge request is clamped to the max
 * nanosecond count to avoid undefined `double`->`int64` conversion. */
void zap_time_sleep_cancellable_seconds(double s) {
    if (!(s > 0.0)) { /* <= 0 or NaN */
        zap_time_sleep_cancellable_ns(0);
        return;
    }
    double ns = s * 1e9;
    int64_t ins = (ns >= 9.2e18) ? INT64_MAX : (int64_t)ns;
    zap_time_sleep_cancellable_ns(ins);
}

/* --- duration unit constructors (-> nanoseconds) --- */
int64_t zap_time_seconds_to_ns(int64_t s) { return s * 1000000000LL; }
int64_t zap_time_millis_to_ns(int64_t ms) { return ms * 1000000LL; }
int64_t zap_time_micros_to_ns(int64_t us) { return us * 1000LL; }

/* --- duration unit converters (nanoseconds -> unit as float) --- */
double zap_time_ns_to_seconds(int64_t ns) { return (double)ns / 1e9; }
double zap_time_ns_to_millis(int64_t ns) { return (double)ns / 1e6; }
double zap_time_ns_to_micros(int64_t ns) { return (double)ns / 1e3; }

/* --- civil (calendar) time ---
 * `is_local` selects local time (localtime_r) over UTC (gmtime_r). */
static void zap_civil(int64_t ts, int64_t is_local, struct tm *out) {
    time_t t = (time_t)ts;
    if (is_local) {
        localtime_r(&t, out);
    } else {
        gmtime_r(&t, out);
    }
}
int64_t zap_datetime_year(int64_t ts, int64_t is_local) {
    struct tm tm;
    zap_civil(ts, is_local, &tm);
    return (int64_t)tm.tm_year + 1900;
}
int64_t zap_datetime_month(int64_t ts, int64_t is_local) {
    struct tm tm;
    zap_civil(ts, is_local, &tm);
    return (int64_t)tm.tm_mon + 1;
}
int64_t zap_datetime_day(int64_t ts, int64_t is_local) {
    struct tm tm;
    zap_civil(ts, is_local, &tm);
    return (int64_t)tm.tm_mday;
}
int64_t zap_datetime_hour(int64_t ts, int64_t is_local) {
    struct tm tm;
    zap_civil(ts, is_local, &tm);
    return (int64_t)tm.tm_hour;
}
int64_t zap_datetime_minute(int64_t ts, int64_t is_local) {
    struct tm tm;
    zap_civil(ts, is_local, &tm);
    return (int64_t)tm.tm_min;
}
int64_t zap_datetime_second(int64_t ts, int64_t is_local) {
    struct tm tm;
    zap_civil(ts, is_local, &tm);
    return (int64_t)tm.tm_sec;
}

int64_t zap_datetime_now_unix(void) { return (int64_t)time(NULL); }

/* Civil components (UTC) -> Unix timestamp. Uses timegm (glibc extension; the
 * CI toolchain provides it) so the components are interpreted as UTC. */
int64_t zap_datetime_to_unix(int64_t y, int64_t mo, int64_t d, int64_t h,
                             int64_t mi, int64_t s) {
    struct tm tm;
    memset(&tm, 0, sizeof tm);
    tm.tm_year = (int)(y - 1900);
    tm.tm_mon = (int)(mo - 1);
    tm.tm_mday = (int)d;
    tm.tm_hour = (int)h;
    tm.tm_min = (int)mi;
    tm.tm_sec = (int)s;
    return (int64_t)timegm(&tm);
}

/* --- subsecond carry/borrow (#1148) ---
 * Combine a `DateTime`'s nanoseconds-of-second (`base_nanos`, always in
 * [0,1e9)) with a signed nanosecond delta (a `Duration`, D-0227) into a whole-
 * second carry and a normalized nanoseconds-of-second. The split is FLOORED, so
 * a negative delta borrows correctly (e.g. -1000 ns at .000000000 -> carry -1,
 * nanos 999_999_000); C's `/` and `%` truncate toward zero, which would borrow
 * wrongly, so the floor adjustment is explicit. These are the reason the `add`
 * lowering no longer divides the Duration to whole seconds. */
int64_t zap_datetime_nanos_carry(int64_t base_nanos, int64_t delta_nanos) {
    int64_t total = base_nanos + delta_nanos;
    int64_t q = total / 1000000000LL;
    int64_t r = total % 1000000000LL;
    if (r < 0) {
        q -= 1; /* floor toward negative infinity */
    }
    return q;
}
int64_t zap_datetime_nanos_of_second(int64_t base_nanos, int64_t delta_nanos) {
    int64_t r = (base_nanos + delta_nanos) % 1000000000LL;
    if (r < 0) {
        r += 1000000000LL;
    }
    return r;
}

/* Day-of-week for a civil date (#974, D-0221): Python's `weekday()`, Monday=0 …
 * Sunday=6. The date is recomposed in UTC (`timegm` then `gmtime_r`, the same
 * path `strftime` uses for `%A`/`%j`); C's `tm_wday` is Sunday=0, shifted to
 * Monday=0. Only y/mo/d matter, but all six components are taken so the lowerer
 * can reuse its `DateTime` field read. */
int64_t zap_datetime_weekday(int64_t y, int64_t mo, int64_t d, int64_t h,
                             int64_t mi, int64_t s) {
    (void)h;
    (void)mi;
    (void)s;
    time_t ts = (time_t)zap_datetime_to_unix(y, mo, d, 0, 0, 0);
    struct tm tm;
    gmtime_r(&ts, &tm);
    return (int64_t)((tm.tm_wday + 6) % 7);
}

/* Day of the year for a civil date (#974, D-0221): 1..=366 (Python
 * `timetuple().tm_yday` / `%j`), leap years handled by the libc civil math. */
int64_t zap_datetime_day_of_year(int64_t y, int64_t mo, int64_t d, int64_t h,
                                 int64_t mi, int64_t s) {
    (void)h;
    (void)mi;
    (void)s;
    time_t ts = (time_t)zap_datetime_to_unix(y, mo, d, 0, 0, 0);
    struct tm tm;
    gmtime_r(&ts, &tm);
    return (int64_t)(tm.tm_yday + 1);
}

/* Civil components -> ISO-8601 "YYYY-MM-DDTHH:MM:SS" (returns a fresh ZapStr*). */
void *zap_datetime_format_iso(int64_t y, int64_t mo, int64_t d, int64_t h,
                              int64_t mi, int64_t s) {
    char buf[32];
    int n = snprintf(buf, sizeof buf, "%04lld-%02lld-%02lldT%02lld:%02lld:%02lld",
                     (long long)y, (long long)mo, (long long)d, (long long)h,
                     (long long)mi, (long long)s);
    if (n < 0) {
        n = 0;
    }
    return zap_str_new(buf, (int64_t)n);
}

/* Civil components -> a `strftime`-style custom-formatted string (#286).
 *
 * A Python-faithful subset of `strftime` directives, built **directly from the
 * integer fields** (so it does not depend on run-time string slicing, #284):
 *
 *   %Y  year, >=4 digits, zero-padded   %y  year mod 100, 2 digits
 *   %m  month 01-12                     %d  day of month 01-31
 *   %H  hour 00-23                      %I  hour 01-12 (12-hour clock)
 *   %M  minute 00-59                    %S  second 00-59
 *   %p  AM / PM                         %j  day of year 001-366
 *   %A  weekday name (Sunday)           %a  weekday abbr (Sun)
 *   %B  month name (January)            %b  month abbr (Jan)
 *   %Z  time-zone name                  %z  UTC offset (+HHMM)
 *   %%  a literal percent
 *
 * `DateTime` is naive (it carries no zone), so — like Python's `strftime` on a
 * naive datetime — `%Z` and `%z` expand to the empty string. An unknown `%X`
 * is emitted verbatim (`%` then `X`), matching CPython. Returns a fresh
 * `ZapStr*`. Weekday / day-of-year are derived by recomposing the instant in
 * UTC (`timegm` then `gmtime_r`), reusing the civil-time path above. */
void *zap_datetime_strftime(int64_t y, int64_t mo, int64_t d, int64_t h,
                            int64_t mi, int64_t s, void *fmt, int64_t offset,
                            int64_t is_aware) {
    static const char *const MON_FULL[12] = {
        "January", "February", "March",     "April",   "May",      "June",
        "July",    "August",   "September", "October", "November", "December"};
    static const char *const MON_ABBR[12] = {"Jan", "Feb", "Mar", "Apr",
                                             "May", "Jun", "Jul", "Aug",
                                             "Sep", "Oct", "Nov", "Dec"};
    static const char *const DAY_FULL[7] = {"Sunday",   "Monday", "Tuesday",
                                            "Wednesday", "Thursday",
                                            "Friday",   "Saturday"};
    static const char *const DAY_ABBR[7] = {"Sun", "Mon", "Tue", "Wed",
                                            "Thu", "Fri", "Sat"};
    ZapStr *f = (ZapStr *)fmt;
    int64_t flen = f->len < 0 ? 0 : f->len;

    /* Derive weekday (0=Sunday) and 1-based day-of-year by recomposing the
     * civil components into a UTC instant and splitting them back out. */
    struct tm tm;
    memset(&tm, 0, sizeof tm);
    tm.tm_year = (int)(y - 1900);
    tm.tm_mon = (int)(mo - 1);
    tm.tm_mday = (int)d;
    tm.tm_hour = (int)h;
    tm.tm_min = (int)mi;
    tm.tm_sec = (int)s;
    time_t t = timegm(&tm);
    struct tm norm;
    gmtime_r(&t, &norm);
    int wday = norm.tm_wday; /* 0=Sunday .. 6=Saturday */
    int yday = norm.tm_yday + 1;

    /* Upper bound: the widest single directive ("Wednesday"/"September") is 9
     * bytes for 2 input bytes; literal bytes map 1:1. `flen*9 + 1` is safe. */
    size_t cap = (size_t)flen * 9 + 1;
    char *out = (char *)malloc(cap);
    if (!out) {
        return zap_str_new("", 0);
    }
    size_t n = 0;
    for (int64_t i = 0; i < flen; i++) {
        char c = f->data[i];
        if (c != '%' || i + 1 >= flen) {
            out[n++] = c;
            continue;
        }
        char dir = f->data[++i];
        char tmp[16];
        int w;
        switch (dir) {
        case 'Y':
            w = snprintf(tmp, sizeof tmp, "%04lld", (long long)y);
            memcpy(out + n, tmp, (size_t)w);
            n += (size_t)w;
            break;
        case 'y':
            w = snprintf(tmp, sizeof tmp, "%02lld", (long long)(((y % 100) + 100) % 100));
            memcpy(out + n, tmp, (size_t)w);
            n += (size_t)w;
            break;
        case 'm':
            w = snprintf(tmp, sizeof tmp, "%02lld", (long long)mo);
            memcpy(out + n, tmp, (size_t)w);
            n += (size_t)w;
            break;
        case 'd':
            w = snprintf(tmp, sizeof tmp, "%02lld", (long long)d);
            memcpy(out + n, tmp, (size_t)w);
            n += (size_t)w;
            break;
        case 'H':
            w = snprintf(tmp, sizeof tmp, "%02lld", (long long)h);
            memcpy(out + n, tmp, (size_t)w);
            n += (size_t)w;
            break;
        case 'I': {
            long long h12 = (long long)(h % 12);
            if (h12 == 0) {
                h12 = 12;
            }
            w = snprintf(tmp, sizeof tmp, "%02lld", h12);
            memcpy(out + n, tmp, (size_t)w);
            n += (size_t)w;
            break;
        }
        case 'M':
            w = snprintf(tmp, sizeof tmp, "%02lld", (long long)mi);
            memcpy(out + n, tmp, (size_t)w);
            n += (size_t)w;
            break;
        case 'S':
            w = snprintf(tmp, sizeof tmp, "%02lld", (long long)s);
            memcpy(out + n, tmp, (size_t)w);
            n += (size_t)w;
            break;
        case 'j':
            w = snprintf(tmp, sizeof tmp, "%03d", yday);
            memcpy(out + n, tmp, (size_t)w);
            n += (size_t)w;
            break;
        case 'p': {
            const char *ampm = (h < 12) ? "AM" : "PM";
            memcpy(out + n, ampm, 2);
            n += 2;
            break;
        }
        case 'A': {
            const char *nm = DAY_FULL[(wday % 7 + 7) % 7];
            size_t l = strlen(nm);
            memcpy(out + n, nm, l);
            n += l;
            break;
        }
        case 'a': {
            const char *nm = DAY_ABBR[(wday % 7 + 7) % 7];
            size_t l = strlen(nm);
            memcpy(out + n, nm, l);
            n += l;
            break;
        }
        case 'B': {
            const char *nm = MON_FULL[((mo - 1) % 12 + 12) % 12];
            size_t l = strlen(nm);
            memcpy(out + n, nm, l);
            n += l;
            break;
        }
        case 'b': {
            const char *nm = MON_ABBR[((mo - 1) % 12 + 12) % 12];
            size_t l = strlen(nm);
            memcpy(out + n, nm, l);
            n += l;
            break;
        }
        case 'z':
            /* Aware: numeric UTC offset `±HHMM` (Python's `%z`, no colon), with
             * trailing `SS` when the offset is not a whole minute — a historical
             * LMT like +05:53:28 -> `+055328` (#1055, D-0228). Naive: empty. */
            if (is_aware) {
                long long a = offset < 0 ? -offset : offset;
                int hh = (int)(a / 3600), mm = (int)((a % 3600) / 60),
                    ss = (int)(a % 60);
                char sign = offset < 0 ? '-' : '+';
                if (ss != 0) {
                    w = snprintf(tmp, sizeof tmp, "%c%02d%02d%02d", sign, hh, mm,
                                 ss);
                } else {
                    w = snprintf(tmp, sizeof tmp, "%c%02d%02d", sign, hh, mm);
                }
                memcpy(out + n, tmp, (size_t)w);
                n += (size_t)w;
            }
            break;
        case 'Z':
            /* Aware: `UTC±HH:MM` (no zone abbreviation is carried on the value),
             * with `:SS` for a sub-minute offset (#1055). Naive: empty. */
            if (is_aware) {
                long long a = offset < 0 ? -offset : offset;
                int hh = (int)(a / 3600), mm = (int)((a % 3600) / 60),
                    ss = (int)(a % 60);
                char sign = offset < 0 ? '-' : '+';
                if (ss != 0) {
                    w = snprintf(tmp, sizeof tmp, "UTC%c%02d:%02d:%02d", sign, hh,
                                 mm, ss);
                } else {
                    w = snprintf(tmp, sizeof tmp, "UTC%c%02d:%02d", sign, hh, mm);
                }
                memcpy(out + n, tmp, (size_t)w);
                n += (size_t)w;
            }
            break;
        case '%':
            out[n++] = '%';
            break;
        default:
            /* Unknown directive: emit verbatim, like CPython. */
            out[n++] = '%';
            out[n++] = dir;
            break;
        }
    }
    void *z = zap_str_new(out, (int64_t)n);
    free(out);
    return z;
}

/* ISO-8601 string -> Unix timestamp (UTC). Accepts a date ("YYYY-MM-DD") or a
 * full date-time with a 'T' or ' ' separator; fields left unmatched default to
 * the start of the day. v1 parsing is lenient (sscanf): malformed input yields
 * a best-effort timestamp rather than an error (strict parsing is a follow-up,
 * #99). */
int64_t zap_datetime_parse_iso(void *s) {
    ZapStr *z = (ZapStr *)s;
    char buf[64];
    int64_t n = z->len;
    if (n < 0) {
        n = 0;
    }
    if (n > 63) {
        n = 63;
    }
    memcpy(buf, z->data, (size_t)n);
    buf[n] = '\0';
    int y = 1970, mo = 1, d = 1, h = 0, mi = 0, sec = 0;
    sscanf(buf, "%d-%d-%d%*c%d:%d:%d", &y, &mo, &d, &h, &mi, &sec);
    return zap_datetime_to_unix(y, mo, d, h, mi, sec);
}

/* The fractional-second component of an ISO/RFC-3339 string, in nanoseconds-of-
 * second (#1148). Scans the time portion (after the `T`/space separator) for a
 * `.` or `,` fraction that follows the seconds, before any offset. Accepts a
 * GENERAL variable-length fraction of 1-9 digits, right-padded to nanoseconds
 * (`.5` -> 500_000_000, `.000001` -> 1000); digits past the 9th are consumed but
 * truncated. 0 when no fraction is present. Lenient, matching `parse`'s v1
 * contract. The companion to `zap_datetime_parse_iso` (whole seconds). */
int64_t zap_datetime_parse_nanos(void *s) {
    ZapStr *z = (ZapStr *)s;
    int64_t len = z->len < 0 ? 0 : z->len;
    const char *p = z->data;
    int64_t start = 0;
    for (int64_t i = 0; i < len; i++) {
        char c = p[i];
        if (c == 'T' || c == 't' || c == ' ') {
            start = i + 1;
            break;
        }
    }
    int64_t i = start;
    int found = 0;
    for (; i < len; i++) {
        char c = p[i];
        if (c == '.' || c == ',') {
            i++;
            found = 1;
            break;
        }
        /* An offset sign or `Z` before any `.`/`,` means no fraction. */
        if (c == 'Z' || c == 'z' || c == '+' || c == '-') {
            return 0;
        }
    }
    if (!found || i >= len) {
        return 0;
    }
    int64_t v = 0;
    int count = 0;
    while (i < len && p[i] >= '0' && p[i] <= '9') {
        if (count < 9) {
            v = v * 10 + (p[i] - '0');
            count++;
        }
        i++;
    }
    if (count == 0) {
        return 0;
    }
    for (int k = count; k < 9; k++) {
        v *= 10;
    }
    return v;
}

/* --- strptime support (#338) ---
 * Read up to `maxw` consecutive ASCII digits from `p[*pos..plen]`, advancing
 * `*pos`; returns the parsed value (0 if none). Bounding the width lets packed
 * forms such as "%Y%m%d" over "20260101" split without a separator. */
static int64_t zap_read_digits(const char *p, int64_t plen, int64_t *pos,
                               int maxw) {
    int64_t v = 0;
    int count = 0;
    while (*pos < plen && count < maxw && p[*pos] >= '0' && p[*pos] <= '9') {
        v = v * 10 + (p[*pos] - '0');
        (*pos)++;
        count++;
    }
    return v;
}

/* If `word` occurs at `p[pos..]` (ASCII case-insensitive), return its length,
 * else 0. Used for the English weekday/month name directives so parsing stays
 * locale-free and deterministic. */
static int zap_ci_prefix(const char *p, int64_t plen, int64_t pos,
                         const char *word) {
    int64_t wl = (int64_t)strlen(word);
    if (pos + wl > plen) {
        return 0;
    }
    for (int64_t k = 0; k < wl; k++) {
        char a = p[pos + k];
        char b = word[k];
        if (a >= 'A' && a <= 'Z') {
            a = (char)(a - 'A' + 'a');
        }
        if (b >= 'A' && b <= 'Z') {
            b = (char)(b - 'A' + 'a');
        }
        if (a != b) {
            return 0;
        }
    }
    return (int)wl;
}

/* Parse `s` against a strftime-style `fmt` into a UTC Unix timestamp (#338).
 *
 * This is the inverse of `zap_datetime_strftime`: it recognises the same
 * Python-faithful directive subset and is likewise deterministic and
 * locale-free (%A/%a/%B/%b match our own English tables, never the C locale):
 *
 *   %Y year (<=4 digits)   %y year mod 100 (-> 2000+yy)   %j day-of-year (skip)
 *   %m %d %H %M %S  numeric fields (<=2 digits each, padding optional)
 *   %I 12-hour clock + %p AM/PM     %B/%b month name -> month
 *   %A/%a weekday name (consumed, ignored)
 *   %z UTC offset (Z or [+-]HH[:]MM, consumed) %Z zone name (consumed)
 *   %%  a literal percent
 *
 * `DateTime` is naive UTC, so the zone directives are read and discarded.
 * Whitespace in the format matches a run of input whitespace; any other literal
 * byte must match the input. Parsing is lenient — matching `parse`'s v1
 * contract (#99) — so on a mismatch it stops and recomposes a best-effort
 * timestamp from the fields read so far. Returns Unix seconds (UTC). */
int64_t zap_datetime_strptime(void *s, void *fmt) {
    static const char *const MON_FULL[12] = {
        "January", "February", "March",     "April",   "May",      "June",
        "July",    "August",   "September", "October", "November", "December"};
    static const char *const MON_ABBR[12] = {"Jan", "Feb", "Mar", "Apr",
                                             "May", "Jun", "Jul", "Aug",
                                             "Sep", "Oct", "Nov", "Dec"};
    static const char *const DAY_FULL[7] = {
        "Sunday",   "Monday", "Tuesday",  "Wednesday",
        "Thursday", "Friday", "Saturday"};
    static const char *const DAY_ABBR[7] = {"Sun", "Mon", "Tue", "Wed",
                                            "Thu", "Fri", "Sat"};
    ZapStr *zs = (ZapStr *)s;
    ZapStr *zf = (ZapStr *)fmt;
    const char *in = zs->data;
    int64_t ilen = zs->len < 0 ? 0 : zs->len;
    const char *f = zf->data;
    int64_t flen = zf->len < 0 ? 0 : zf->len;
    int64_t ip = 0;
    int64_t fp = 0;
    int y = 1970, mo = 1, d = 1, h = 0, mi = 0, sec = 0;
    int hour12 = -1; /* raw %I value, if a 12-hour clock was parsed */
    int pm = -1;     /* -1 unknown, 0 AM, 1 PM */

    while (fp < flen) {
        char fc = f[fp];
        if (fc == '%' && fp + 1 < flen) {
            char dir = f[fp + 1];
            fp += 2;
            switch (dir) {
            case 'Y':
                y = (int)zap_read_digits(in, ilen, &ip, 4);
                break;
            case 'y':
                y = 2000 + (int)zap_read_digits(in, ilen, &ip, 2);
                break;
            case 'm':
                mo = (int)zap_read_digits(in, ilen, &ip, 2);
                break;
            case 'd':
                d = (int)zap_read_digits(in, ilen, &ip, 2);
                break;
            case 'H':
                h = (int)zap_read_digits(in, ilen, &ip, 2);
                break;
            case 'I':
                hour12 = (int)zap_read_digits(in, ilen, &ip, 2);
                break;
            case 'M':
                mi = (int)zap_read_digits(in, ilen, &ip, 2);
                break;
            case 'S':
                sec = (int)zap_read_digits(in, ilen, &ip, 2);
                break;
            case 'j':
                (void)zap_read_digits(in, ilen, &ip, 3); /* redundant */
                break;
            case 'p':
                if (zap_ci_prefix(in, ilen, ip, "AM")) {
                    pm = 0;
                    ip += 2;
                } else if (zap_ci_prefix(in, ilen, ip, "PM")) {
                    pm = 1;
                    ip += 2;
                }
                break;
            case 'B':
            case 'b': {
                int matched = 0;
                for (int k = 0; k < 12; k++) {
                    int l = zap_ci_prefix(in, ilen, ip, MON_FULL[k]);
                    if (!l) {
                        l = zap_ci_prefix(in, ilen, ip, MON_ABBR[k]);
                    }
                    if (l) {
                        mo = k + 1;
                        ip += l;
                        matched = 1;
                        break;
                    }
                }
                if (!matched) {
                    goto done;
                }
                break;
            }
            case 'A':
            case 'a': {
                int matched = 0;
                for (int k = 0; k < 7; k++) {
                    int l = zap_ci_prefix(in, ilen, ip, DAY_FULL[k]);
                    if (!l) {
                        l = zap_ci_prefix(in, ilen, ip, DAY_ABBR[k]);
                    }
                    if (l) {
                        ip += l;
                        matched = 1;
                        break;
                    }
                }
                if (!matched) {
                    goto done;
                }
                break;
            }
            case 'z':
                if (ip < ilen && (in[ip] == 'Z' || in[ip] == 'z')) {
                    ip++;
                } else if (ip < ilen && (in[ip] == '+' || in[ip] == '-')) {
                    ip++;
                    (void)zap_read_digits(in, ilen, &ip, 2);
                    if (ip < ilen && in[ip] == ':') {
                        ip++;
                    }
                    (void)zap_read_digits(in, ilen, &ip, 2);
                }
                break;
            case 'Z':
                while (ip < ilen && ((in[ip] >= 'A' && in[ip] <= 'Z') ||
                                     (in[ip] >= 'a' && in[ip] <= 'z'))) {
                    ip++;
                }
                break;
            case '%':
                if (ip < ilen && in[ip] == '%') {
                    ip++;
                } else {
                    goto done;
                }
                break;
            default:
                /* Unknown directive: mirror strftime, which emits "%X". */
                if (ip < ilen && in[ip] == '%') {
                    ip++;
                }
                if (ip < ilen && in[ip] == dir) {
                    ip++;
                }
                break;
            }
        } else if (fc == ' ' || fc == '\t' || fc == '\n') {
            fp++;
            while (ip < ilen &&
                   (in[ip] == ' ' || in[ip] == '\t' || in[ip] == '\n')) {
                ip++;
            }
        } else {
            if (ip < ilen && in[ip] == fc) {
                ip++;
                fp++;
            } else {
                goto done;
            }
        }
    }
done:
    if (hour12 >= 0) {
        h = hour12 % 12;
        if (pm == 1) {
            h += 12;
        }
    }
    return zap_datetime_to_unix(y, mo, d, h, mi, sec);
}


/* ===========================================================================
 * Timezone database (#1045, D-0225).
 *
 * Offsets are a pure function of (zone, UTC instant): there is NO "zone default
 * offset", which is exactly what avoids the classic pytz footgun of surfacing a
 * zone's first/LMT offset for a modern date. The bundled IANA tables live in
 * `tzdata_generated.inc.c` (included by `zaprt.c` immediately before this
 * fragment): `ZAP_TZ_ZONES[]` is sorted by name; each zone is a flat list of
 * `(at, off)` intervals with `trans[0].at == INT64_MIN`, so a binary search for
 * the last interval whose `at <= instant` always resolves. All TZif/POSIX-rule
 * parsing was done offline by `tools/gen_tzdata.py`; the runtime only searches.
 * ===========================================================================*/

/* Host UTC offset (seconds east of UTC) at `ts`, computed portably as
 * `timegm(localtime(ts)) - ts` (no reliance on the `tm_gmtoff` extension). Used
 * by the `*_local` constructors to make a host-local `DateTime` zone-aware. */
int64_t zap_datetime_local_offset(int64_t ts) {
    time_t t = (time_t)ts;
    struct tm lt;
    localtime_r(&t, &lt);
    return (int64_t)(timegm(&lt) - t);
}

/* Civil components + offset/aware -> RFC-3339 string. Aware values carry a
 * `±HH:MM` suffix (`+00:00` for a zero offset); a naive value is the bare civil
 * ISO form, byte-identical to `zap_datetime_format_iso`. Sub-minute offsets
 * (a historical LMT) render to whole minutes. */
void *zap_datetime_isoformat(int64_t y, int64_t mo, int64_t d, int64_t h,
                             int64_t mi, int64_t s, int64_t offset,
                             int64_t is_aware, int64_t nanos) {
    char buf[64];
    int n = snprintf(buf, sizeof buf,
                     "%04lld-%02lld-%02lldT%02lld:%02lld:%02lld", (long long)y,
                     (long long)mo, (long long)d, (long long)h, (long long)mi,
                     (long long)s);
    if (n < 0) {
        n = 0;
    }
    /* Fractional second (#1148), before the offset suffix. Policy: emitted iff
     * non-zero; a whole number of microseconds renders as 6 digits (exactly
     * Python `isoformat`, trailing zeros included — `.250000`, `.000001`), finer
     * nanosecond precision renders as 9 digits (`.123456789`). A zero fraction is
     * omitted (Python). RFC-3339 accepts this variable-length fraction. */
    if (nanos > 0 && nanos < 1000000000LL) {
        int m;
        if (nanos % 1000 == 0) {
            m = snprintf(buf + n, sizeof buf - (size_t)n, ".%06lld",
                         (long long)(nanos / 1000));
        } else {
            m = snprintf(buf + n, sizeof buf - (size_t)n, ".%09lld",
                         (long long)nanos);
        }
        if (m > 0) {
            n += m;
        }
    }
    if (is_aware) {
        long long a = offset < 0 ? -offset : offset;
        int hh = (int)(a / 3600), mm = (int)((a % 3600) / 60), ss = (int)(a % 60);
        char sign = offset < 0 ? '-' : '+';
        int m;
        if (ss != 0) {
            /* Sub-minute offset (a historical LMT): RFC-3339 `±HH:MM:SS` (#1055). */
            m = snprintf(buf + n, sizeof buf - (size_t)n, "%c%02d:%02d:%02d", sign,
                         hh, mm, ss);
        } else {
            m = snprintf(buf + n, sizeof buf - (size_t)n, "%c%02d:%02d", sign, hh,
                         mm);
        }
        if (m > 0) {
            n += m;
        }
    }
    return zap_str_new(buf, (int64_t)n);
}

/* The trailing RFC-3339 zone of `s` (`Z`, `±HH:MM`, or `±HHMM`) in seconds east
 * of UTC; 0 for `Z` or when no offset is present. Scans only the time portion
 * (after the `T`/space separator) so the date's hyphens are never mistaken for
 * an offset sign. Lenient, matching the `parse` family's v1 contract. */
int64_t zap_datetime_rfc3339_offset(void *s) {
    ZapStr *z = (ZapStr *)s;
    int64_t len = z->len < 0 ? 0 : z->len;
    const char *p = z->data;
    int64_t start = 0;
    for (int64_t i = 0; i < len; i++) {
        char c = p[i];
        if (c == 'T' || c == 't' || c == ' ') {
            start = i + 1;
            break;
        }
    }
    for (int64_t i = start; i < len; i++) {
        char c = p[i];
        if (c == 'Z' || c == 'z') {
            return 0;
        }
        if (c == '+' || c == '-') {
            int sign = c == '-' ? -1 : 1;
            int64_t j = i + 1;
            int hh = 0, mm = 0, cnt = 0;
            while (j < len && p[j] >= '0' && p[j] <= '9' && cnt < 2) {
                hh = hh * 10 + (p[j] - '0');
                j++;
                cnt++;
            }
            if (j < len && p[j] == ':') {
                j++;
            }
            cnt = 0;
            while (j < len && p[j] >= '0' && p[j] <= '9' && cnt < 2) {
                mm = mm * 10 + (p[j] - '0');
                j++;
                cnt++;
            }
            return (int64_t)sign * (hh * 3600 + mm * 60);
        }
    }
    return 0;
}

/* Binary search the name-sorted zone index; NULL if the zone is not bundled. */
static const zap_tz_zone *zap_tz_find_zone(const char *name) {
    int lo = 0, hi = ZAP_TZ_NZONES - 1;
    while (lo <= hi) {
        int mid = (lo + hi) / 2;
        int c = strcmp(name, ZAP_TZ_ZONES[mid].name);
        if (c == 0) {
            return &ZAP_TZ_ZONES[mid];
        }
        if (c < 0) {
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
    }
    return NULL;
}

/* The zone's UTC offset (seconds) at `inst`: the `off` of the last interval
 * whose `at <= inst`. `trans[0].at == INT64_MIN`, so this always resolves (an
 * instant before the first real transition takes the LMT interval). */
static int64_t zap_tz_offset_for(const zap_tz_zone *z, int64_t inst) {
    int lo = 0, hi = z->n - 1, ans = 0;
    while (lo <= hi) {
        int mid = (lo + hi) / 2;
        if (z->trans[mid].at <= inst) {
            ans = mid;
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    return (int64_t)z->trans[ans].off;
}

/* Copy a `ZapStr*` zone name into a NUL-terminated C buffer, validating the
 * WHOLE length-delimited string (#1172). Returns 1 when the name is usable as a
 * bundled-zone key, 0 when it is not: an embedded NUL (which would let the
 * downstream `strcmp` silently match only the truncated prefix) or a length that
 * overflows the buffer (which would truncate to a possibly-valid zone). A real
 * IANA key carries no NUL and is far shorter than `cap`, so a rejected name is
 * never a bundled zone — callers treat 0 as "unknown zone". `buf` is still
 * filled with a bounded, best-effort copy for the diagnostic message. */
static int zap_tz_name_cstr(void *name, char *buf, size_t cap) {
    ZapStr *z = (ZapStr *)name;
    int64_t len = z->len < 0 ? 0 : z->len;
    int ok = 1;
    if ((size_t)len > cap - 1) {
        len = (int64_t)(cap - 1);
        ok = 0; /* overlong: must not truncate to a valid zone */
    }
    for (int64_t i = 0; i < len; i++) {
        if (z->data[i] == '\0') {
            ok = 0; /* embedded NUL: the full string is not a bundled key */
            break;
        }
    }
    memcpy(buf, z->data, (size_t)len);
    buf[len] = '\0';
    return ok;
}

/* Resolve a zone's offset at `inst`. A fixed zone returns its constant offset;
 * a named zone looks the instant up in the bundled table. An unknown named zone
 * — including one with an embedded NUL or an overlong name (#1172) — raises a
 * catchable error (Python `ZoneInfoNotFoundError`). */
int64_t zap_tz_resolve(void *name, int64_t fixed_off, int64_t is_fixed,
                       int64_t inst) {
    if (is_fixed) {
        return fixed_off;
    }
    char buf[128];
    int ok = zap_tz_name_cstr(name, buf, sizeof buf);
    const zap_tz_zone *zn = ok ? zap_tz_find_zone(buf) : NULL;
    if (!zn) {
        char msg[192];
        int m = snprintf(msg, sizeof msg, "unknown time zone: %s", buf);
        if (m < 0) {
            m = 0;
        }
        zap_panic(zap_str_new(msg, (int64_t)m));
        return 0; /* unreachable: zap_panic does not return */
    }
    return zap_tz_offset_for(zn, inst);
}

/* Validate a named zone against the bundled index, returning the name back so
 * `zone(name)` can both check and carry it; raises for an unknown zone,
 * including an embedded-NUL or overlong name (#1172). */
void *zap_tz_zone_require(void *name) {
    char buf[128];
    int ok = zap_tz_name_cstr(name, buf, sizeof buf);
    if (!ok || !zap_tz_find_zone(buf)) {
        char msg[192];
        int m = snprintf(msg, sizeof msg, "unknown time zone: %s", buf);
        if (m < 0) {
            m = 0;
        }
        zap_panic(zap_str_new(msg, (int64_t)m));
    }
    return name;
}

/* The host system's configured local zone as a bundled IANA name (#1171), or
 * NULL if it cannot be resolved to a bundled key. Unix sources, tried in order:
 *   1. the `TZ` environment variable (a leading ':' stripped);
 *   2. the `/etc/localtime` symlink target (the suffix after the last
 *      "zoneinfo/");
 *   3. `/etc/timezone`'s first trimmed line (Debian/Ubuntu).
 * Each candidate must be present in the bundled table, so a POSIX rule string
 * (e.g. "EST5EDT") or a non-bundled zone falls through. On a match the name is
 * copied into `buf` (stable storage the caller can read); the returned pointer
 * is either into `buf` or into the environment (copied immediately by the
 * caller). */
static const char *zap_tz_local_name_lookup(char *buf, size_t cap) {
    /* 1. TZ — portable (all platforms). */
    const char *tz = getenv("TZ");
    if (tz && tz[0] != '\0') {
        if (tz[0] == ':') {
            tz++;
        }
        if (tz[0] != '\0' && zap_tz_find_zone(tz)) {
            return tz;
        }
    }
#ifndef _WIN32
    /* 2. /etc/localtime symlink (POSIX: `readlink` is not available on Windows,
     * which also has no zoneinfo symlink; there TZ above is the only source). */
    ssize_t rl = readlink("/etc/localtime", buf, cap - 1);
    if (rl > 0) {
        buf[rl] = '\0';
        const char *marker = "zoneinfo/";
        char *z = strstr(buf, marker);
        if (z) {
            const char *name = z + strlen(marker);
            if (name[0] != '\0' && zap_tz_find_zone(name)) {
                return name;
            }
        }
    }
    /* 3. /etc/timezone (Debian/Ubuntu). */
    FILE *f = fopen("/etc/timezone", "r");
    if (f) {
        char line[128];
        char *got = fgets(line, sizeof line, f);
        fclose(f);
        if (got) {
            size_t l = strlen(line);
            while (l > 0 && (line[l - 1] == '\n' || line[l - 1] == '\r' ||
                             line[l - 1] == ' ' || line[l - 1] == '\t')) {
                line[--l] = '\0';
            }
            if (l > 0 && l < cap && zap_tz_find_zone(line)) {
                memcpy(buf, line, l + 1);
                return buf;
            }
        }
    }
#else
    (void)buf;
    (void)cap;
#endif
    return NULL;
}

/* `datetime.local_zone()` backing (#1171): the host local zone's bundled IANA
 * name as a fresh ZapStr*, raising the same catchable error family as an unknown
 * `zone()` when the host zone cannot be resolved to a bundled key. */
void *zap_datetime_local_zone_name(void) {
    char buf[256];
    const char *name = zap_tz_local_name_lookup(buf, sizeof buf);
    if (!name) {
        const char *msg = "cannot determine system local zone";
        zap_panic(zap_str_new(msg, (int64_t)strlen(msg)));
        return NULL; /* unreachable: zap_panic does not return */
    }
    return zap_str_new(name, (int64_t)strlen(name));
}

/* ===========================================================================
 * Named-zone retention + civil-time resolution (#1149).
 *
 * A `DateTime` retains its IANA zone as an `int64 zone_id`: `-1` for a naive or
 * fixed-offset value, else an index into the name-sorted `ZAP_TZ_ZONES[]`. The
 * resolution helpers derive gap / unique / overlap entirely from the already
 * bundled `(at, off)` intervals (no separate DST flag, no table regeneration):
 * a wall time `w = timegm(civil)` is realised by interval `i` iff
 * `trans[i].at <= w - trans[i].off < trans[i+1].at`. Counting the matches gives
 * 0 = gap (imaginary), 1 = unique, 2 = overlap (ambiguous); the two overlap
 * instants are `w - off` for each match. Tables are small, so a linear scan is
 * used for exactness (this is not a hot path).
 * ===========================================================================*/

/* The bundled zone at index `id`, or NULL if out of range (id < 0 = none). */
static const zap_tz_zone *zap_tz_zone_at(int64_t id) {
    if (id < 0 || id >= ZAP_TZ_NZONES) {
        return NULL;
    }
    return &ZAP_TZ_ZONES[id];
}

/* The `zone_id` handle for a `TimeZone`: a fixed zone (or an empty/unknown name)
 * is -1; a bundled named zone is its index in the sorted table. Non-raising —
 * the name was validated by `zone()`; an unexpected miss simply yields -1. */
int64_t zap_tz_zone_id(void *name, int64_t is_fixed) {
    if (is_fixed) {
        return -1;
    }
    char buf[128];
    /* An embedded-NUL / overlong name is not a bundled key (#1172) -> none. */
    if (!zap_tz_name_cstr(name, buf, sizeof buf)) {
        return -1;
    }
    if (buf[0] == '\0') {
        return -1;
    }
    int lo = 0, hi = ZAP_TZ_NZONES - 1;
    while (lo <= hi) {
        int mid = (lo + hi) / 2;
        int c = strcmp(buf, ZAP_TZ_ZONES[mid].name);
        if (c == 0) {
            return (int64_t)mid;
        }
        if (c < 0) {
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
    }
    return -1;
}

/* A zone's offset at UTC instant `inst` by zone id. A naive / fixed value
 * (id < 0) returns `fixed_off` unchanged — so instant arithmetic on such values
 * is byte-for-byte the pre-#1149 math. */
int64_t zap_tz_offset_at_id(int64_t zone_id, int64_t inst, int64_t fixed_off) {
    const zap_tz_zone *z = zap_tz_zone_at(zone_id);
    if (!z) {
        return fixed_off;
    }
    return zap_tz_offset_for(z, inst);
}

/* The IANA abbreviation of zone `zone_id` at UTC instant `inst`, as a fresh
 * `ZapStr*` — the `abbr` of the last interval whose `at <= inst` (the same
 * binary search `zap_tz_offset_for` uses). Returns NULL when the value carries
 * no named zone (`zone_id < 0`: naive, fixed-offset, or unknown); the caller
 * (`datetime.zone_abbreviation`) maps NULL to `Optional.None` and a non-NULL
 * pointer to `Optional.Some(str)` via the niche layout, so no abbreviation is
 * ever derived from an offset alone. Faithful to the bundled tz database,
 * including numeric forms (`-04`, `+0530`) and `LMT` for pre-standard intervals
 * (CPython `zoneinfo.tzname()` parity). Non-raising; not a hot path (#1345). */
void *zap_tz_abbr_at_id(int64_t zone_id, int64_t inst) {
    const zap_tz_zone *z = zap_tz_zone_at(zone_id);
    if (!z) {
        return NULL;
    }
    int lo = 0, hi = z->n - 1, ans = 0;
    while (lo <= hi) {
        int mid = (lo + hi) / 2;
        if (z->trans[mid].at <= inst) {
            ans = mid;
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    const char *abbr = ZAP_TZ_ABBRS[z->trans[ans].abbr];
    return zap_str_new(abbr, (int64_t)strlen(abbr));
}

/* Count the intervals that realise wall time `w`, recording the first match's
 * offset in `*off0` (the earlier instant / larger offset) and a second, if any,
 * in `*off1` (the later instant / smaller offset). Returns 0, 1 or 2. */
static int zap_tz_wall_candidates(const zap_tz_zone *z, int64_t w, int64_t *off0,
                                  int64_t *off1) {
    int count = 0;
    for (int i = 0; i < z->n; i++) {
        int64_t lo = z->trans[i].at;
        int64_t hi = (i + 1 < z->n) ? z->trans[i + 1].at : INT64_MAX;
        int64_t off = z->trans[i].off;
        int64_t inst = w - off;
        if (inst >= lo && inst < hi) {
            if (count == 0) {
                *off0 = off;
            } else {
                *off1 = off;
            }
            count++;
            if (count == 2) {
                break;
            }
        }
    }
    return count;
}

/* For a wall time `w` with no matching interval (a spring-forward gap), find the
 * surrounding offsets: `*ob` just before the transition, `*oa` just after
 * (`oa > ob`). Returns 1 on success, 0 if `w` is not actually in a gap. */
static int zap_tz_gap_offsets(const zap_tz_zone *z, int64_t w, int64_t *ob,
                              int64_t *oa) {
    for (int i = 1; i < z->n; i++) {
        int64_t t = z->trans[i].at;
        int64_t b = z->trans[i - 1].off;
        int64_t a = z->trans[i].off;
        if (a > b && w >= t + b && w < t + a) {
            *ob = b;
            *oa = a;
            return 1;
        }
    }
    return 0;
}

/* Classify a civil wall clock in a zone: 0 unique, 1 gap (imaginary), 2 overlap
 * (ambiguous). A naive / fixed value (zone_id < 0) is always unique. */
int64_t zap_tz_civil_kind(int64_t zone_id, int64_t y, int64_t mo, int64_t d,
                          int64_t h, int64_t mi, int64_t s) {
    const zap_tz_zone *z = zap_tz_zone_at(zone_id);
    if (!z) {
        return 0;
    }
    int64_t w = zap_datetime_to_unix(y, mo, d, h, mi, s);
    int64_t a = 0, b = 0;
    int c = zap_tz_wall_candidates(z, w, &a, &b);
    if (c == 0) {
        return 1;
    }
    if (c >= 2) {
        return 2;
    }
    return 0;
}

/* Resolve the UTC offset for a wall clock in a zone, with `fold` selecting the
 * instant on an overlap (0 = earlier, 1 = later) and the pre/post offset in a
 * gap (0 = pre, 1 = post, PEP 495). A naive / fixed value (zone_id < 0) returns
 * `fixed_off` unchanged. */
int64_t zap_tz_resolve_local(int64_t zone_id, int64_t y, int64_t mo, int64_t d,
                             int64_t h, int64_t mi, int64_t s, int64_t fold,
                             int64_t fixed_off) {
    const zap_tz_zone *z = zap_tz_zone_at(zone_id);
    if (!z) {
        return fixed_off;
    }
    int64_t w = zap_datetime_to_unix(y, mo, d, h, mi, s);
    int64_t off0 = 0, off1 = 0;
    int c = zap_tz_wall_candidates(z, w, &off0, &off1);
    if (c == 1) {
        return off0;
    }
    if (c >= 2) {
        return fold ? off1 : off0;
    }
    int64_t ob = 0, oa = 0;
    if (zap_tz_gap_offsets(z, w, &ob, &oa)) {
        return fold ? oa : ob;
    }
    /* No match and not a classic gap (should not occur): fall back to the
     * instant-based lookup so a value is always produced. */
    return zap_tz_offset_for(z, w);
}

/* The canonical real UTC instant for a wall clock in a zone, used by
 * `resolve_imaginary`: a real time (unique OR overlap) is a strict no-op — it
 * maps to the instant denoted by the value's OWN carried offset (`fixed_off`),
 * so rebuilding reproduces the same offset and instant (#1173). This matters on
 * a fall-back overlap, where both `fold=0` and `fold=1` are real: the supplied
 * value already picked its side, and `resolve_imaginary` must leave it there
 * rather than snapping it to the earlier (`off0`) instant. A gap time (no
 * matching interval) maps just past the transition, so rebuilding shifts the
 * wall clock forward by the gap length onto the real post-transition time
 * (dateutil `resolve_imaginary`). Mirrors `is_imaginary` (true only for a gap),
 * the predicate `resolve_imaginary` keys off. */
int64_t zap_tz_imaginary_instant(int64_t zone_id, int64_t y, int64_t mo,
                                 int64_t d, int64_t h, int64_t mi, int64_t s,
                                 int64_t fixed_off) {
    int64_t w = zap_datetime_to_unix(y, mo, d, h, mi, s);
    const zap_tz_zone *z = zap_tz_zone_at(zone_id);
    if (!z) {
        return w - fixed_off;
    }
    int64_t off0 = 0, off1 = 0;
    int c = zap_tz_wall_candidates(z, w, &off0, &off1);
    if (c >= 1) {
        /* Real (unique or ambiguous): preserve the value's own offset/instant.
         * For a unique time `fixed_off == off0`; on an overlap it is whichever
         * fold the value carries, so we must not collapse it to `off0`. */
        return w - fixed_off;
    }
    int64_t ob = 0, oa = 0;
    if (zap_tz_gap_offsets(z, w, &ob, &oa)) {
        return w - ob;
    }
    return w - zap_tz_offset_for(z, w);
}

/* Separate date / time-of-day renderers (#1046, D-0226). `Date` -> "YYYY-MM-DD",
 * `Time` -> "HH:MM:SS" (zero-padded, no zone — these types carry none). */
void *zap_date_isoformat(int64_t y, int64_t mo, int64_t d) {
    char buf[24];
    int n = snprintf(buf, sizeof buf, "%04lld-%02lld-%02lld", (long long)y,
                     (long long)mo, (long long)d);
    if (n < 0) {
        n = 0;
    }
    return zap_str_new(buf, (int64_t)n);
}

void *zap_time_isoformat(int64_t h, int64_t mi, int64_t s) {
    char buf[16];
    int n = snprintf(buf, sizeof buf, "%02lld:%02lld:%02lld", (long long)h,
                     (long long)mi, (long long)s);
    if (n < 0) {
        n = 0;
    }
    return zap_str_new(buf, (int64_t)n);
}

/* Human-readable Duration formatting (Go `time.Duration.String()` style, #1055,
 * D-0228). "0s"; sub-second uses ns / us / ms with a trimmed fraction
 * ("7.5us", "250ms"); >=1s is "<h>h<m>m<s>s" with a trimmed fractional-second
 * ("1.5s", "1m30.5s", "1h1m1s"). ASCII "us" (not "µs") for golden/terminal
 * portability. Negative durations get a leading '-'. */
void *zap_duration_format(int64_t nanos) {
    if (nanos == 0) {
        return zap_str_new("0s", 2);
    }
    int neg = nanos < 0;
    /* abs into uint64, INT64_MIN-safe */
    uint64_t u = neg ? ((uint64_t)(-(nanos + 1)) + 1u) : (uint64_t)nanos;
    char buf[64];
    int n = 0;
    if (u < 1000000000ULL) {
        const char *unit;
        uint64_t scale;
        int digits;
        if (u < 1000ULL) {
            unit = "ns";
            scale = 1;
            digits = 0;
        } else if (u < 1000000ULL) {
            unit = "us";
            scale = 1000;
            digits = 3;
        } else {
            unit = "ms";
            scale = 1000000;
            digits = 6;
        }
        n += snprintf(buf + n, sizeof buf - (size_t)n, "%llu",
                      (unsigned long long)(u / scale));
        uint64_t frac = u % scale;
        if (frac != 0) {
            char fb[16];
            int fw = snprintf(fb, sizeof fb, "%0*llu", digits,
                              (unsigned long long)frac);
            while (fw > 0 && fb[fw - 1] == '0') {
                fw--;
            }
            n += snprintf(buf + n, sizeof buf - (size_t)n, ".%.*s", fw, fb);
        }
        n += snprintf(buf + n, sizeof buf - (size_t)n, "%s", unit);
    } else {
        uint64_t sec_total = u / 1000000000ULL;
        uint64_t frac = u % 1000000000ULL;
        uint64_t s = sec_total % 60;
        uint64_t min_total = sec_total / 60;
        uint64_t m = min_total % 60;
        uint64_t h = min_total / 60;
        if (h > 0) {
            n += snprintf(buf + n, sizeof buf - (size_t)n, "%lluh",
                          (unsigned long long)h);
        }
        if (min_total > 0) {
            n += snprintf(buf + n, sizeof buf - (size_t)n, "%llum",
                          (unsigned long long)m);
        }
        n += snprintf(buf + n, sizeof buf - (size_t)n, "%llu",
                      (unsigned long long)s);
        if (frac != 0) {
            char fb[16];
            int fw = snprintf(fb, sizeof fb, "%09llu", (unsigned long long)frac);
            while (fw > 0 && fb[fw - 1] == '0') {
                fw--;
            }
            n += snprintf(buf + n, sizeof buf - (size_t)n, ".%.*s", fw, fb);
        }
        n += snprintf(buf + n, sizeof buf - (size_t)n, "s");
    }
    if (n < 0) {
        n = 0;
    }
    if (neg) {
        char out[66];
        out[0] = '-';
        memcpy(out + 1, buf, (size_t)n);
        return zap_str_new(out, (int64_t)n + 1);
    }
    return zap_str_new(buf, (int64_t)n);
}
