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
 * v1 is a plain blocking nanosleep (EINTR-resuming): it blocks the current OS
 * thread, so under the M:N scheduler it parks that M rather than yielding
 * cooperatively. A scheduler-aware sleep (via the netpoller timer) is a
 * follow-up (#99).
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
                            int64_t mi, int64_t s, void *fmt) {
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
        case 'Z':
        case 'z':
            /* Naive DateTime: no zone, so these expand empty (Python-faithful). */
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
