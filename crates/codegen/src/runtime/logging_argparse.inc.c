/* ===========================================================================
 * logging / argparse (ticket #101, D-0056). Application scaffolding of the
 * bundled stdlib, dispatched by flat name through the module seam (#121):
 * `logging` and `argparse` are compiler-known namespaced modules (`import`
 * accepted but not required), building on the `io`/`sys` stderr/argv path (#97).
 *
 * Design (flagged in the PR):
 *   - `logging` is leveled (Python's numeric levels DEBUG=10 … CRITICAL=50),
 *     threshold-filtered, written to a raw fd (default stderr), with a
 *     configurable line format over the tokens %(asctime)s / %(levelname)s /
 *     %(message)s (default "%(levelname)s:%(message)s", no timestamp, so output
 *     is deterministic unless the program opts into %(asctime)s). The root
 *     config is process-global, guarded by a mutex so concurrent coroutines
 *     emit whole lines without interleaving. Named loggers and structured
 *     key-value fields are deferred (noted in the PR).
 *   - `argparse` is a GLOBAL SINGLETON parser (a notable v1 choice): with no
 *     prelude object/namespace types on the seam yet, the parser is process-
 *     global state. It supports `--flag` booleans, `--name VALUE` string/int
 *     options (with defaults), required positionals, and an auto-generated
 *     `-h/--help`. `parse(argv)` skips argv[0] (the program name), matching
 *     Python's parse_args(sys.argv[1:]). Subcommands and single-dash short
 *     options are deferred. Parse errors and bad lookups raise a catchable Zap
 *     exception (via zap_panic) rather than Python's exit(2).
 *
 * Config/parser state lives in libc-`malloc`ed memory (NOT GC objects): C
 * globals are not scanned by the precise collector (#48), so persisting a
 * `ZapStr*` across calls would be unsafe. Strings are copied in on the way in
 * and fresh `ZapStr`s are minted on the way out.
 * ========================================================================== */

/* --- shared: a tiny malloc-backed string builder --------------------------- */

typedef struct {
    char *p;
    size_t len;
    size_t cap;
} ZapSB;

static void zap_sb_ensure(ZapSB *b, size_t extra) {
    if (b->len + extra <= b->cap) {
        return;
    }
    size_t cap = b->cap ? b->cap : 64;
    while (cap < b->len + extra) {
        cap *= 2;
    }
    char *p = (char *)realloc(b->p, cap);
    if (p == NULL) {
        perror("zap: realloc");
        abort();
    }
    b->p = p;
    b->cap = cap;
}

static void zap_sb_add(ZapSB *b, const char *s, size_t n) {
    if (n == 0) {
        return;
    }
    zap_sb_ensure(b, n);
    memcpy(b->p + b->len, s, n);
    b->len += n;
}

static void zap_sb_addc(ZapSB *b, const char *s) { zap_sb_add(b, s, strlen(s)); }

/* A fresh malloc'd NUL-terminated copy of a ZapStr (persistent config storage,
 * freed on overwrite/reset). */
static char *zap_cfg_dup(const ZapStr *s) {
    char *out = (char *)malloc((size_t)s->len + 1);
    if (out == NULL) {
        perror("zap: malloc");
        abort();
    }
    if (s->len > 0) {
        memcpy(out, s->data, (size_t)s->len);
    }
    out[s->len] = '\0';
    return out;
}

/* ===========================================================================
 * logging
 * ========================================================================== */

static int64_t g_log_level = 30; /* default threshold: WARNING (Python root) */
static int g_log_fd = 2;         /* default stream: stderr                   */
static char *g_log_fmt = NULL;   /* NULL => the default format below         */
static pthread_mutex_t g_log_mu = PTHREAD_MUTEX_INITIALIZER;

#define ZAP_LOG_DEFAULT_FMT "%(levelname)s:%(message)s"

/* The level name for a numeric level (Python's names); a non-standard level
 * renders as "Level N" into `scratch` (caller holds the config mutex). */
static const char *zap_log_levelname(int64_t level, char *scratch, size_t n) {
    switch (level) {
    case 10:
        return "DEBUG";
    case 20:
        return "INFO";
    case 30:
        return "WARNING";
    case 40:
        return "ERROR";
    case 50:
        return "CRITICAL";
    default:
        snprintf(scratch, n, "Level %lld", (long long)level);
        return scratch;
    }
}

/* logging.set_level(level): drop records below `level`. */
void zap_logging_set_level(int64_t level) {
    zap_np_lock(&g_log_mu);
    g_log_level = level;
    zap_np_unlock(&g_log_mu);
}

/* logging.get_level() -> int64. */
int64_t zap_logging_get_level(void) {
    zap_np_lock(&g_log_mu);
    int64_t v = g_log_level;
    zap_np_unlock(&g_log_mu);
    return v;
}

/* logging.set_stream(fd): direct output to a raw fd (e.g. sys.stdout_fd()). */
void zap_logging_set_stream(int64_t fd) {
    zap_np_lock(&g_log_mu);
    g_log_fd = (int)fd;
    zap_np_unlock(&g_log_mu);
}

/* logging.set_format(fmt): the line template (tokens %(asctime)s /
 * %(levelname)s / %(message)s). */
void zap_logging_set_format(void *fmt_) {
    char *dup = zap_cfg_dup((ZapStr *)fmt_);
    zap_np_lock(&g_log_mu);
    free(g_log_fmt);
    g_log_fmt = dup;
    zap_np_unlock(&g_log_mu);
}

/* logging.log(level, msg) and the per-level emitters: format one line per the
 * active template and write it (threshold permitting). The config mutex is held
 * across the write so lines from concurrent coroutines never interleave. */
void zap_logging_log(int64_t level, void *msg_) {
    ZapStr *msg = (ZapStr *)msg_;
    zap_np_lock(&g_log_mu);
    if (level < g_log_level) {
        zap_np_unlock(&g_log_mu);
        return;
    }
    const char *fmt = g_log_fmt ? g_log_fmt : ZAP_LOG_DEFAULT_FMT;
    int fd = g_log_fd;
    char lvlbuf[32];
    const char *lvlname = zap_log_levelname(level, lvlbuf, sizeof lvlbuf);

    ZapSB b = {0};
    for (const char *c = fmt; *c != '\0';) {
        if (c[0] == '%' && c[1] == '(') {
            const char *end = strstr(c + 2, ")s");
            if (end != NULL) {
                if (strncmp(c + 2, "levelname)s", 11) == 0) {
                    zap_sb_addc(&b, lvlname);
                    c = end + 2;
                    continue;
                }
                if (strncmp(c + 2, "message)s", 9) == 0) {
                    zap_sb_add(&b, msg->data, (size_t)msg->len);
                    c = end + 2;
                    continue;
                }
                if (strncmp(c + 2, "asctime)s", 9) == 0) {
                    time_t now = time(NULL);
                    struct tm tmv;
                    localtime_r(&now, &tmv);
                    char ts[32];
                    size_t k = strftime(ts, sizeof ts, "%Y-%m-%d %H:%M:%S", &tmv);
                    zap_sb_add(&b, ts, k);
                    c = end + 2;
                    continue;
                }
                /* Unknown %(token)s: emit verbatim. */
            }
        }
        zap_sb_add(&b, c, 1);
        c++;
    }
    zap_sb_add(&b, "\n", 1);

    size_t off = 0;
    while (off < b.len) {
        ssize_t w = write(fd, b.p + off, b.len - off);
        if (w < 0) {
            if (errno == EINTR) {
                continue;
            }
            break; /* best-effort: a failed log write never aborts the program */
        }
        off += (size_t)w;
    }
    free(b.p);
    zap_np_unlock(&g_log_mu);
}

/* ===========================================================================
 * argparse
 * ========================================================================== */

enum { AP_FLAG, AP_OPTION, AP_INT, AP_POSITIONAL };

typedef struct {
    char *name;    /* argument name (option long name / positional name)      */
    int kind;      /* AP_FLAG / AP_OPTION / AP_INT / AP_POSITIONAL            */
    char *sval;    /* string value or default (OPTION/POSITIONAL); else NULL  */
    int64_t ival;  /* int value or default (AP_INT)                           */
    int present;   /* set once the argument has been seen / filled            */
} ApArg;

#define AP_MAX 64
static ApArg g_ap_args[AP_MAX];
static int g_ap_nargs = 0;
static char *g_ap_prog = NULL;
static char *g_ap_desc = NULL;
static int g_ap_help = 0;
static pthread_mutex_t g_ap_mu = PTHREAD_MUTEX_INITIALIZER;

/* Raise "argparse: <detail>" (caller must have released g_ap_mu first). */
static void zap_ap_fail(const char *detail) {
    char buf[512];
    int n = snprintf(buf, sizeof buf, "argparse: %s", detail);
    if (n < 0) {
        n = 0;
    }
    if (n >= (int)sizeof buf) {
        n = (int)sizeof buf - 1;
    }
    zap_panic(zap_str_new(buf, (int64_t)n));
}

/* Find a registered argument by name (caller holds g_ap_mu). */
static ApArg *zap_ap_find(const char *name) {
    for (int i = 0; i < g_ap_nargs; i++) {
        if (strcmp(g_ap_args[i].name, name) == 0) {
            return &g_ap_args[i];
        }
    }
    return NULL;
}

/* Register an argument (caller holds g_ap_mu). Returns a detail string on error
 * (table full / duplicate), else NULL. */
static const char *zap_ap_add(const char *name, int kind, char *sval, int64_t ival) {
    if (g_ap_nargs >= AP_MAX) {
        free(sval);
        return "too many arguments registered";
    }
    if (zap_ap_find(name) != NULL) {
        free(sval);
        return "duplicate argument name";
    }
    ApArg *a = &g_ap_args[g_ap_nargs++];
    a->name = strdup(name);
    a->kind = kind;
    a->sval = sval;
    a->ival = ival;
    a->present = 0;
    return NULL;
}

/* Append a "--name" / "--name METAVAR" / "POSITIONAL" spelling to a usage buf. */
static void zap_ap_spelling(ZapSB *b, const ApArg *a, int in_usage) {
    char metavar[128];
    size_t j = 0;
    for (const char *p = a->name; *p != '\0' && j + 1 < sizeof metavar; p++) {
        char ch = *p;
        if (ch >= 'a' && ch <= 'z') {
            ch = (char)(ch - 'a' + 'A');
        }
        metavar[j++] = ch;
    }
    metavar[j] = '\0';
    if (a->kind == AP_POSITIONAL) {
        zap_sb_addc(b, a->name);
        return;
    }
    if (in_usage) {
        zap_sb_addc(b, "[");
    }
    zap_sb_addc(b, "--");
    zap_sb_addc(b, a->name);
    if (a->kind == AP_OPTION || a->kind == AP_INT) {
        zap_sb_addc(b, " ");
        zap_sb_addc(b, metavar);
    }
    if (in_usage) {
        zap_sb_addc(b, "]");
    }
}

/* argparse.set_prog(name). */
void zap_argparse_set_prog(void *name_) {
    char *dup = zap_cfg_dup((ZapStr *)name_);
    zap_np_lock(&g_ap_mu);
    free(g_ap_prog);
    g_ap_prog = dup;
    zap_np_unlock(&g_ap_mu);
}

/* argparse.set_description(text). */
void zap_argparse_set_description(void *text_) {
    char *dup = zap_cfg_dup((ZapStr *)text_);
    zap_np_lock(&g_ap_mu);
    free(g_ap_desc);
    g_ap_desc = dup;
    zap_np_unlock(&g_ap_mu);
}

/* argparse.add_flag(name): a boolean "--name" (default false). */
void zap_argparse_add_flag(void *name_) {
    char *name = zap_cfg_dup((ZapStr *)name_);
    zap_np_lock(&g_ap_mu);
    const char *err = zap_ap_add(name, AP_FLAG, NULL, 0);
    zap_np_unlock(&g_ap_mu);
    free(name);
    if (err != NULL) {
        zap_ap_fail(err);
    }
}

/* argparse.add_positional(name): a required positional string argument. */
void zap_argparse_add_positional(void *name_) {
    char *name = zap_cfg_dup((ZapStr *)name_);
    zap_np_lock(&g_ap_mu);
    const char *err = zap_ap_add(name, AP_POSITIONAL, NULL, 0);
    zap_np_unlock(&g_ap_mu);
    free(name);
    if (err != NULL) {
        zap_ap_fail(err);
    }
}

/* argparse.add_option(name, default): a "--name VALUE" string option. */
void zap_argparse_add_option(void *name_, void *def_) {
    char *name = zap_cfg_dup((ZapStr *)name_);
    char *def = zap_cfg_dup((ZapStr *)def_);
    zap_np_lock(&g_ap_mu);
    const char *err = zap_ap_add(name, AP_OPTION, def, 0);
    zap_np_unlock(&g_ap_mu);
    free(name);
    if (err != NULL) {
        zap_ap_fail(err);
    }
}

/* argparse.add_int(name, default): a "--name VALUE" int64 option. */
void zap_argparse_add_int(void *name_, int64_t def) {
    char *name = zap_cfg_dup((ZapStr *)name_);
    zap_np_lock(&g_ap_mu);
    const char *err = zap_ap_add(name, AP_INT, NULL, def);
    zap_np_unlock(&g_ap_mu);
    free(name);
    if (err != NULL) {
        zap_ap_fail(err);
    }
}

/* Build the generated usage/help text (caller holds g_ap_mu). */
static char *zap_ap_format_help_locked(void) {
    const char *prog = g_ap_prog ? g_ap_prog : "prog";
    ZapSB b = {0};
    zap_sb_addc(&b, "usage: ");
    zap_sb_addc(&b, prog);
    zap_sb_addc(&b, " [-h]");
    for (int i = 0; i < g_ap_nargs; i++) {
        if (g_ap_args[i].kind != AP_POSITIONAL) {
            zap_sb_addc(&b, " ");
            zap_ap_spelling(&b, &g_ap_args[i], 1);
        }
    }
    for (int i = 0; i < g_ap_nargs; i++) {
        if (g_ap_args[i].kind == AP_POSITIONAL) {
            zap_sb_addc(&b, " ");
            zap_ap_spelling(&b, &g_ap_args[i], 1);
        }
    }
    zap_sb_addc(&b, "\n");
    if (g_ap_desc != NULL) {
        zap_sb_addc(&b, "\n");
        zap_sb_addc(&b, g_ap_desc);
        zap_sb_addc(&b, "\n");
    }
    int has_pos = 0;
    for (int i = 0; i < g_ap_nargs; i++) {
        if (g_ap_args[i].kind == AP_POSITIONAL) {
            has_pos = 1;
            break;
        }
    }
    if (has_pos) {
        zap_sb_addc(&b, "\npositional arguments:\n");
        for (int i = 0; i < g_ap_nargs; i++) {
            if (g_ap_args[i].kind == AP_POSITIONAL) {
                zap_sb_addc(&b, "  ");
                zap_ap_spelling(&b, &g_ap_args[i], 0);
                zap_sb_addc(&b, "\n");
            }
        }
    }
    zap_sb_addc(&b, "\noptions:\n  -h, --help  show this help message and exit\n");
    for (int i = 0; i < g_ap_nargs; i++) {
        if (g_ap_args[i].kind != AP_POSITIONAL) {
            zap_sb_addc(&b, "  ");
            zap_ap_spelling(&b, &g_ap_args[i], 0);
            zap_sb_addc(&b, "\n");
        }
    }
    zap_sb_ensure(&b, 1);
    b.p[b.len] = '\0';
    return b.p; /* caller frees */
}

/* argparse.parse(argv): parse `argv` (a list[str]); argv[0] is the program name
 * and is skipped (Python's parse_args(sys.argv[1:])). Raises on error. */
void zap_argparse_parse(void *argv_) {
    int64_t n = zap_list_len(argv_);
    char errbuf[256];
    const char *err = NULL;
    int printed_help = 0;

    zap_np_lock(&g_ap_mu);
    if (g_ap_prog == NULL && n >= 1) {
        ZapStr *p0 = (ZapStr *)zap_list_get_ptr(argv_, 0);
        g_ap_prog = zap_cfg_dup(p0);
    }
    int64_t next_pos = 0; /* index into positionals, in registration order */
    for (int64_t i = 1; i < n; i++) {
        ZapStr *tok = (ZapStr *)zap_list_get_ptr(argv_, i);
        char *s = zap_cfg_dup(tok); /* NUL-terminated working copy */
        if (strcmp(s, "-h") == 0 || strcmp(s, "--help") == 0) {
            g_ap_help = 1;
            char *help = zap_ap_format_help_locked();
            size_t hlen = strlen(help);
            size_t off = 0;
            while (off < hlen) {
                ssize_t w = write(1, help + off, hlen - off);
                if (w < 0) {
                    if (errno == EINTR) {
                        continue;
                    }
                    break;
                }
                off += (size_t)w;
            }
            free(help);
            free(s);
            printed_help = 1;
            continue;
        }
        if (s[0] == '-' && s[1] == '-') {
            ApArg *a = zap_ap_find(s + 2);
            if (a == NULL) {
                snprintf(errbuf, sizeof errbuf, "unrecognized argument: %s", s);
                err = errbuf;
                free(s);
                break;
            }
            if (a->kind == AP_FLAG) {
                a->present = 1;
                free(s);
                continue;
            }
            /* OPTION / INT need a following value. */
            if (i + 1 >= n) {
                snprintf(errbuf, sizeof errbuf, "argument --%s: expected one argument",
                         a->name);
                err = errbuf;
                free(s);
                break;
            }
            ZapStr *vtok = (ZapStr *)zap_list_get_ptr(argv_, ++i);
            char *v = zap_cfg_dup(vtok);
            if (a->kind == AP_OPTION) {
                free(a->sval);
                a->sval = v;
                a->present = 1;
            } else { /* AP_INT */
                char *endp = NULL;
                errno = 0;
                long long parsed = strtoll(v, &endp, 10);
                if (errno != 0 || endp == v || *endp != '\0') {
                    snprintf(errbuf, sizeof errbuf,
                             "argument --%s: invalid int value: '%s'", a->name, v);
                    err = errbuf;
                    free(v);
                    free(s);
                    break;
                }
                a->ival = (int64_t)parsed;
                a->present = 1;
                free(v);
            }
            free(s);
            continue;
        }
        /* A positional. */
        ApArg *slot = NULL;
        int64_t seen = 0;
        for (int k = 0; k < g_ap_nargs; k++) {
            if (g_ap_args[k].kind == AP_POSITIONAL) {
                if (seen == next_pos) {
                    slot = &g_ap_args[k];
                    break;
                }
                seen++;
            }
        }
        if (slot == NULL) {
            snprintf(errbuf, sizeof errbuf, "unrecognized argument: %s", s);
            err = errbuf;
            free(s);
            break;
        }
        free(slot->sval);
        slot->sval = s; /* takes ownership of the working copy */
        slot->present = 1;
        next_pos++;
    }
    /* Required positionals must all be present (skipped when -h/--help seen,
     * matching Python). */
    if (err == NULL && !printed_help) {
        for (int k = 0; k < g_ap_nargs; k++) {
            if (g_ap_args[k].kind == AP_POSITIONAL && !g_ap_args[k].present) {
                snprintf(errbuf, sizeof errbuf,
                         "the following argument is required: %s", g_ap_args[k].name);
                err = errbuf;
                break;
            }
        }
    }
    zap_np_unlock(&g_ap_mu);
    if (err != NULL) {
        zap_ap_fail(err);
    }
}

/* argparse.get(name) -> str: an OPTION's value / a POSITIONAL's value. */
void *zap_argparse_get(void *name_) {
    char *name = zap_cfg_dup((ZapStr *)name_);
    char errbuf[256];
    const char *err = NULL;
    char *val = NULL;
    zap_np_lock(&g_ap_mu);
    ApArg *a = zap_ap_find(name);
    if (a == NULL) {
        snprintf(errbuf, sizeof errbuf, "no such argument: '%s'", name);
        err = errbuf;
    } else if (a->kind != AP_OPTION && a->kind != AP_POSITIONAL) {
        snprintf(errbuf, sizeof errbuf, "argument '%s' is not a string", name);
        err = errbuf;
    } else {
        val = strdup(a->sval ? a->sval : "");
    }
    zap_np_unlock(&g_ap_mu);
    free(name);
    if (err != NULL) {
        zap_ap_fail(err);
    }
    void *out = zap_str_new(val, (int64_t)strlen(val));
    free(val);
    return out;
}

/* argparse.get_int(name) -> int64: an AP_INT option's value (or default). */
int64_t zap_argparse_get_int(void *name_) {
    char *name = zap_cfg_dup((ZapStr *)name_);
    char errbuf[256];
    const char *err = NULL;
    int64_t val = 0;
    zap_np_lock(&g_ap_mu);
    ApArg *a = zap_ap_find(name);
    if (a == NULL) {
        snprintf(errbuf, sizeof errbuf, "no such argument: '%s'", name);
        err = errbuf;
    } else if (a->kind != AP_INT) {
        snprintf(errbuf, sizeof errbuf, "argument '%s' is not an int", name);
        err = errbuf;
    } else {
        val = a->ival;
    }
    zap_np_unlock(&g_ap_mu);
    free(name);
    if (err != NULL) {
        zap_ap_fail(err);
    }
    return val;
}

/* argparse.get_flag(name) -> int64 0/1: whether an AP_FLAG was present. */
int64_t zap_argparse_get_flag(void *name_) {
    char *name = zap_cfg_dup((ZapStr *)name_);
    char errbuf[256];
    const char *err = NULL;
    int64_t val = 0;
    zap_np_lock(&g_ap_mu);
    ApArg *a = zap_ap_find(name);
    if (a == NULL) {
        snprintf(errbuf, sizeof errbuf, "no such argument: '%s'", name);
        err = errbuf;
    } else if (a->kind != AP_FLAG) {
        snprintf(errbuf, sizeof errbuf, "argument '%s' is not a flag", name);
        err = errbuf;
    } else {
        val = a->present ? 1 : 0;
    }
    zap_np_unlock(&g_ap_mu);
    free(name);
    if (err != NULL) {
        zap_ap_fail(err);
    }
    return val;
}

/* argparse.format_help() -> str: the generated usage/help text. */
void *zap_argparse_format_help(void) {
    zap_np_lock(&g_ap_mu);
    char *help = zap_ap_format_help_locked();
    zap_np_unlock(&g_ap_mu);
    void *out = zap_str_new(help, (int64_t)strlen(help));
    free(help);
    return out;
}

/* argparse.help_requested() -> int64 0/1: whether -h/--help was seen. */
int64_t zap_argparse_help_requested(void) {
    zap_np_lock(&g_ap_mu);
    int64_t v = g_ap_help ? 1 : 0;
    zap_np_unlock(&g_ap_mu);
    return v;
}

/* argparse.reset(): clear all registered arguments and parsed state. */
void zap_argparse_reset(void) {
    zap_np_lock(&g_ap_mu);
    for (int i = 0; i < g_ap_nargs; i++) {
        free(g_ap_args[i].name);
        free(g_ap_args[i].sval);
        g_ap_args[i].name = NULL;
        g_ap_args[i].sval = NULL;
    }
    g_ap_nargs = 0;
    free(g_ap_prog);
    g_ap_prog = NULL;
    free(g_ap_desc);
    g_ap_desc = NULL;
    g_ap_help = 0;
    zap_np_unlock(&g_ap_mu);
}

/* JSON boolean literal — lowercase "true"/"false" (RFC 8259), for the
 * @derive(Json) `to_json` codec (ticket #100 / #131). Distinct from
 * zap_bool_to_str, whose Python-style "True"/"False" is not valid JSON. */
void *zap_json_bool_str(int64_t b) {
    return b ? zap_str_new("true", 4) : zap_str_new("false", 5);
}
