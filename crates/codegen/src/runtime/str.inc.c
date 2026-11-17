/* zap runtime fragment: str.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * stdlib: thin `str` method set (ticket #971, D-0210)
 * ---------------------------------------------------------------------------
 * The Python-faithful string toolkit that was missing: whitespace / maxsplit
 * split, rsplit, splitlines; count / rfind / rindex; removeprefix / removesuffix
 * and char-set strip; the ljust / rjust / center / zfill pad family; the
 * title / capitalize / casefold / swapcase case family; and the is* predicates.
 * All operate over raw UTF-8 bytes (a `str`'s bytes ARE its UTF-8), so ASCII
 * classification is byte-local and multibyte codepoints pass through untouched;
 * offsets returned (`rfind`/`rindex`) and counts are BYTE offsets/counts (as
 * `find`/`index` already are), which coincide with codepoints on ASCII input.
 * Padding widths count CODEPOINTS (`zap_str_len_cp`), Python-style. None of these
 * poll a safepoint, so every raw slice stays valid for the call ("allocation
 * never collects mid-call"). `maxsplit < 0` means unlimited.
 * =========================================================================== */

/* split() with no separator: split on runs of ASCII whitespace, ignoring
 * leading/trailing whitespace (Python `str.split()` / Go `strings.Fields`). With
 * `maxsplit >= 0`, after that many splits the remainder (from its first
 * non-whitespace byte to the end, verbatim) is the final field. An all-whitespace
 * or empty string yields an empty list. */
void *zap_str_split_ws(void *sp, int64_t maxsplit) {
    const ZapStr *s = (const ZapStr *)sp;
    const char *d = s->data;
    int64_t n = s->len;
    void *out = zap_list_new();
    int64_t i = 0, splits = 0;
    while (i < n) {
        while (i < n && zap_is_ascii_ws((unsigned char)d[i])) {
            i++;
        }
        if (i >= n) {
            break;
        }
        if (maxsplit >= 0 && splits == maxsplit) {
            /* remainder is one field, verbatim (keeps interior/trailing ws). */
            zap_list_push_ptr(out, zap_str_new(d + i, n - i));
            break;
        }
        int64_t start = i;
        while (i < n && !zap_is_ascii_ws((unsigned char)d[i])) {
            i++;
        }
        zap_list_push_ptr(out, zap_str_new(d + start, i - start));
        splits++;
    }
    return out;
}

/* split(sep[, maxsplit]): split on the explicit (non-empty) separator. An empty
 * separator returns the whole string as the single element (unchanged from the
 * historical `zap_str_split`, which never raised). `maxsplit >= 0` caps the
 * number of splits; the tail keeps the rest verbatim. */
void *zap_str_split_sep(void *sp, void *sepp, int64_t maxsplit) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *sep = (const ZapStr *)sepp;
    const char *d = s->data;
    int64_t n = s->len, m = sep->len;
    void *out = zap_list_new();
    if (m == 0) {
        zap_list_push_ptr(out, zap_str_new(d, n));
        return out;
    }
    int64_t start = 0, i = 0, splits = 0;
    while (i + m <= n) {
        if (maxsplit >= 0 && splits == maxsplit) {
            break;
        }
        if (memcmp(d + i, sep->data, (size_t)m) == 0) {
            zap_list_push_ptr(out, zap_str_new(d + start, i - start));
            i += m;
            start = i;
            splits++;
        } else {
            i++;
        }
    }
    zap_list_push_ptr(out, zap_str_new(d + start, n - start));
    return out;
}

/* rsplit() with no separator: whitespace split from the right. Only `maxsplit`
 * matters for the result vs `split_ws` (the head keeps the rest verbatim); with
 * no limit the result equals `split_ws`. */
void *zap_str_rsplit_ws(void *sp, int64_t maxsplit) {
    const ZapStr *s = (const ZapStr *)sp;
    const char *d = s->data;
    int64_t n = s->len;
    if (maxsplit < 0) {
        return zap_str_split_ws(sp, -1);
    }
    /* collect fields from the right into a temp list, then reverse. */
    void *rev = zap_list_new();
    int64_t i = n, splits = 0;
    while (i > 0) {
        while (i > 0 && zap_is_ascii_ws((unsigned char)d[i - 1])) {
            i--;
        }
        if (i <= 0) {
            break;
        }
        if (splits == maxsplit) {
            zap_list_push_ptr(rev, zap_str_new(d, i));
            i = 0;
            break;
        }
        int64_t end = i;
        while (i > 0 && !zap_is_ascii_ws((unsigned char)d[i - 1])) {
            i--;
        }
        zap_list_push_ptr(rev, zap_str_new(d + i, end - i));
        splits++;
    }
    int64_t k = zap_list_len(rev);
    void *out = zap_list_new();
    for (int64_t j = k - 1; j >= 0; j--) {
        zap_list_push_ptr(out, zap_list_get_ptr(rev, j));
    }
    return out;
}

/* rsplit(sep[, maxsplit]): split on an explicit separator from the right. */
void *zap_str_rsplit_sep(void *sp, void *sepp, int64_t maxsplit) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *sep = (const ZapStr *)sepp;
    const char *d = s->data;
    int64_t n = s->len, m = sep->len;
    if (m == 0 || maxsplit < 0) {
        return zap_str_split_sep(sp, sepp, maxsplit);
    }
    void *rev = zap_list_new();
    int64_t end = n, i = n - m, splits = 0;
    while (i >= 0) {
        if (splits == maxsplit) {
            break;
        }
        if (memcmp(d + i, sep->data, (size_t)m) == 0) {
            zap_list_push_ptr(rev, zap_str_new(d + i + m, end - (i + m)));
            end = i;
            i -= m;
            splits++;
        } else {
            i--;
        }
    }
    zap_list_push_ptr(rev, zap_str_new(d, end));
    int64_t k = zap_list_len(rev);
    void *out = zap_list_new();
    for (int64_t j = k - 1; j >= 0; j--) {
        zap_list_push_ptr(out, zap_list_get_ptr(rev, j));
    }
    return out;
}

/* splitlines(): split on ASCII line boundaries (\n, \r, \r\n), dropping the
 * terminators (keepends=False). A trailing newline does NOT produce a final
 * empty field (Python). */
void *zap_str_splitlines(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    const char *d = s->data;
    int64_t n = s->len;
    void *out = zap_list_new();
    int64_t start = 0, i = 0;
    while (i < n) {
        unsigned char c = (unsigned char)d[i];
        if (c == '\n' || c == '\r') {
            zap_list_push_ptr(out, zap_str_new(d + start, i - start));
            if (c == '\r' && i + 1 < n && d[i + 1] == '\n') {
                i += 2;
            } else {
                i++;
            }
            start = i;
        } else {
            i++;
        }
    }
    if (start < n) {
        zap_list_push_ptr(out, zap_str_new(d + start, n - start));
    }
    return out;
}

/* count(sub): non-overlapping occurrences (byte scan). An empty `sub` returns
 * byte-length + 1 (Python's count-of-empty, byte-based here). */
int64_t zap_str_count(void *sp, void *subp) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *sub = (const ZapStr *)subp;
    int64_t n = s->len, m = sub->len;
    if (m == 0) {
        return n + 1;
    }
    int64_t c = 0, i = 0;
    while (i + m <= n) {
        if (memcmp(s->data + i, sub->data, (size_t)m) == 0) {
            c++;
            i += m;
        } else {
            i++;
        }
    }
    return c;
}

/* rfind(sub): last byte offset of `sub`, or -1. An empty `sub` returns the
 * byte length (Python). */
int64_t zap_str_rfind(void *sp, void *subp) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *sub = (const ZapStr *)subp;
    int64_t n = s->len, m = sub->len;
    if (m == 0) {
        return n;
    }
    for (int64_t i = n - m; i >= 0; i--) {
        if (memcmp(s->data + i, sub->data, (size_t)m) == 0) {
            return i;
        }
    }
    return -1;
}

/* rindex(sub): like rfind, but faults when absent (Python's ValueError), the
 * same abort contract as `zap_str_index_of`. */
int64_t zap_str_rindex(void *sp, void *subp) {
    int64_t r = zap_str_rfind(sp, subp);
    if (r < 0) {
        fprintf(stderr, "zap: substring not found\n");
        abort();
    }
    return r;
}

/* removeprefix(p): drop a leading `p` if present (else unchanged). */
void *zap_str_removeprefix(void *sp, void *pp) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *p = (const ZapStr *)pp;
    if (p->len > 0 && p->len <= s->len && memcmp(s->data, p->data, (size_t)p->len) == 0) {
        return zap_str_new(s->data + p->len, s->len - p->len);
    }
    return zap_str_new(s->data, s->len);
}

/* removesuffix(x): drop a trailing `x` if present (else unchanged). */
void *zap_str_removesuffix(void *sp, void *xp) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *x = (const ZapStr *)xp;
    if (x->len > 0 && x->len <= s->len &&
        memcmp(s->data + (s->len - x->len), x->data, (size_t)x->len) == 0) {
        return zap_str_new(s->data, s->len - x->len);
    }
    return zap_str_new(s->data, s->len);
}

/* strip(chars, mode): mode 0 both / 1 left / 2 right. Trim any byte that appears
 * in the `chars` set (Python `strip(chars)`). An empty set trims nothing. */
void *zap_str_strip_chars(void *sp, void *charsp, int64_t mode) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *cs = (const ZapStr *)charsp;
    int64_t a = 0, b = s->len;
    uint8_t set[256] = {0};
    for (int64_t i = 0; i < cs->len; i++) {
        set[(unsigned char)cs->data[i]] = 1;
    }
    if (mode != 2) {
        while (a < b && set[(unsigned char)s->data[a]]) {
            a++;
        }
    }
    if (mode != 1) {
        while (b > a && set[(unsigned char)s->data[b - 1]]) {
            b--;
        }
    }
    return zap_str_new(s->data + a, b - a);
}

/* Pad `s` to `width` CODEPOINTS using the first byte of `fill` (ASCII; space
 * when `fill` is empty). `side`: 0 left-justify (pad right), 1 right-justify
 * (pad left), 2 center. If `s` is already wide enough it is returned unchanged. */
static void *zap_str_pad(void *sp, int64_t width, void *fillp, int side) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *fill = (const ZapStr *)fillp;
    char fc = (fill && fill->len > 0) ? fill->data[0] : ' ';
    int64_t cur = zap_str_len_cp(sp);
    if (width <= cur) {
        return zap_str_new(s->data, s->len);
    }
    int64_t pad = width - cur;
    int64_t left = 0, right = 0;
    if (side == 0) {
        right = pad;
    } else if (side == 1) {
        left = pad;
    } else {
        left = pad / 2;
        right = pad - left;
    }
    int64_t total = s->len + left + right;
    char *buf = (char *)zap_buf((size_t)(total > 0 ? total : 1));
    int64_t w = 0;
    for (int64_t i = 0; i < left; i++) {
        buf[w++] = fc;
    }
    memcpy(buf + w, s->data, (size_t)s->len);
    w += s->len;
    for (int64_t i = 0; i < right; i++) {
        buf[w++] = fc;
    }
    return zap_str_adopt(buf, total);
}

void *zap_str_ljust(void *sp, int64_t width, void *fillp) {
    return zap_str_pad(sp, width, fillp, 0);
}
void *zap_str_rjust(void *sp, int64_t width, void *fillp) {
    return zap_str_pad(sp, width, fillp, 1);
}
void *zap_str_center(void *sp, int64_t width, void *fillp) {
    return zap_str_pad(sp, width, fillp, 2);
}

/* zfill(width): right-justify with '0', but keep a leading sign ('+'/'-') at the
 * front (Python). Codepoint width. */
void *zap_str_zfill(void *sp, int64_t width) {
    const ZapStr *s = (const ZapStr *)sp;
    int64_t cur = zap_str_len_cp(sp);
    if (width <= cur) {
        return zap_str_new(s->data, s->len);
    }
    int64_t pad = width - cur;
    int64_t total = s->len + pad;
    char *buf = (char *)zap_buf((size_t)(total > 0 ? total : 1));
    int64_t w = 0, src = 0;
    if (s->len > 0 && (s->data[0] == '+' || s->data[0] == '-')) {
        buf[w++] = s->data[0];
        src = 1;
    }
    for (int64_t i = 0; i < pad; i++) {
        buf[w++] = '0';
    }
    memcpy(buf + w, s->data + src, (size_t)(s->len - src));
    w += s->len - src;
    return zap_str_adopt(buf, total);
}

/* title(): ASCII titlecase — the first letter of each run of letters uppercased,
 * the rest lowercased; non-letters are word boundaries. */
void *zap_str_title(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    char *buf = (char *)zap_buf((size_t)(s->len > 0 ? s->len : 1));
    int in_word = 0;
    for (int64_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        int is_alpha = (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z');
        if (is_alpha) {
            if (!in_word) {
                buf[i] = (c >= 'a' && c <= 'z') ? (char)(c - 32) : (char)c;
            } else {
                buf[i] = (c >= 'A' && c <= 'Z') ? (char)(c + 32) : (char)c;
            }
            in_word = 1;
        } else {
            buf[i] = (char)c;
            in_word = 0;
        }
    }
    return zap_str_adopt(buf, s->len);
}

/* capitalize(): first char uppercased, the rest lowercased (ASCII). */
void *zap_str_capitalize(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    char *buf = (char *)zap_buf((size_t)(s->len > 0 ? s->len : 1));
    for (int64_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        if (i == 0) {
            buf[i] = (c >= 'a' && c <= 'z') ? (char)(c - 32) : (char)c;
        } else {
            buf[i] = (c >= 'A' && c <= 'Z') ? (char)(c + 32) : (char)c;
        }
    }
    return zap_str_adopt(buf, s->len);
}

/* casefold(): ASCII case-folding == lowercase (full Unicode folding is a
 * documented follow-up). */
void *zap_str_casefold(void *sp) { return zap_str_lower(sp); }

/* swapcase(): invert the case of each ASCII letter. */
void *zap_str_swapcase(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    char *buf = (char *)zap_buf((size_t)(s->len > 0 ? s->len : 1));
    for (int64_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        if (c >= 'a' && c <= 'z') {
            buf[i] = (char)(c - 32);
        } else if (c >= 'A' && c <= 'Z') {
            buf[i] = (char)(c + 32);
        } else {
            buf[i] = (char)c;
        }
    }
    return zap_str_adopt(buf, s->len);
}

/* The is* predicates (ASCII). Each is False for the empty string (Python). */
int64_t zap_str_isdigit(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    if (s->len == 0) {
        return 0;
    }
    for (int64_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        if (c < '0' || c > '9') {
            return 0;
        }
    }
    return 1;
}
int64_t zap_str_isalpha(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    if (s->len == 0) {
        return 0;
    }
    for (int64_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        if (!((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z'))) {
            return 0;
        }
    }
    return 1;
}
int64_t zap_str_isalnum(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    if (s->len == 0) {
        return 0;
    }
    for (int64_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        if (!((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9'))) {
            return 0;
        }
    }
    return 1;
}
int64_t zap_str_isspace(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    if (s->len == 0) {
        return 0;
    }
    for (int64_t i = 0; i < s->len; i++) {
        if (!zap_is_ascii_ws((unsigned char)s->data[i])) {
            return 0;
        }
    }
    return 1;
}
/* islower(): at least one cased letter, and no uppercase letter (Python). */
int64_t zap_str_islower(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    int cased = 0;
    for (int64_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        if (c >= 'A' && c <= 'Z') {
            return 0;
        }
        if (c >= 'a' && c <= 'z') {
            cased = 1;
        }
    }
    return cased ? 1 : 0;
}
/* isupper(): at least one cased letter, and no lowercase letter (Python). */
int64_t zap_str_isupper(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    int cased = 0;
    for (int64_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        if (c >= 'a' && c <= 'z') {
            return 0;
        }
        if (c >= 'A' && c <= 'Z') {
            cased = 1;
        }
    }
    return cased ? 1 : 0;
}

typedef struct {
    ZapStr *type_name;
    ZapStr *deco_name;
} ZapReflectEntry;

typedef struct {
    ZapReflectEntry *items;
    int64_t len;
    int64_t cap;
} ZapReflectTable;

static ZapReflectTable zap_reflect_table;

/* Record that `type_name` carries the annotation/decorator `deco_name`. */
void zap_reflect_register(void *type_name, void *deco_name) {
    ZapReflectTable *t = &zap_reflect_table;
    if (t->len == t->cap) {
        int64_t ncap = t->cap == 0 ? 8 : t->cap * 2;
        ZapReflectEntry *ni =
            (ZapReflectEntry *)zap_ptrbuf((int64_t)ncap * (int64_t)sizeof(ZapReflectEntry));
        for (int64_t i = 0; i < t->len; i++) {
            ni[i] = t->items[i];
        }
        t->items = ni;
        t->cap = ncap;
    }
    t->items[t->len].type_name = (ZapStr *)type_name;
    t->items[t->len].deco_name = (ZapStr *)deco_name;
    t->len++;
}

/* 1 if `type_name` was registered with `deco_name`, else 0. */
int64_t zap_reflect_has(void *type_name, void *deco_name) {
    ZapReflectTable *t = &zap_reflect_table;
    for (int64_t i = 0; i < t->len; i++) {
        if (zap_str_eq(t->items[i].type_name, type_name) &&
            zap_str_eq(t->items[i].deco_name, deco_name)) {
            return 1;
        }
    }
    return 0;
}

/* The comma-joined names registered for `type_name` (""+ if none), in
 * registration order. */
void *zap_reflect_names(void *type_name) {
    ZapReflectTable *t = &zap_reflect_table;
    ZapStr *out = (ZapStr *)zap_str_new("", 0);
    int first = 1;
    for (int64_t i = 0; i < t->len; i++) {
        if (!zap_str_eq(t->items[i].type_name, type_name)) {
            continue;
        }
        if (!first) {
            out = (ZapStr *)zap_str_concat(out, zap_str_new(", ", 2));
        }
        out = (ZapStr *)zap_str_concat(out, t->items[i].deco_name);
        first = 0;
    }
    return out;
}

/* ---------------------------------------------------------------------------
 * Function/callable signature reflection (I#458, D-0089). A parallel registry
 * to the struct/type one above, keyed by an `owner` string — a function name
 * (top-level `def`) or `"Type.method"`. Two flat tables, populated once at the
 * top of `main`: one `ZapParamEntry` per reflectable parameter (in declaration
 * order), and one `ZapSigEntry` per function holding its precomputed readable
 * signature. Queried by `reflect_params`/`reflect_sig`/`reflect_param_*`.
 * ------------------------------------------------------------------------- */

typedef struct {
    ZapStr *owner;
    ZapStr *name;
    ZapStr *type_disp; /* declared type display, "" if untyped */
    int64_t has_default;
    ZapStr *annos; /* comma-joined per-param annotation names, "" if none */
} ZapParamEntry;

typedef struct {
    ZapParamEntry *items;
    int64_t len;
    int64_t cap;
} ZapParamTable;

typedef struct {
    ZapStr *owner;
    ZapStr *sig;
} ZapSigEntry;

typedef struct {
    ZapSigEntry *items;
    int64_t len;
    int64_t cap;
} ZapSigTable;

static ZapParamTable zap_param_table;
static ZapSigTable zap_sig_table;

/* Record one reflectable parameter of `owner` (append preserves declaration
 * order). */
void zap_reflect_param_register(void *owner, void *name, void *type_disp,
                                int64_t has_default, void *annos) {
    ZapParamTable *t = &zap_param_table;
    if (t->len == t->cap) {
        int64_t ncap = t->cap == 0 ? 8 : t->cap * 2;
        ZapParamEntry *ni =
            (ZapParamEntry *)zap_ptrbuf((int64_t)ncap * (int64_t)sizeof(ZapParamEntry));
        for (int64_t i = 0; i < t->len; i++) {
            ni[i] = t->items[i];
        }
        t->items = ni;
        t->cap = ncap;
    }
    t->items[t->len].owner = (ZapStr *)owner;
    t->items[t->len].name = (ZapStr *)name;
    t->items[t->len].type_disp = (ZapStr *)type_disp;
    t->items[t->len].has_default = has_default;
    t->items[t->len].annos = (ZapStr *)annos;
    t->len++;
}

/* Record the precomputed readable signature of `owner`. */
void zap_reflect_sig_register(void *owner, void *sig) {
    ZapSigTable *t = &zap_sig_table;
    if (t->len == t->cap) {
        int64_t ncap = t->cap == 0 ? 8 : t->cap * 2;
        ZapSigEntry *ni = (ZapSigEntry *)zap_ptrbuf((int64_t)ncap * (int64_t)sizeof(ZapSigEntry));
        for (int64_t i = 0; i < t->len; i++) {
            ni[i] = t->items[i];
        }
        t->items = ni;
        t->cap = ncap;
    }
    t->items[t->len].owner = (ZapStr *)owner;
    t->items[t->len].sig = (ZapStr *)sig;
    t->len++;
}

/* The comma-joined parameter names of `owner` ("" if none/unknown), in
 * declaration order. */
void *zap_reflect_params(void *owner) {
    ZapParamTable *t = &zap_param_table;
    ZapStr *out = (ZapStr *)zap_str_new("", 0);
    int first = 1;
    for (int64_t i = 0; i < t->len; i++) {
        if (!zap_str_eq(t->items[i].owner, owner)) {
            continue;
        }
        if (!first) {
            out = (ZapStr *)zap_str_concat(out, zap_str_new(", ", 2));
        }
        out = (ZapStr *)zap_str_concat(out, t->items[i].name);
        first = 0;
    }
    return out;
}

/* The declared type display of `owner`'s parameter `name` ("" if untyped or not
 * found). The first matching (owner, name) wins. */
void *zap_reflect_param_type(void *owner, void *name) {
    ZapParamTable *t = &zap_param_table;
    for (int64_t i = 0; i < t->len; i++) {
        if (zap_str_eq(t->items[i].owner, owner) && zap_str_eq(t->items[i].name, name)) {
            return t->items[i].type_disp;
        }
    }
    return zap_str_new("", 0);
}

/* 1 if `owner`'s parameter `name` has a default value, else 0 (also 0 if not
 * found). */
int64_t zap_reflect_param_has_default(void *owner, void *name) {
    ZapParamTable *t = &zap_param_table;
    for (int64_t i = 0; i < t->len; i++) {
        if (zap_str_eq(t->items[i].owner, owner) && zap_str_eq(t->items[i].name, name)) {
            return t->items[i].has_default;
        }
    }
    return 0;
}

/* The comma-joined annotation names on `owner`'s parameter `name` ("" if none or
 * not found). */
void *zap_reflect_param_annotations(void *owner, void *name) {
    ZapParamTable *t = &zap_param_table;
    for (int64_t i = 0; i < t->len; i++) {
        if (zap_str_eq(t->items[i].owner, owner) && zap_str_eq(t->items[i].name, name)) {
            return t->items[i].annos;
        }
    }
    return zap_str_new("", 0);
}

/* The precomputed readable signature of `owner` ("" if unknown). */
void *zap_reflect_sig(void *owner) {
    ZapSigTable *t = &zap_sig_table;
    for (int64_t i = 0; i < t->len; i++) {
        if (zap_str_eq(t->items[i].owner, owner)) {
            return t->items[i].sig;
        }
    }
    return zap_str_new("", 0);
}

/* ---------------------------------------------------------------------------
 * Regex (ticket #23, D-0010): Perl-style `m/.../` and `s/.../.../` backed by
 * PCRE2 (8-bit API). The compiler lowers each literal to one of the entry
 * points below (see crates/ir/src/lower.rs); patterns arrive as `ZapStr*`
 * holding the verbatim pattern text and a flag bitmask.
 *
 * A match is a GC-allocated `ZapMatch` that *owns* copies of every captured
 * group (and a copy of the pattern's name table). Because the captures live in
 * the returned value and not in any global/thread-local state, capture state is
 * coroutine-safe by construction (D-0010) — a coroutine that migrates between
 * OS threads cannot observe another's "last match". It also lets us free the
 * compiled program immediately; a compiled-pattern cache is future work.
 *
 * Flag bits mirror `regex_flag_bits` in lower.rs: 1=i 2=m 4=s 8=x 16=g. */

#define ZAP_RE_CASELESS 1
#define ZAP_RE_MULTILINE 2
#define ZAP_RE_DOTALL 4
#define ZAP_RE_EXTENDED 8
#define ZAP_RE_GLOBAL 16

typedef struct {
    int64_t ngroups;   /* capture groups, including group 0 (whole match) */
    ZapStr **groups;   /* ngroups entries; NULL if the group did not match */
    int64_t nnames;    /* named capture groups */
    ZapStr **names;    /* nnames name strings */
    int64_t *name_num; /* parallel: the group number each name refers to */
} ZapMatch;

/* Non-null test for a reference value. Codegen's `ICmp` only compares integers,
 * so pointer truthiness (`if match:`, `!~`) routes through this. */
int64_t zap_ptr_nonnull(void *p) { return p != NULL ? 1 : 0; }

#if !defined(ZAP_NO_PCRE2)
static uint32_t zap_regex_compile_opts(int64_t flags) {
    uint32_t o = 0;
    if (flags & ZAP_RE_CASELESS) o |= PCRE2_CASELESS;
    if (flags & ZAP_RE_MULTILINE) o |= PCRE2_MULTILINE;
    if (flags & ZAP_RE_DOTALL) o |= PCRE2_DOTALL;
    if (flags & ZAP_RE_EXTENDED) o |= PCRE2_EXTENDED;
    return o;
}

/* Report a regex error and abort (exit 1). Literal-pattern validation is a
 * runtime check for now; a compile-time check would need PCRE2 in the compiler,
 * which must stay host-safe (buildable without LLVM/PCRE2 on the host). */
static void zap_regex_fail(const char *what, int errorcode) {
    PCRE2_UCHAR msg[256];
    pcre2_get_error_message(errorcode, msg, sizeof(msg));
    fprintf(stderr, "regex error: %s: %s\n", what, (const char *)msg);
    exit(1);
}

static pcre2_code *zap_regex_compile(const ZapStr *pattern, int64_t flags) {
    int errorcode;
    PCRE2_SIZE erroroffset;
    pcre2_code *re = pcre2_compile((PCRE2_SPTR)pattern->data, (PCRE2_SIZE)pattern->len,
                                   zap_regex_compile_opts(flags), &errorcode, &erroroffset, NULL);
    if (re == NULL) {
        zap_regex_fail("could not compile pattern", errorcode);
    }
    return re;
}

/* `subject =~ m/pattern/flags` → a ZapMatch* owning its captures, or NULL (no
 * match / falsy). */
void *zap_regex_match(void *subject_, void *pattern_, int64_t flags) {
    ZapStr *subject = (ZapStr *)subject_;
    ZapStr *pattern = (ZapStr *)pattern_;
    pcre2_code *re = zap_regex_compile(pattern, flags);
    pcre2_match_data *md = pcre2_match_data_create_from_pattern(re, NULL);
    int rc = pcre2_match(re, (PCRE2_SPTR)subject->data, (PCRE2_SIZE)subject->len, 0, 0, md, NULL);
    if (rc < 0) {
        /* PCRE2_ERROR_NOMATCH (and any other error) => no match. */
        pcre2_match_data_free(md);
        pcre2_code_free(re);
        return NULL;
    }

    PCRE2_SIZE *ov = pcre2_get_ovector_pointer(md);
    uint32_t capcount = 0;
    pcre2_pattern_info(re, PCRE2_INFO_CAPTURECOUNT, &capcount);
    int64_t ngroups = (int64_t)capcount + 1;
    /* rc == 0 would mean the ovector was too small; sized from the pattern it is
     * not, but guard anyway. */
    int64_t avail = rc > 0 ? (int64_t)rc : ngroups;

    ZapMatch *m = (ZapMatch *)zap_obj(&zap_desc_matchobj);
    m->ngroups = ngroups;
    m->groups = (ZapStr **)zap_ptrbuf((int64_t)ngroups * (int64_t)sizeof(ZapStr *));
    for (int64_t i = 0; i < ngroups; i++) {
        PCRE2_SIZE s = ov[2 * i];
        PCRE2_SIZE e = ov[2 * i + 1];
        if (i < avail && s != PCRE2_UNSET && e != PCRE2_UNSET && e >= s) {
            m->groups[i] = (ZapStr *)zap_str_new(subject->data + s, (int64_t)(e - s));
        } else {
            m->groups[i] = NULL;
        }
    }

    /* Copy the name table so lookups work after the compiled program is freed.
     * Each entry is [group-number (2 bytes, big-endian)][name][NUL]. */
    uint32_t namecount = 0, nameentrysize = 0;
    PCRE2_SPTR nametable = NULL;
    pcre2_pattern_info(re, PCRE2_INFO_NAMECOUNT, &namecount);
    pcre2_pattern_info(re, PCRE2_INFO_NAMEENTRYSIZE, &nameentrysize);
    pcre2_pattern_info(re, PCRE2_INFO_NAMETABLE, &nametable);
    m->nnames = (int64_t)namecount;
    if (namecount > 0) {
        m->names = (ZapStr **)zap_ptrbuf((int64_t)namecount * (int64_t)sizeof(ZapStr *));
        m->name_num = (int64_t *)zap_buf((int64_t)namecount * (int64_t)sizeof(int64_t));
        for (uint32_t i = 0; i < namecount; i++) {
            PCRE2_SPTR entry = nametable + (size_t)i * nameentrysize;
            int gnum = (entry[0] << 8) | entry[1];
            const char *nm = (const char *)(entry + 2);
            m->names[i] = (ZapStr *)zap_str_new(nm, (int64_t)strlen(nm));
            m->name_num[i] = gnum;
        }
    } else {
        m->names = NULL;
        m->name_num = NULL;
    }

    pcre2_match_data_free(md);
    pcre2_code_free(re);
    return m;
}

/* `m.group(idx)` — positional capture; 0 is the whole match. Out-of-range or
 * non-participating groups yield the empty string. */
void *zap_regex_group(void *m_, int64_t idx) {
    ZapMatch *m = (ZapMatch *)m_;
    if (m == NULL || idx < 0 || idx >= m->ngroups || m->groups[idx] == NULL) {
        return zap_str_new("", 0);
    }
    return m->groups[idx];
}

/* `m.group("name")` — named capture; unknown or non-participating names yield
 * the empty string. */
void *zap_regex_group_name(void *m_, void *name_) {
    ZapMatch *m = (ZapMatch *)m_;
    ZapStr *name = (ZapStr *)name_;
    if (m != NULL) {
        for (int64_t i = 0; i < m->nnames; i++) {
            if (zap_str_eq(m->names[i], name)) {
                int64_t g = m->name_num[i];
                if (g >= 0 && g < m->ngroups && m->groups[g] != NULL) {
                    return m->groups[g];
                }
                break;
            }
        }
    }
    return zap_str_new("", 0);
}

/* `subject =~ s/pattern/replacement/flags` → a new string (subject unchanged).
 * `g` replaces all occurrences, else the first. Replacement uses PCRE2 syntax
 * for group references (`$1`, `${name}`). */
void *zap_regex_subst(void *subject_, void *pattern_, void *replacement_, int64_t flags) {
    ZapStr *subject = (ZapStr *)subject_;
    ZapStr *pattern = (ZapStr *)pattern_;
    ZapStr *replacement = (ZapStr *)replacement_;
    pcre2_code *re = zap_regex_compile(pattern, flags);

    uint32_t opts = PCRE2_SUBSTITUTE_OVERFLOW_LENGTH;
    if (flags & ZAP_RE_GLOBAL) opts |= PCRE2_SUBSTITUTE_GLOBAL;

    /* OVERFLOW_LENGTH makes a too-small buffer set *outlen to the needed size
     * (including the NUL) and return PCRE2_ERROR_NOMEMORY, so we size, grow,
     * and retry at most once. On success *outlen is the length excluding NUL. */
    PCRE2_SIZE outlen = (PCRE2_SIZE)subject->len + 256;
    PCRE2_UCHAR *buf = (PCRE2_UCHAR *)zap_buf((size_t)outlen);
    int rc = pcre2_substitute(re, (PCRE2_SPTR)subject->data, (PCRE2_SIZE)subject->len, 0, opts,
                              NULL, NULL, (PCRE2_SPTR)replacement->data,
                              (PCRE2_SIZE)replacement->len, buf, &outlen);
    if (rc == PCRE2_ERROR_NOMEMORY) {
        buf = (PCRE2_UCHAR *)zap_buf((size_t)outlen);
        rc = pcre2_substitute(re, (PCRE2_SPTR)subject->data, (PCRE2_SIZE)subject->len, 0, opts,
                              NULL, NULL, (PCRE2_SPTR)replacement->data,
                              (PCRE2_SIZE)replacement->len, buf, &outlen);
    }
    pcre2_code_free(re);
    if (rc < 0) {
        zap_regex_fail("substitution failed", rc);
    }
    return zap_str_new((const char *)buf, (int64_t)outlen);
}
#else /* ZAP_NO_PCRE2 — regex backend (PCRE2) gated off (#853): the program's
       * import graph never reaches a `zap_regex_*` seam, so libpcre2 is not
       * linked and <pcre2.h> is not included. The entry points still lower and
       * link, but each raises a catchable Zap exception if somehow reached —
       * the same shape as the portable CPython stubs (D-0153). */
static void *zap_regex_unavailable(void) {
    const char *m = "regex is unavailable in this build "
                    "(the program's import graph reaches no regex use; see #853)";
    zap_panic(zap_str_new(m, (int64_t)strlen(m)));
    return NULL;
}
void *zap_regex_match(void *subject_, void *pattern_, int64_t flags) {
    (void)subject_;
    (void)pattern_;
    (void)flags;
    return zap_regex_unavailable();
}
void *zap_regex_group(void *m_, int64_t idx) {
    (void)m_;
    (void)idx;
    return zap_regex_unavailable();
}
void *zap_regex_group_name(void *m_, void *name_) {
    (void)m_;
    (void)name_;
    return zap_regex_unavailable();
}
void *zap_regex_subst(void *subject_, void *pattern_, void *replacement_, int64_t flags) {
    (void)subject_;
    (void)pattern_;
    (void)replacement_;
    (void)flags;
    return zap_regex_unavailable();
}
#endif /* ZAP_NO_PCRE2 */

/* ---------------------------------------------------------------------------
 * Python interop (ticket #27, D-0012, design/0003-python-interop.md): embed
 * CPython and marshal values across the Zap <-> Python boundary. The compiler
 * lowers `import python`, attribute/call dispatch, and conversions to the
 * `zap_py_*` entry points below (see crates/ir/src/lower.rs).
 *
 * Representation & GC bridging: a Python value held by Zap is an opaque,
 * GC-managed handle `ZapPy` that OWNS one CPython reference. Capture takes a new
 * reference (the C-API calls here return new references, handed straight to
 * `zap_py_wrap`); a finalizer DECREFs when the wrapper is collected — under Boehm
 * via GC_register_finalizer, under the precise Immix collector via its post-mark
 * finalizer queue (ticket #92), through the one backend-uniform
 * `zap_gc_register_finalizer` call site. The
 * wrapper is GC-scanned, and the ZapList argument vectors that carry handles are
 * GC_MALLOC'd, so a live Zap handle keeps its PyObject alive across collection.
 *
 * GIL: every entry point acquires the GIL (PyGILState_Ensure) around its C-API
 * work and releases it before returning. With the single-threaded runtime of
 * this increment the main thread simply re-acquires it; GIL contention vs.
 * coroutine parallelism (#25) is a documented limitation (design open issue).
 *
 * Exceptions: a Python error is translated to a Zap exception — the error is
 * formatted as "<type>: <message>" into a ZapStr and raised with `zap_panic`,
 * so it propagates and is catchable with `try`/`except` (D-0007, ticket #22).
 *
 * Follow-ups (documented in the PR): list/dict/ndarray marshaling (the design's
 * container rows), keyword arguments, real `PyObject_IsTrue` truthiness, and a
 * thread-safe interpreter init for the coroutine runtime. */

#if defined(ZAP_HAVE_CPYTHON)
typedef struct {
    PyObject *obj; /* owned reference; NULL once finalized */
} ZapPy;

/* Initialize the interpreter exactly once (single-threaded this increment). */
static void zap_py_ensure_init(void) {
    if (!Py_IsInitialized()) {
        Py_InitializeEx(0); /* 0 = do not install signal handlers */
    }
}

/* Boehm finalizer: drop the owned CPython reference when the wrapper dies. */
static void zap_py_finalize(void *obj, void *client_data) {
    (void)client_data;
    ZapPy *h = (ZapPy *)obj;
    if (h->obj != NULL) {
        PyGILState_STATE g = PyGILState_Ensure();
        Py_DECREF(h->obj);
        PyGILState_Release(g);
        h->obj = NULL;
    }
}

/* Wrap a *new* reference `o` (ownership transferred to the handle). */
static ZapPy *zap_py_wrap(PyObject *o) {
    ZapPy *h = (ZapPy *)zap_obj(&zap_desc_pyobj);
    h->obj = o;
    /* Backend-uniform: Boehm forwards to GC_register_finalizer; the precise Immix
     * collector runs this via its own post-mark finalizer queue (ticket #92). */
    zap_gc_register_finalizer(h, zap_py_finalize, NULL);
    return h;
}

/* NUL-terminated copy of a ZapStr (for the C-string CPython APIs). */
static char *zap_py_cstr(const ZapStr *s) {
    char *buf = (char *)zap_buf((size_t)s->len + 1);
    if (s->len > 0) {
        memcpy(buf, s->data, (size_t)s->len);
    }
    buf[s->len] = '\0';
    return buf;
}

/* Translate the current Python error to a Zap exception and raise it. Releases
 * the GIL first (the caller passes its gstate), then raises the mapped Zap type,
 * unwinds and never returns. */
static int64_t zap_py_exception_type_id(const char *name) {
    const char *short_name = strrchr(name, '.');
    if (short_name != NULL) name = short_name + 1;
    if (strcmp(name, "ValueError") == 0) return ZAP_EXC_VALUE_ERROR;
    if (strcmp(name, "TypeError") == 0) return ZAP_EXC_TYPE_ERROR;
    if (strcmp(name, "IndexError") == 0) return ZAP_EXC_INDEX_ERROR;
    if (strcmp(name, "KeyError") == 0) return ZAP_EXC_KEY_ERROR;
    if (strcmp(name, "ZeroDivisionError") == 0) return ZAP_EXC_ZERO_DIVISION_ERROR;
    if (strcmp(name, "ArithmeticError") == 0) return ZAP_EXC_ARITHMETIC_ERROR;
    if (strcmp(name, "LookupError") == 0) return ZAP_EXC_LOOKUP_ERROR;
    if (strcmp(name, "RuntimeError") == 0) return ZAP_EXC_RUNTIME_ERROR;
    if (strcmp(name, "ExceptionGroup") == 0) return ZAP_EXC_EXCEPTION_GROUP;
    if (strcmp(name, "ScopeTimeout") == 0) return ZAP_EXC_SCOPE_TIMEOUT;
    if (strcmp(name, "TimeoutError") == 0) return ZAP_EXC_TIMEOUT_ERROR;
    if (strcmp(name, "AssertionError") == 0) return ZAP_EXC_ASSERTION_ERROR;
    if (strcmp(name, "BaseException") == 0) return ZAP_EXC_BASE;
    return ZAP_EXC_EXCEPTION;
}

static void zap_py_raise(PyGILState_STATE g) {
    PyObject *type = NULL, *value = NULL, *tb = NULL;
    PyErr_Fetch(&type, &value, &tb);
    PyErr_NormalizeException(&type, &value, &tb);
    const char *tname =
        (type != NULL) ? ((PyTypeObject *)type)->tp_name : "PythonError";
    ZapStr *msg = (ZapStr *)zap_str_new(tname, (int64_t)strlen(tname));
    if (value != NULL) {
        PyObject *s = PyObject_Str(value);
        if (s != NULL) {
            Py_ssize_t n = 0;
            const char *u = PyUnicode_AsUTF8AndSize(s, &n);
            ZapStr *sep = (ZapStr *)zap_str_new(": ", 2);
            ZapStr *body = (ZapStr *)zap_str_new(u ? u : "", (int64_t)(u ? n : 0));
            msg = (ZapStr *)zap_str_concat(zap_str_concat(msg, sep), body);
            Py_DECREF(s);
        }
    }
    int64_t type_id = zap_py_exception_type_id(tname);
    Py_XDECREF(type);
    Py_XDECREF(value);
    Py_XDECREF(tb);
    PyGILState_Release(g);
    zap_panic_typed(type_id, msg);
}

/* `import python "module"` → a module handle. */
void *zap_py_import(void *name_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    char *name = zap_py_cstr((ZapStr *)name_);
    PyObject *m = PyImport_ImportModule(name);
    if (m == NULL) {
        zap_py_raise(g);
    }
    ZapPy *out = zap_py_wrap(m);
    PyGILState_Release(g);
    return out;
}

/* `obj.attr` on a Python handle → the attribute value (another handle). */
void *zap_py_getattr(void *obj_, void *name_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    ZapPy *obj = (ZapPy *)obj_;
    char *name = zap_py_cstr((ZapStr *)name_);
    PyObject *a = PyObject_GetAttrString(obj->obj, name);
    if (a == NULL) {
        zap_py_raise(g);
    }
    ZapPy *out = zap_py_wrap(a);
    PyGILState_Release(g);
    return out;
}

/* Call a Python callable with positional args collected in a ZapList of ZapPy*
 * handle words (built by the lowering). Returns the result as a handle. */
void *zap_py_call(void *callable_, void *arglist_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    ZapPy *callable = (ZapPy *)callable_;
    ZapList *argl = (ZapList *)arglist_;
    int64_t n = (argl != NULL) ? argl->len : 0;
    PyObject *tuple = PyTuple_New((Py_ssize_t)n);
    if (tuple == NULL) {
        zap_py_raise(g);
    }
    for (int64_t i = 0; i < n; i++) {
        ZapPy *h = (ZapPy *)(intptr_t)argl->data[i];
        PyObject *o = h->obj;
        Py_INCREF(o); /* PyTuple_SET_ITEM steals the reference */
        PyTuple_SET_ITEM(tuple, (Py_ssize_t)i, o);
    }
    PyObject *r = PyObject_CallObject(callable->obj, tuple);
    Py_DECREF(tuple);
    if (r == NULL) {
        zap_py_raise(g);
    }
    ZapPy *out = zap_py_wrap(r);
    PyGILState_Release(g);
    return out;
}

/* --- marshaling: Zap scalar/str -> Python (the handle owns the new ref). --- */
void *zap_py_from_i64(int64_t v) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *o = PyLong_FromLongLong((long long)v);
    if (o == NULL) {
        zap_py_raise(g);
    }
    ZapPy *out = zap_py_wrap(o);
    PyGILState_Release(g);
    return out;
}
void *zap_py_from_f64(double v) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *o = PyFloat_FromDouble(v);
    if (o == NULL) {
        zap_py_raise(g);
    }
    ZapPy *out = zap_py_wrap(o);
    PyGILState_Release(g);
    return out;
}
void *zap_py_from_bool(int64_t v) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    PyObject *o = PyBool_FromLong((long)v); /* new ref to Py_True/Py_False */
    ZapPy *out = zap_py_wrap(o);
    PyGILState_Release(g);
    return out;
}
void *zap_py_from_str(void *s_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    ZapStr *s = (ZapStr *)s_;
    PyObject *o = PyUnicode_FromStringAndSize(s->data, (Py_ssize_t)s->len);
    if (o == NULL) {
        zap_py_raise(g);
    }
    ZapPy *out = zap_py_wrap(o);
    PyGILState_Release(g);
    return out;
}

/* --- marshaling: Python -> Zap scalar/str. --- */
int64_t zap_py_to_i64(void *obj_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    ZapPy *obj = (ZapPy *)obj_;
    PyObject *n = PyNumber_Long(obj->obj); /* accepts int and float */
    if (n == NULL) {
        zap_py_raise(g);
    }
    long long v = PyLong_AsLongLong(n);
    Py_DECREF(n);
    if (v == -1 && PyErr_Occurred()) {
        zap_py_raise(g);
    }
    PyGILState_Release(g);
    return (int64_t)v;
}
double zap_py_to_f64(void *obj_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    ZapPy *obj = (ZapPy *)obj_;
    double v = PyFloat_AsDouble(obj->obj); /* accepts int and float */
    if (v == -1.0 && PyErr_Occurred()) {
        zap_py_raise(g);
    }
    PyGILState_Release(g);
    return v;
}

/* `str(obj)` as a Zap string (used by `print` of a Python handle). */
void *zap_py_str(void *obj_) {
    zap_py_ensure_init();
    PyGILState_STATE g = PyGILState_Ensure();
    ZapPy *obj = (ZapPy *)obj_;
    PyObject *s = PyObject_Str(obj->obj);
    if (s == NULL) {
        zap_py_raise(g);
    }
    Py_ssize_t n = 0;
    const char *u = PyUnicode_AsUTF8AndSize(s, &n);
    ZapStr *out = (ZapStr *)zap_str_new(u ? u : "", (int64_t)(u ? n : 0));
    Py_DECREF(s);
    PyGILState_Release(g);
    return out;
}
#else /* !ZAP_HAVE_CPYTHON — CPython embedding gated off: Windows v1 (I#198) or
       * the portable build profile (`--portable`, I#287), which drops libpython
       * for a self-contained AL2023-runnable binary. */
/* `import python` and its attribute/call/convert entry points still lower and
 * link, but CPython is not embedded (no python3-embed under MinGW-w64, or the
 * portable profile deliberately omits it, see build_executable), so each raises a
 * catchable Zap exception at runtime rather than failing the link. The wrapper
 * type keeps its shape so the shared `zap_desc_pyobj` descriptor (sizeof)
 * compiles. Full Windows Python interop is a post-v1 follow-up. */
typedef struct {
    void *obj;
} ZapPy;
static void *zap_py_unavailable(void) {
    const char *m = "python interop is unavailable in this build "
                    "(portable profile, see I#287; or Windows v1, see I#198)";
    zap_panic(zap_str_new(m, (int64_t)strlen(m)));
    return NULL;
}
void *zap_py_import(void *name_) {
    (void)name_;
    return zap_py_unavailable();
}
void *zap_py_getattr(void *obj_, void *name_) {
    (void)obj_;
    (void)name_;
    return zap_py_unavailable();
}
void *zap_py_call(void *callable_, void *arglist_) {
    (void)callable_;
    (void)arglist_;
    return zap_py_unavailable();
}
void *zap_py_from_i64(int64_t v) {
    (void)v;
    return zap_py_unavailable();
}
void *zap_py_from_f64(double v) {
    (void)v;
    return zap_py_unavailable();
}
void *zap_py_from_bool(int64_t v) {
    (void)v;
    return zap_py_unavailable();
}
void *zap_py_from_str(void *s_) {
    (void)s_;
    return zap_py_unavailable();
}
int64_t zap_py_to_i64(void *obj_) {
    (void)obj_;
    zap_py_unavailable();
    return 0;
}
double zap_py_to_f64(void *obj_) {
    (void)obj_;
    zap_py_unavailable();
    return 0;
}
void *zap_py_str(void *obj_) {
    (void)obj_;
    return zap_py_unavailable();
}
#endif /* ZAP_HAVE_CPYTHON */

