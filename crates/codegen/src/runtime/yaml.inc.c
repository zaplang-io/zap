/* zap runtime fragment: yaml.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * yaml — subset YAML -> JSON transpiler (ticket #149, D-0063)
 * ---------------------------------------------------------------------------
 * `yaml.load` reuses the existing JsonValue DOM: YAML 1.2 is a JSON superset,
 * so rather than invent a parallel DOM we transpile the supported YAML subset
 * to a JSON string here in C, then hand it to the json tokenizer + the Zap
 * prelude tree builder (`__yaml_loads(s) = __json_loads(yaml._to_json(s))`).
 * This keeps all JsonValue layout knowledge out of the runtime (the json
 * precedent) and keeps the core dependency-free (no libyaml).
 *
 * Supported (v1): block mappings + block sequences (incl. sequences of
 * mappings via the `- key: val` form), JSON-compatible flow style (`{...}` /
 * `[...]` passed through verbatim), plain / single- / double-quoted scalars,
 * `#` comments, a leading `---`/`...` document marker, and the standard scalar
 * coercions (null/~, true/false, int, float, else string). Indentation must be
 * spaces (a tab in indentation is rejected). Deferred (see the PR / DECISIONS):
 * anchors & aliases, tags (`!!`), merge keys (`<<`), multi-doc streams, block
 * scalars (`|` / `>`), complex keys, directives.
 * ======================================================================== */

/* One preprocessed logical line: content with indentation stripped, trailing
 * comment removed, and right-trimmed. `p` points into the source ZapStr (kept
 * live by the caller across the transpile). */
typedef struct {
    const char *p;
    int64_t len;
    int64_t indent;
} ZapYamlLine;

/* A growable malloc output buffer for the emitted JSON text. */
typedef struct {
    char *buf;
    int64_t len;
    int64_t cap;
} ZapYamlOut;

static void zap_yaml_fault(const char *msg) {
    zap_panic(zap_str_new(msg, (int64_t)strlen(msg)));
}

static void yo_reserve(ZapYamlOut *o, int64_t extra) {
    if (o->len + extra <= o->cap) {
        return;
    }
    int64_t cap = o->cap > 0 ? o->cap : 64;
    while (o->len + extra > cap) {
        cap *= 2;
    }
    char *nb = (char *)realloc(o->buf, (size_t)cap);
    if (!nb) {
        free(o->buf);
        zap_yaml_fault("yaml: out of memory");
        return;
    }
    o->buf = nb;
    o->cap = cap;
}

static void yo_putc(ZapYamlOut *o, char c) {
    yo_reserve(o, 1);
    o->buf[o->len++] = c;
}

static void yo_putn(ZapYamlOut *o, const char *s, int64_t n) {
    if (n <= 0) {
        return;
    }
    yo_reserve(o, n);
    memcpy(o->buf + o->len, s, (size_t)n);
    o->len += n;
}

static void yo_puts(ZapYamlOut *o, const char *s) { yo_putn(o, s, (int64_t)strlen(s)); }

/* Emit `s[0..n)` as a JSON string literal (quoted + escaped). */
static void yaml_emit_jstr(ZapYamlOut *o, const char *s, int64_t n) {
    static const char hex[] = "0123456789abcdef";
    yo_putc(o, '"');
    for (int64_t i = 0; i < n; i++) {
        unsigned char c = (unsigned char)s[i];
        switch (c) {
        case '"': yo_putc(o, '\\'); yo_putc(o, '"'); break;
        case '\\': yo_putc(o, '\\'); yo_putc(o, '\\'); break;
        case '\n': yo_putc(o, '\\'); yo_putc(o, 'n'); break;
        case '\t': yo_putc(o, '\\'); yo_putc(o, 't'); break;
        case '\r': yo_putc(o, '\\'); yo_putc(o, 'r'); break;
        default:
            if (c < 0x20) {
                yo_putc(o, '\\');
                yo_putc(o, 'u');
                yo_putc(o, '0');
                yo_putc(o, '0');
                yo_putc(o, hex[(c >> 4) & 0xF]);
                yo_putc(o, hex[c & 0xF]);
            } else {
                yo_putc(o, (char)c);
            }
        }
    }
    yo_putc(o, '"');
}

/* Emit a YAML single-quoted scalar `s[0..n)` (including the surrounding
 * quotes) as a JSON string. A doubled `''` is a literal single quote. */
static void yaml_emit_squoted(ZapYamlOut *o, const char *s, int64_t n) {
    /* s[0] == '\'' ; scan content until a lone closing quote. */
    int64_t i = 1;
    ZapYamlOut tmp = {0};
    while (i < n) {
        char c = s[i];
        if (c == '\'') {
            if (i + 1 < n && s[i + 1] == '\'') {
                yo_putc(&tmp, '\'');
                i += 2;
                continue;
            }
            break; /* closing quote */
        }
        yo_putc(&tmp, c);
        i++;
    }
    yaml_emit_jstr(o, tmp.buf ? tmp.buf : "", tmp.len);
    free(tmp.buf);
}

/* Classify a plain scalar: 1 = integer, 2 = float, 0 = neither (string). */
static int yaml_num_kind(const char *p, int64_t n) {
    if (n == 0) {
        return 0;
    }
    int64_t i = 0;
    if (p[i] == '-' || p[i] == '+') {
        i++;
    }
    int64_t digits = 0;
    while (i < n && p[i] >= '0' && p[i] <= '9') {
        i++;
        digits++;
    }
    int is_float = 0;
    if (i < n && p[i] == '.') {
        is_float = 1;
        i++;
        while (i < n && p[i] >= '0' && p[i] <= '9') {
            i++;
            digits++;
        }
    }
    if (i < n && (p[i] == 'e' || p[i] == 'E')) {
        is_float = 1;
        i++;
        if (i < n && (p[i] == '+' || p[i] == '-')) {
            i++;
        }
        int64_t edig = 0;
        while (i < n && p[i] >= '0' && p[i] <= '9') {
            i++;
            edig++;
        }
        if (edig == 0) {
            return 0;
        }
    }
    if (i != n || digits == 0) {
        return 0;
    }
    return is_float ? 2 : 1;
}

static int yaml_eq(const char *p, int64_t n, const char *lit) {
    return (int64_t)strlen(lit) == n && memcmp(p, lit, (size_t)n) == 0;
}

/* Emit a scalar value `p[0..n)` (already right-trimmed, non-empty handled by
 * caller) with YAML 1.2 core scalar resolution. */
static void yaml_emit_scalar(ZapYamlOut *o, const char *p, int64_t n) {
    if (n == 0) {
        yo_puts(o, "null");
        return;
    }
    /* Flow collections and JSON-style quoted strings pass through verbatim
     * (the supported flow subset is JSON-compatible). */
    if (p[0] == '{' || p[0] == '[' || p[0] == '"') {
        yo_putn(o, p, n);
        return;
    }
    if (p[0] == '\'') {
        yaml_emit_squoted(o, p, n);
        return;
    }
    if (yaml_eq(p, n, "~") || yaml_eq(p, n, "null") || yaml_eq(p, n, "Null") ||
        yaml_eq(p, n, "NULL")) {
        yo_puts(o, "null");
        return;
    }
    if (yaml_eq(p, n, "true") || yaml_eq(p, n, "True") || yaml_eq(p, n, "TRUE")) {
        yo_puts(o, "true");
        return;
    }
    if (yaml_eq(p, n, "false") || yaml_eq(p, n, "False") || yaml_eq(p, n, "FALSE")) {
        yo_puts(o, "false");
        return;
    }
    int k = yaml_num_kind(p, n);
    if (k != 0) {
        yo_putn(o, p, n); /* numeric literal — valid JSON number */
        return;
    }
    yaml_emit_jstr(o, p, n); /* plain string */
}

/* Find the key/value separator colon in a block-mapping line: a ':' that is at
 * end of content or followed by a space, outside quotes. Returns its index, or
 * -1 if the line is not a mapping entry. */
static int64_t yaml_find_colon(const char *p, int64_t n) {
    int in_s = 0, in_d = 0;
    for (int64_t i = 0; i < n; i++) {
        char c = p[i];
        if (in_s) {
            if (c == '\'') {
                in_s = 0;
            }
            continue;
        }
        if (in_d) {
            if (c == '\\') {
                i++;
            } else if (c == '"') {
                in_d = 0;
            }
            continue;
        }
        if (c == '\'') {
            in_s = 1;
        } else if (c == '"') {
            in_d = 1;
        } else if (c == ':' && (i + 1 >= n || p[i + 1] == ' ')) {
            return i;
        }
    }
    return -1;
}

static int yaml_is_seq_item(const ZapYamlLine *l) {
    return l->len > 0 && l->p[0] == '-' && (l->len == 1 || l->p[1] == ' ');
}

/* Forward declaration: emit the block whose first line is L[*i] at the given
 * indent (== L[*i].indent). */
static void yaml_emit_block(ZapYamlLine *L, int64_t n, int64_t *i, int64_t base, ZapYamlOut *o);

/* Emit one mapping-entry key, then the ':' separator. `p[0..colon)` is the key
 * (right-trimmed); keys are always emitted as JSON strings. */
static void yaml_emit_key(ZapYamlOut *o, const char *p, int64_t colon) {
    int64_t klen = colon;
    while (klen > 0 && p[klen - 1] == ' ') {
        klen--;
    }
    if (klen > 0 && p[0] == '"') {
        yo_putn(o, p, klen); /* JSON-compatible quoted key, verbatim */
    } else if (klen > 0 && p[0] == '\'') {
        yaml_emit_squoted(o, p, klen);
    } else {
        yaml_emit_jstr(o, p, klen);
    }
    yo_putc(o, ':');
}

/* Emit a block mapping: consecutive lines at indent `base` that are `key:`
 * entries (and their nested blocks). */
static void yaml_emit_map(ZapYamlLine *L, int64_t n, int64_t *i, int64_t base, ZapYamlOut *o) {
    yo_putc(o, '{');
    int first = 1;
    while (*i < n && L[*i].indent == base && !yaml_is_seq_item(&L[*i])) {
        const char *p = L[*i].p;
        int64_t len = L[*i].len;
        int64_t colon = yaml_find_colon(p, len);
        if (colon < 0) {
            break; /* not a mapping entry */
        }
        if (!first) {
            yo_putc(o, ',');
        }
        first = 0;
        yaml_emit_key(o, p, colon);
        int64_t vs = colon + 1;
        while (vs < len && p[vs] == ' ') {
            vs++;
        }
        int64_t vlen = len - vs;
        const char *vp = p + vs;
        if (vlen == 0) {
            (*i)++;
            if (*i < n && L[*i].indent > base) {
                yaml_emit_block(L, n, i, L[*i].indent, o);
            } else if (*i < n && L[*i].indent == base && yaml_is_seq_item(&L[*i])) {
                /* a block sequence nested under the key at the same indent */
                yaml_emit_block(L, n, i, base, o);
            } else {
                yo_puts(o, "null");
            }
        } else {
            yaml_emit_scalar(o, vp, vlen);
            (*i)++;
        }
    }
    yo_putc(o, '}');
}

/* Emit a block sequence: consecutive `-` items at indent `base`. */
static void yaml_emit_seq(ZapYamlLine *L, int64_t n, int64_t *i, int64_t base, ZapYamlOut *o) {
    yo_putc(o, '[');
    int first = 1;
    while (*i < n && L[*i].indent == base && yaml_is_seq_item(&L[*i])) {
        if (!first) {
            yo_putc(o, ',');
        }
        first = 0;
        const char *p = L[*i].p;
        int64_t len = L[*i].len;
        /* content after the dash, with one optional separating space */
        int64_t rs = 1;
        while (rs < len && p[rs] == ' ') {
            rs++;
        }
        int64_t rlen = len - rs;
        const char *rp = p + rs;
        if (rlen == 0) {
            /* bare dash: a nested block on the following deeper-indented lines */
            (*i)++;
            if (*i < n && L[*i].indent > base) {
                yaml_emit_block(L, n, i, L[*i].indent, o);
            } else {
                yo_puts(o, "null");
            }
        } else if (yaml_find_colon(rp, rlen) >= 0) {
            /* sequence of mappings: rewrite this line as a map entry at column
             * base+2 and re-dispatch, so the entry plus its aligned
             * continuation lines are consumed as one mapping. */
            L[*i].p = rp;
            L[*i].len = rlen;
            L[*i].indent = base + 2;
            yaml_emit_block(L, n, i, base + 2, o);
        } else {
            yaml_emit_scalar(o, rp, rlen);
            (*i)++;
        }
    }
    yo_putc(o, ']');
}

static void yaml_emit_block(ZapYamlLine *L, int64_t n, int64_t *i, int64_t base, ZapYamlOut *o) {
    if (*i >= n) {
        yo_puts(o, "null");
        return;
    }
    const char *p = L[*i].p;
    int64_t len = L[*i].len;
    /* A line that opens with a flow collection or a quote is a single scalar
     * value (flow `{...}`/`[...]`, or a quoted string) — not a block mapping,
     * even though it may contain a `:` inside the flow/quotes. */
    int leading_flow = len > 0 && (p[0] == '{' || p[0] == '[' || p[0] == '"' || p[0] == '\'');
    if (yaml_is_seq_item(&L[*i])) {
        yaml_emit_seq(L, n, i, base, o);
    } else if (!leading_flow && yaml_find_colon(p, len) >= 0) {
        yaml_emit_map(L, n, i, base, o);
    } else {
        /* a bare scalar / flow / quoted document */
        yaml_emit_scalar(o, p, len);
        (*i)++;
    }
}

/* Strip a trailing ` #...` comment (outside quotes) and right-trim spaces from a
 * line's content `[*start, *end)`. A line whose first non-space char is `#` is a
 * whole-line comment (becomes empty). */
static void yaml_strip_comment(const char *d, int64_t *start, int64_t *end) {
    int in_s = 0, in_d = 0;
    int64_t s = *start, e = *end;
    for (int64_t i = s; i < e; i++) {
        char c = d[i];
        if (in_s) {
            if (c == '\'') {
                in_s = 0;
            }
            continue;
        }
        if (in_d) {
            if (c == '\\') {
                i++;
            } else if (c == '"') {
                in_d = 0;
            }
            continue;
        }
        if (c == '\'') {
            in_s = 1;
        } else if (c == '"') {
            in_d = 1;
        } else if (c == '#' && (i == s || d[i - 1] == ' ' || d[i - 1] == '\t')) {
            e = i;
            break;
        }
    }
    while (e > s && (d[e - 1] == ' ' || d[e - 1] == '\t')) {
        e--;
    }
    *end = e;
}

/* yaml._to_json(s): transpile the supported YAML subset to a JSON string. */
void *zap_yaml_to_json(void *sp) {
    ZapStr *src = (ZapStr *)sp;
    const char *d = src->data;
    int64_t slen = src->len;

    /* Preprocess into logical lines (skip blanks, comments, document markers). */
    int64_t cap = 16;
    ZapYamlLine *L = (ZapYamlLine *)malloc((size_t)cap * sizeof(ZapYamlLine));
    if (!L) {
        zap_yaml_fault("yaml: out of memory");
    }
    int64_t nlines = 0;
    int64_t pos = 0;
    while (pos < slen) {
        int64_t ls = pos;
        while (pos < slen && d[pos] != '\n') {
            pos++;
        }
        int64_t le = pos; /* [ls, le) is the physical line (no newline) */
        if (pos < slen) {
            pos++; /* consume '\n' */
        }
        /* indentation: count spaces; reject a tab in the indentation. */
        int64_t j = ls;
        int64_t indent = 0;
        while (j < le && (d[j] == ' ' || d[j] == '\t')) {
            if (d[j] == '\t') {
                free(L);
                zap_yaml_fault("yaml: tab character in indentation (use spaces)");
            }
            indent++;
            j++;
        }
        int64_t cstart = j;
        int64_t cend = le;
        yaml_strip_comment(d, &cstart, &cend);
        int64_t clen = cend - cstart;
        if (clen == 0) {
            continue; /* blank or comment-only */
        }
        if ((clen == 3 && memcmp(d + cstart, "---", 3) == 0) ||
            (clen == 3 && memcmp(d + cstart, "...", 3) == 0)) {
            continue; /* single-document marker: ignored */
        }
        if (nlines == cap) {
            cap *= 2;
            ZapYamlLine *nl = (ZapYamlLine *)realloc(L, (size_t)cap * sizeof(ZapYamlLine));
            if (!nl) {
                free(L);
                zap_yaml_fault("yaml: out of memory");
            }
            L = nl;
        }
        L[nlines].p = d + cstart;
        L[nlines].len = clen;
        L[nlines].indent = indent;
        nlines++;
    }

    ZapYamlOut o = {0};
    if (nlines == 0) {
        yo_puts(&o, "null");
    } else {
        int64_t i = 0;
        yaml_emit_block(L, nlines, &i, L[0].indent, &o);
    }
    free(L);
    void *r = zap_str_new(o.buf ? o.buf : "null", o.buf ? o.len : 4);
    free(o.buf);
    return r;
}

/* Prefix every non-empty line of `s` with `pad` — the indentation step for the
 * `@derive(Yaml)` block encoder when it nests a child struct's `to_yaml`. */
void *zap_yaml_indent(void *sp, void *padp) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *pad = (const ZapStr *)padp;
    int64_t n = s->len;
    const char *d = s->data;
    /* worst case: a `pad` before every byte, plus a leading pad. */
    int64_t cap = n + (n + 1) * pad->len + 1;
    char *out = (char *)zap_buf(cap > 0 ? cap : 1);
    int64_t o = 0;
    int at_line_start = 1;
    for (int64_t i = 0; i < n; i++) {
        if (at_line_start && d[i] != '\n' && pad->len > 0) {
            memcpy(out + o, pad->data, (size_t)pad->len);
            o += pad->len;
        }
        out[o++] = d[i];
        at_line_start = (d[i] == '\n');
    }
    return zap_str_new(out, o);
}

