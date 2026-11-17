/* zap runtime fragment: json.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * json — JSON tokenizer cursor (ticket #100 / #131, D-0059)
 *
 * A pull lexer over an in-memory str/bytes source. The high-level tree build
 * (json.loads) and serialization (json.dumps) live in Zap prelude functions
 * (see PRELUDE_SRC) that drive this cursor through the internal `json._lex_*`
 * seam primitives — so JSON<->JsonValue construction stays in compiler-emitted
 * code and this C never needs to know the JsonValue struct layout.
 *
 * The cursor is a pointer-free leaf (zap_buf): it keeps a raw pointer into the
 * source ZapStr's byte buffer (the caller keeps the source live across the
 * parse), plus the current token's kind and decoded scalar value.
 * ======================================================================== */
#include <errno.h>

enum {
    ZAP_JTOK_END = 0,
    ZAP_JTOK_NULL = 1,
    ZAP_JTOK_TRUE = 2,
    ZAP_JTOK_FALSE = 3,
    ZAP_JTOK_INT = 4,
    ZAP_JTOK_FLOAT = 5,
    ZAP_JTOK_STR = 6,
    ZAP_JTOK_LBRACE = 7,
    ZAP_JTOK_RBRACE = 8,
    ZAP_JTOK_LBRACKET = 9,
    ZAP_JTOK_RBRACKET = 10,
    ZAP_JTOK_COLON = 11,
    ZAP_JTOK_COMMA = 12,
    ZAP_JTOK_ERR = -1
};

typedef struct {
    const char *data;
    int64_t len;
    int64_t pos;
    int64_t kind;      /* current token kind (ZAP_JTOK_*) */
    int64_t ival;      /* ZAP_JTOK_INT value */
    double fval;       /* ZAP_JTOK_FLOAT value */
    const char *sdata; /* ZAP_JTOK_STR: start of raw content (inside the quotes) */
    int64_t slen;      /* ZAP_JTOK_STR: raw content length */
    int64_t has_esc;   /* ZAP_JTOK_STR: content contains backslash escapes */
} ZapJsonLex;

/* Open a cursor over a str/bytes source (a ZapStr*). */
/* Validate a complete RFC 8259 document before exposing a cursor. The prelude
 * builder relies on these grammar invariants; rejecting here also prevents its
 * truncated-array EOF loop. No allocation or GC safepoint during validation.
 * Duplicate object names retain the existing last-value-wins DOM policy. */
typedef struct {
    const unsigned char *data;
    int64_t len;
    int64_t pos;
} ZapJsonCheck;

static void zap_json_check_ws(ZapJsonCheck *s) {
    while (s->pos < s->len) {
        unsigned char c = s->data[s->pos];
        if (c != ' ' && c != '\t' && c != '\n' && c != '\r') break;
        s->pos++;
    }
}

static int zap_json_hex4(const unsigned char *d, int64_t n, unsigned *out) {
    if (n < 4) return 0;
    unsigned v = 0;
    for (int k = 0; k < 4; k++) {
        unsigned c = d[k];
        unsigned x;
        if (c >= '0' && c <= '9') x = c - '0';
        else if (c >= 'a' && c <= 'f') x = c - 'a' + 10;
        else if (c >= 'A' && c <= 'F') x = c - 'A' + 10;
        else return 0;
        v = (v << 4) | x;
    }
    *out = v;
    return 1;
}

static int zap_json_check_string(ZapJsonCheck *s) {
    if (s->pos >= s->len || s->data[s->pos++] != '"') return 0;
    while (s->pos < s->len) {
        unsigned c = s->data[s->pos++];
        if (c == '"') return 1;
        if (c < 0x20) return 0;
        if (c != '\\') continue;
        if (s->pos == s->len) return 0;
        c = s->data[s->pos++];
        if (c == '"' || c == '\\' || c == '/' || c == 'b' || c == 'f' ||
            c == 'n' || c == 'r' || c == 't') continue;
        if (c != 'u') return 0;
        unsigned cp;
        if (!zap_json_hex4(s->data + s->pos, s->len - s->pos, &cp)) return 0;
        s->pos += 4;
        if (cp >= 0xDC00 && cp <= 0xDFFF) return 0;
        if (cp >= 0xD800 && cp <= 0xDBFF) {
            if (s->len - s->pos < 6 || s->data[s->pos] != '\\' ||
                s->data[s->pos + 1] != 'u') return 0;
            unsigned low;
            if (!zap_json_hex4(s->data + s->pos + 2, s->len - s->pos - 2, &low) ||
                low < 0xDC00 || low > 0xDFFF) return 0;
            s->pos += 6;
        }
    }
    return 0;
}

static int zap_json_check_value(ZapJsonCheck *s, int depth) {
    zap_json_check_ws(s);
    if (s->pos == s->len) return 0;
    unsigned c = s->data[s->pos];
    if (c == '"') return zap_json_check_string(s);
    if (c == '{' || c == '[') {
        if (depth == 128) return 0;
        int object = c == '{';
        unsigned close = object ? '}' : ']';
        s->pos++;
        zap_json_check_ws(s);
        if (s->pos < s->len && s->data[s->pos] == close) {
            s->pos++;
            return 1;
        }
        for (;;) {
            if (object) {
                if (!zap_json_check_string(s)) return 0;
                zap_json_check_ws(s);
                if (s->pos == s->len || s->data[s->pos++] != ':') return 0;
            }
            if (!zap_json_check_value(s, depth + 1)) return 0;
            zap_json_check_ws(s);
            if (s->pos == s->len) return 0;
            c = s->data[s->pos++];
            if (c == close) return 1;
            if (c != ',') return 0;
            zap_json_check_ws(s);
            /* No empty slot or trailing comma: next iteration requires value/key. */
        }
    }
    const char *literal = NULL;
    int64_t length = 0;
    if (c == 'n') { literal = "null"; length = 4; }
    if (c == 't') { literal = "true"; length = 4; }
    if (c == 'f') { literal = "false"; length = 5; }
    if (literal) {
        if (s->len - s->pos < length ||
            memcmp(s->data + s->pos, literal, (size_t)length) != 0) return 0;
        s->pos += length;
        return 1;
    }
    /* number = -? (0 | [1-9][0-9]*) (.[0-9]+)? ([eE][+-]?[0-9]+)? */
    if (c == '-') s->pos++;
    if (s->pos == s->len) return 0;
    c = s->data[s->pos];
    if (c == '0') s->pos++;
    else {
        if (c < '1' || c > '9') return 0;
        do { s->pos++; }
        while (s->pos < s->len && s->data[s->pos] >= '0' && s->data[s->pos] <= '9');
    }
    if (s->pos < s->len && s->data[s->pos] == '.') {
        s->pos++;
        int64_t start = s->pos;
        while (s->pos < s->len && s->data[s->pos] >= '0' && s->data[s->pos] <= '9') s->pos++;
        if (s->pos == start) return 0;
    }
    if (s->pos < s->len && (s->data[s->pos] == 'e' || s->data[s->pos] == 'E')) {
        s->pos++;
        if (s->pos < s->len && (s->data[s->pos] == '+' || s->data[s->pos] == '-')) s->pos++;
        int64_t start = s->pos;
        while (s->pos < s->len && s->data[s->pos] >= '0' && s->data[s->pos] <= '9') s->pos++;
        if (s->pos == start) return 0;
    }
    return 1;
}

static int zap_json_document_valid(const unsigned char *d, int64_t n) {
    ZapJsonCheck s = { d, n, 0 };
    int64_t bad;
    if (!zap_utf8_valid(d, n, &bad) || !zap_json_check_value(&s, 0)) return 0;
    zap_json_check_ws(&s);
    return s.pos == s.len;
}

void *zap_json_lex_new(void *s) {
    ZapStr *z = (ZapStr *)s;
    if (!zap_json_document_valid((const unsigned char *)z->data, z->len)) {
        zap_panic(zap_str_new("invalid JSON document or nesting exceeds 128", 44));
    }
    ZapJsonLex *lx = (ZapJsonLex *)zap_buf((int64_t)sizeof(ZapJsonLex));
    lx->data = z->data;
    lx->len = z->len;
    lx->pos = 0;
    lx->kind = ZAP_JTOK_END;
    lx->ival = 0;
    lx->fval = 0.0;
    lx->sdata = NULL;
    lx->slen = 0;
    lx->has_esc = 0;
    return lx;
}

static int zap_json_is_ws(char c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r';
}

/* Advance to and classify the next token; returns (and stores) its kind. */
int64_t zap_json_lex_next(void *h) {
    ZapJsonLex *lx = (ZapJsonLex *)h;
    const char *d = lx->data;
    int64_t n = lx->len;
    int64_t i = lx->pos;
    while (i < n && zap_json_is_ws(d[i])) {
        i++;
    }
    if (i >= n) {
        lx->pos = i;
        lx->kind = ZAP_JTOK_END;
        return ZAP_JTOK_END;
    }
    char c = d[i];
    switch (c) {
    case '{': lx->pos = i + 1; lx->kind = ZAP_JTOK_LBRACE; return lx->kind;
    case '}': lx->pos = i + 1; lx->kind = ZAP_JTOK_RBRACE; return lx->kind;
    case '[': lx->pos = i + 1; lx->kind = ZAP_JTOK_LBRACKET; return lx->kind;
    case ']': lx->pos = i + 1; lx->kind = ZAP_JTOK_RBRACKET; return lx->kind;
    case ':': lx->pos = i + 1; lx->kind = ZAP_JTOK_COLON; return lx->kind;
    case ',': lx->pos = i + 1; lx->kind = ZAP_JTOK_COMMA; return lx->kind;
    case '"': {
        int64_t start = i + 1;
        int64_t j = start;
        int64_t esc = 0;
        while (j < n && d[j] != '"') {
            if (d[j] == '\\') {
                esc = 1;
                j += (j + 1 < n) ? 2 : 1;
            } else {
                j++;
            }
        }
        lx->sdata = d + start;
        lx->slen = (j <= n ? j : n) - start;
        lx->has_esc = esc;
        lx->kind = ZAP_JTOK_STR;
        lx->pos = (j < n) ? j + 1 : n; /* consume the closing quote */
        return lx->kind;
    }
    default:
        break;
    }
    if (c == 'n' && i + 4 <= n && memcmp(d + i, "null", 4) == 0) {
        lx->pos = i + 4;
        lx->kind = ZAP_JTOK_NULL;
        return lx->kind;
    }
    if (c == 't' && i + 4 <= n && memcmp(d + i, "true", 4) == 0) {
        lx->pos = i + 4;
        lx->kind = ZAP_JTOK_TRUE;
        return lx->kind;
    }
    if (c == 'f' && i + 5 <= n && memcmp(d + i, "false", 5) == 0) {
        lx->pos = i + 5;
        lx->kind = ZAP_JTOK_FALSE;
        return lx->kind;
    }
    if (c == '-' || (c >= '0' && c <= '9')) {
        int64_t start = i;
        int is_float = 0;
        if (d[i] == '-') {
            i++;
        }
        while (i < n && d[i] >= '0' && d[i] <= '9') {
            i++;
        }
        if (i < n && d[i] == '.') {
            is_float = 1;
            i++;
            while (i < n && d[i] >= '0' && d[i] <= '9') {
                i++;
            }
        }
        if (i < n && (d[i] == 'e' || d[i] == 'E')) {
            is_float = 1;
            i++;
            if (i < n && (d[i] == '+' || d[i] == '-')) {
                i++;
            }
            while (i < n && d[i] >= '0' && d[i] <= '9') {
                i++;
            }
        }
        int64_t tlen = i - start;
        char buf[80];
        int64_t cl = tlen < 79 ? tlen : 79;
        memcpy(buf, d + start, (size_t)cl);
        buf[cl] = 0;
        if (!is_float) {
            errno = 0;
            char *end = NULL;
            long long v = strtoll(buf, &end, 10);
            if (errno == 0 && end == buf + cl) {
                lx->ival = (int64_t)v;
                lx->kind = ZAP_JTOK_INT;
                lx->pos = i;
                return lx->kind;
            }
            /* overflow / out-of-i64: fall back to float (D-0059) */
        }
        lx->fval = strtod(buf, NULL);
        lx->kind = ZAP_JTOK_FLOAT;
        lx->pos = i;
        return lx->kind;
    }
    lx->pos = i + 1;
    lx->kind = ZAP_JTOK_ERR;
    return lx->kind;
}

int64_t zap_json_lex_int(void *h) { return ((ZapJsonLex *)h)->ival; }
double zap_json_lex_float(void *h) { return ((ZapJsonLex *)h)->fval; }

/* Build a ZapStr from the current STR token, decoding escapes if present.
 * Escapes never expand: a 6-byte `\uXXXX` decodes to at most 3 UTF-8 bytes, and
 * a 12-byte surrogate pair `\uXXXX\uXXXX` decodes to a 4-byte astral scalar. So
 * the decoded length never exceeds the escaped source length, and the raw token
 * length is a safe output capacity. */
void *zap_json_lex_str(void *h) {
    ZapJsonLex *lx = (ZapJsonLex *)h;
    const char *s = lx->sdata;
    int64_t n = lx->slen;
    if (!lx->has_esc) {
        return zap_str_new(s, n);
    }
    char *out = (char *)zap_buf(n > 0 ? n : 1);
    int64_t o = 0;
    for (int64_t i = 0; i < n; i++) {
        char c = s[i];
        if (c == '\\' && i + 1 < n) {
            char e = s[++i];
            switch (e) {
            case 'n': out[o++] = '\n'; break;
            case 't': out[o++] = '\t'; break;
            case 'r': out[o++] = '\r'; break;
            case 'b': out[o++] = '\b'; break;
            case 'f': out[o++] = '\f'; break;
            case '/': out[o++] = '/'; break;
            case '\\': out[o++] = '\\'; break;
            case '"': out[o++] = '"'; break;
            case 'u': {
                if (i + 4 < n) {
                    unsigned int cp = 0;
                    for (int k = 0; k < 4; k++) {
                        char hx = s[i + 1 + k];
                        cp <<= 4;
                        if (hx >= '0' && hx <= '9') {
                            cp |= (unsigned)(hx - '0');
                        } else if (hx >= 'a' && hx <= 'f') {
                            cp |= (unsigned)(hx - 'a' + 10);
                        } else if (hx >= 'A' && hx <= 'F') {
                            cp |= (unsigned)(hx - 'A' + 10);
                        }
                    }
                    i += 4;
                    if (cp >= 0xD800 && cp <= 0xDBFF && i + 6 < n) {
                        unsigned int low;
                        if (s[i + 1] == '\\' && s[i + 2] == 'u' &&
                            zap_json_hex4((const unsigned char *)s + i + 3, n - i - 3, &low) &&
                            low >= 0xDC00 && low <= 0xDFFF) {
                            cp = 0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00);
                            i += 6;
                        }
                    }
                    if (cp >= 0xD800 && cp <= 0xDFFF) {
                        /* A lone surrogate would encode as invalid WTF-8. The
                         * pre-validator (zap_json_document_valid) pairs every high
                         * surrogate and rejects lone surrogates, so this is
                         * unreachable for a validated document; raise a catchable
                         * error rather than emit malformed UTF-8 if the cursor is
                         * ever reached without that guarantee. */
                        zap_panic(zap_str_new("invalid JSON surrogate escape", 29));
                    }
                    o += zap_utf8_encode((int32_t)cp, out + o);
                }
                break;
            }
            default: out[o++] = e; break;
            }
        } else {
            out[o++] = c;
        }
    }
    return zap_str_new(out, o);
}

/* --- json.dumps helpers (ticket #100 / #131) ---------------------------------
 * The serializer (`__json_dumps`) is a Zap prelude walker that emits each
 * fragment into a list[str] and joins once; these C helpers cover the pieces
 * Zap lacks a direct surface for: enumerating an object's keys, JSON string
 * escaping, and the final concatenation. Object key order is unspecified (hash
 * order), matching D-0059. */

/* Collect a dict's keys (str/ptr keys) into a fresh list, for `for k in keys`. */
void *zap_dict_keys(void *dp) {
    ZapDict *d = (ZapDict *)dp;
    void *out = zap_list_new();
    uint8_t *tags = zap_d_tags(d);
    int64_t *keys = zap_d_keys(d);
    for (int64_t i = 0; i < d->cap; i++) {
        if (tags[i] >= 2) { /* full slot */
            zap_list_push_ptr(out, (void *)(intptr_t)keys[i]);
        }
    }
    return out;
}

/* JSON-escape a string and wrap it in double quotes (RFC 8259 §7). */
void *zap_json_escape(void *sp) {
    ZapStr *s = (ZapStr *)sp;
    int64_t n = s->len;
    const char *in = s->data;
    /* worst case per byte: \u00XX (6); plus the two surrounding quotes. */
    char *out = (char *)zap_buf(n * 6 + 2);
    int64_t o = 0;
    static const char hex[] = "0123456789abcdef";
    out[o++] = '"';
    for (int64_t i = 0; i < n; i++) {
        unsigned char c = (unsigned char)in[i];
        switch (c) {
        case '"': out[o++] = '\\'; out[o++] = '"'; break;
        case '\\': out[o++] = '\\'; out[o++] = '\\'; break;
        case '\n': out[o++] = '\\'; out[o++] = 'n'; break;
        case '\t': out[o++] = '\\'; out[o++] = 't'; break;
        case '\r': out[o++] = '\\'; out[o++] = 'r'; break;
        case '\b': out[o++] = '\\'; out[o++] = 'b'; break;
        case '\f': out[o++] = '\\'; out[o++] = 'f'; break;
        default:
            if (c < 0x20) {
                out[o++] = '\\';
                out[o++] = 'u';
                out[o++] = '0';
                out[o++] = '0';
                out[o++] = hex[(c >> 4) & 0xF];
                out[o++] = hex[c & 0xF];
            } else {
                out[o++] = (char)c;
            }
        }
    }
    out[o++] = '"';
    return zap_str_new(out, o);
}

/* Python-style `repr` of a string (#274, D-0124). Single-quoted by default;
 * double-quoted only when the string contains a single quote but no double quote
 * (CPython's quote-selection rule). The active quote and backslash are escaped,
 * `\n`/`\r`/`\t` use their short forms, other C0 controls and DEL use `\xNN`, and
 * bytes >= 0x80 (UTF-8 continuation/lead bytes) pass through so printable Unicode
 * shows as itself. Used when a string appears as a struct field, container
 * element, or tuple element (where Python shows repr), NOT at the top level of
 * print()/str(). (Documented v1 simplification vs. CPython: non-printable
 * non-ASCII codepoints are not escaped — the bytes pass through.) */
void *zap_str_repr(void *sp) {
    ZapStr *s = (ZapStr *)sp;
    int64_t n = s->len;
    const char *in = s->data;
    int has_single = 0, has_double = 0;
    for (int64_t i = 0; i < n; i++) {
        unsigned char c = (unsigned char)in[i];
        if (c == '\'') has_single = 1;
        else if (c == '"') has_double = 1;
    }
    char q = (has_single && !has_double) ? '"' : '\'';
    /* worst case per byte: \xNN (4); plus the two surrounding quotes. */
    char *out = (char *)zap_buf(n * 4 + 2);
    int64_t o = 0;
    static const char hex[] = "0123456789abcdef";
    out[o++] = q;
    for (int64_t i = 0; i < n; i++) {
        unsigned char c = (unsigned char)in[i];
        if (c == (unsigned char)q || c == '\\') {
            out[o++] = '\\';
            out[o++] = (char)c;
        } else if (c == '\n') {
            out[o++] = '\\';
            out[o++] = 'n';
        } else if (c == '\r') {
            out[o++] = '\\';
            out[o++] = 'r';
        } else if (c == '\t') {
            out[o++] = '\\';
            out[o++] = 't';
        } else if (c < 0x20 || c == 0x7f) {
            out[o++] = '\\';
            out[o++] = 'x';
            out[o++] = hex[(c >> 4) & 0xF];
            out[o++] = hex[c & 0xF];
        } else {
            out[o++] = (char)c;
        }
    }
    out[o++] = q;
    return zap_str_new(out, o);
}

/* Python-style `repr` of a bytes value (#274, D-0124): the `b'…'` form. Same
 * quote-selection as a string; printable ASCII (0x20..0x7e) passes through
 * except the active quote and backslash; `\t`/`\n`/`\r` use short escapes; every
 * other byte (including all >= 0x80) uses `\xNN` — matching CPython's `bytes`
 * repr, which never passes high bytes through. `bytes` shares the `ZapStr`
 * {len,data} layout. */
void *zap_bytes_repr(void *bp) {
    ZapStr *s = (ZapStr *)bp;
    int64_t n = s->len;
    const char *in = s->data;
    int has_single = 0, has_double = 0;
    for (int64_t i = 0; i < n; i++) {
        unsigned char c = (unsigned char)in[i];
        if (c == '\'') has_single = 1;
        else if (c == '"') has_double = 1;
    }
    char q = (has_single && !has_double) ? '"' : '\'';
    /* worst case per byte: \xNN (4); plus the `b` prefix and two quotes. */
    char *out = (char *)zap_buf(n * 4 + 3);
    int64_t o = 0;
    static const char hex[] = "0123456789abcdef";
    out[o++] = 'b';
    out[o++] = q;
    for (int64_t i = 0; i < n; i++) {
        unsigned char c = (unsigned char)in[i];
        if (c == (unsigned char)q || c == '\\') {
            out[o++] = '\\';
            out[o++] = (char)c;
        } else if (c == '\t') {
            out[o++] = '\\';
            out[o++] = 't';
        } else if (c == '\n') {
            out[o++] = '\\';
            out[o++] = 'n';
        } else if (c == '\r') {
            out[o++] = '\\';
            out[o++] = 'r';
        } else if (c >= 0x20 && c <= 0x7e) {
            out[o++] = (char)c;
        } else {
            out[o++] = '\\';
            out[o++] = 'x';
            out[o++] = hex[(c >> 4) & 0xF];
            out[o++] = hex[c & 0xF];
        }
    }
    out[o++] = q;
    return zap_str_new(out, o);
}

/* Concatenate every ZapStr* element of a list into one string. */
void *zap_json_join(void *lp) {
    ZapList *l = (ZapList *)lp;
    int64_t total = 0;
    for (int64_t i = 0; i < l->len; i++) {
        ZapStr *part = (ZapStr *)zap_list_get_ptr(lp, i);
        total += part->len;
    }
    char *out = (char *)zap_buf(total > 0 ? total : 1);
    int64_t o = 0;
    for (int64_t i = 0; i < l->len; i++) {
        ZapStr *part = (ZapStr *)zap_list_get_ptr(lp, i);
        if (part->len > 0) {
            memcpy(out + o, part->data, (size_t)part->len);
            o += part->len;
        }
    }
    return zap_str_new(out, o);
}

