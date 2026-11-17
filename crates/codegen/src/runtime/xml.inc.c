/* zap runtime fragment: xml.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * stdlib: xml — core XML DOM tokenizer + escaping (ticket #351, D-0085)
 * ---------------------------------------------------------------------------
 * A flat, stateful pull-tokenizer over the UTF-8 source, mirroring the json
 * tokenizer (D-0059). The high-level tree build (`xml.parse`) and serialize walk
 * (`xml.dumps`) are Zap prelude functions that drive this cursor, so the runtime
 * never depends on the XmlElement layout. The tokenizer silently skips the XML
 * declaration / processing instructions / comments / DOCTYPE, emits CDATA as a
 * literal TEXT token, and decodes entities for TEXT and attribute values on
 * demand. The fragment-join step reuses `zap_json_join`. See design/0033-xml.md.
 * ======================================================================== */

enum {
    ZAP_XTOK_END = 0,    /* end of input                                      */
    ZAP_XTOK_OPEN = 1,   /* `<name`  — start-tag opened; name available        */
    ZAP_XTOK_ATTR = 2,   /* an attribute within a start tag (name + value)     */
    ZAP_XTOK_SCLOSE = 3, /* `>`      — start tag closed; element has content    */
    ZAP_XTOK_SELF = 4,   /* `/>`     — self-closing (empty) element            */
    ZAP_XTOK_TEXT = 5,   /* character data (value)                             */
    ZAP_XTOK_CLOSE = 6,  /* `</name>`                                          */
    ZAP_XTOK_ERR = -1
};

typedef struct {
    const char *data;
    int64_t len;
    int64_t pos;
    int64_t kind;    /* current token kind (ZAP_XTOK_*)                        */
    int in_tag;      /* inside a start tag (expecting attrs / `>` / `/>`)       */
    const char *nm;  /* current token name slice (OPEN / ATTR / CLOSE)         */
    int64_t nmlen;
    const char *vl;  /* current token value slice (ATTR value / TEXT), raw     */
    int64_t vllen;
    int has_ent;     /* value slice contains `&` entities to decode            */
} ZapXmlLex;

static int zap_xml_is_ws(char c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r';
}

/* A name character: anything that is not whitespace, `>`, `/` or `=`. Covers
 * prefixed names (`ns:Foo`), whose colon is kept. */
static int zap_xml_is_name(char c) {
    return !zap_xml_is_ws(c) && c != '>' && c != '/' && c != '=';
}

/* Open a cursor over a str/bytes source (a ZapStr*). */
void *zap_xml_lex_new(void *s) {
    ZapStr *z = (ZapStr *)s;
    ZapXmlLex *lx = (ZapXmlLex *)zap_buf((int64_t)sizeof(ZapXmlLex));
    lx->data = z->data;
    lx->len = z->len;
    lx->pos = 0;
    lx->kind = ZAP_XTOK_END;
    lx->in_tag = 0;
    lx->nm = NULL;
    lx->nmlen = 0;
    lx->vl = NULL;
    lx->vllen = 0;
    lx->has_ent = 0;
    return lx;
}

/* Advance to and classify the next token; returns (and stores) its kind. */
int64_t zap_xml_lex_next(void *h) {
    ZapXmlLex *lx = (ZapXmlLex *)h;
    const char *d = lx->data;
    int64_t n = lx->len;

    if (lx->in_tag) {
        int64_t i = lx->pos;
        while (i < n && zap_xml_is_ws(d[i])) {
            i++;
        }
        if (i >= n) {
            lx->pos = i;
            lx->in_tag = 0;
            lx->kind = ZAP_XTOK_END;
            return ZAP_XTOK_END;
        }
        char c = d[i];
        if (c == '/' && i + 1 < n && d[i + 1] == '>') {
            lx->pos = i + 2;
            lx->in_tag = 0;
            lx->kind = ZAP_XTOK_SELF;
            return ZAP_XTOK_SELF;
        }
        if (c == '>') {
            lx->pos = i + 1;
            lx->in_tag = 0;
            lx->kind = ZAP_XTOK_SCLOSE;
            return ZAP_XTOK_SCLOSE;
        }
        /* attribute name: up to `=`, whitespace, `>` or `/`. */
        int64_t ns = i;
        int64_t ne = i;
        while (ne < n && zap_xml_is_name(d[ne])) {
            ne++;
        }
        lx->nm = d + ns;
        lx->nmlen = ne - ns;
        int64_t j = ne;
        while (j < n && zap_xml_is_ws(d[j])) {
            j++;
        }
        if (j < n && d[j] == '=') {
            j++;
            while (j < n && zap_xml_is_ws(d[j])) {
                j++;
            }
            if (j < n && (d[j] == '"' || d[j] == '\'')) {
                char q = d[j];
                j++;
                int64_t vs = j;
                int ent = 0;
                while (j < n && d[j] != q) {
                    if (d[j] == '&') {
                        ent = 1;
                    }
                    j++;
                }
                lx->vl = d + vs;
                lx->vllen = j - vs;
                lx->has_ent = ent;
                lx->pos = (j < n) ? j + 1 : n; /* consume the closing quote */
            } else {
                /* unquoted value (lenient): up to whitespace, `>` or `/`. */
                int64_t vs = j;
                int ent = 0;
                while (j < n && !zap_xml_is_ws(d[j]) && d[j] != '>' && d[j] != '/') {
                    if (d[j] == '&') {
                        ent = 1;
                    }
                    j++;
                }
                lx->vl = d + vs;
                lx->vllen = j - vs;
                lx->has_ent = ent;
                lx->pos = j;
            }
        } else {
            /* value-less attribute (lenient). */
            lx->vl = d + ne;
            lx->vllen = 0;
            lx->has_ent = 0;
            lx->pos = j;
        }
        lx->kind = ZAP_XTOK_ATTR;
        return ZAP_XTOK_ATTR;
    }

    /* content mode: loop so skipped markup (PI / comment / DOCTYPE) re-scans. */
    for (;;) {
        int64_t i = lx->pos;
        if (i >= n) {
            lx->pos = i;
            lx->kind = ZAP_XTOK_END;
            return ZAP_XTOK_END;
        }
        if (d[i] != '<') {
            /* text run up to the next `<`. */
            int64_t ts = i;
            int ent = 0;
            while (i < n && d[i] != '<') {
                if (d[i] == '&') {
                    ent = 1;
                }
                i++;
            }
            lx->vl = d + ts;
            lx->vllen = i - ts;
            lx->has_ent = ent;
            lx->pos = i;
            lx->kind = ZAP_XTOK_TEXT;
            return ZAP_XTOK_TEXT;
        }
        /* `<...` */
        if (i + 1 < n && d[i + 1] == '?') {
            /* XML declaration / processing instruction: skip to `?>`. */
            int64_t j = i + 2;
            while (j + 2 <= n && !(d[j] == '?' && d[j + 1] == '>')) {
                j++;
            }
            lx->pos = (j + 2 <= n) ? j + 2 : n;
            continue;
        }
        if (i + 4 <= n && memcmp(d + i, "<!--", 4) == 0) {
            /* comment: skip to `-->`. */
            int64_t j = i + 4;
            while (j + 3 <= n && !(d[j] == '-' && d[j + 1] == '-' && d[j + 2] == '>')) {
                j++;
            }
            lx->pos = (j + 3 <= n) ? j + 3 : n;
            continue;
        }
        if (i + 9 <= n && memcmp(d + i, "<![CDATA[", 9) == 0) {
            /* CDATA: literal text (no entity decoding), up to `]]>`. */
            int64_t cs = i + 9;
            int64_t ce = cs;
            while (ce < n && !(ce + 2 < n && d[ce] == ']' && d[ce + 1] == ']' && d[ce + 2] == '>')) {
                ce++;
            }
            lx->vl = d + cs;
            lx->vllen = ce - cs;
            lx->has_ent = 0;
            lx->pos = (ce + 2 < n) ? ce + 3 : n;
            lx->kind = ZAP_XTOK_TEXT;
            return ZAP_XTOK_TEXT;
        }
        if (i + 1 < n && d[i + 1] == '!') {
            /* DOCTYPE or other declaration: skip to the next `>` (naive — a
             * DOCTYPE internal subset `[...]` is not handled; AWS never uses one). */
            int64_t j = i + 2;
            while (j < n && d[j] != '>') {
                j++;
            }
            lx->pos = (j < n) ? j + 1 : n;
            continue;
        }
        if (i + 1 < n && d[i + 1] == '/') {
            /* end tag `</name>`. */
            int64_t ns = i + 2;
            int64_t ne = ns;
            while (ne < n && zap_xml_is_name(d[ne])) {
                ne++;
            }
            lx->nm = d + ns;
            lx->nmlen = ne - ns;
            int64_t j = ne;
            while (j < n && d[j] != '>') {
                j++;
            }
            lx->pos = (j < n) ? j + 1 : n;
            lx->kind = ZAP_XTOK_CLOSE;
            return ZAP_XTOK_CLOSE;
        }
        /* start tag `<name`. */
        int64_t ns = i + 1;
        int64_t ne = ns;
        while (ne < n && zap_xml_is_name(d[ne])) {
            ne++;
        }
        lx->nm = d + ns;
        lx->nmlen = ne - ns;
        lx->pos = ne;
        lx->in_tag = 1;
        lx->kind = ZAP_XTOK_OPEN;
        return ZAP_XTOK_OPEN;
    }
}

/* The current token's name (OPEN / ATTR / CLOSE). */
void *zap_xml_lex_name(void *h) {
    ZapXmlLex *lx = (ZapXmlLex *)h;
    return zap_str_new(lx->nm, lx->nmlen);
}

/* Encode a Unicode code point as UTF-8 into `out`; returns the byte count. */
static int64_t zap_xml_utf8(unsigned long cp, char *out) {
    if (cp < 0x80) {
        out[0] = (char)cp;
        return 1;
    }
    if (cp < 0x800) {
        out[0] = (char)(0xC0 | (cp >> 6));
        out[1] = (char)(0x80 | (cp & 0x3F));
        return 2;
    }
    if (cp < 0x10000) {
        out[0] = (char)(0xE0 | (cp >> 12));
        out[1] = (char)(0x80 | ((cp >> 6) & 0x3F));
        out[2] = (char)(0x80 | (cp & 0x3F));
        return 3;
    }
    out[0] = (char)(0xF0 | (cp >> 18));
    out[1] = (char)(0x80 | ((cp >> 12) & 0x3F));
    out[2] = (char)(0x80 | ((cp >> 6) & 0x3F));
    out[3] = (char)(0x80 | (cp & 0x3F));
    return 4;
}

/* Decode XML entities from `s[0..n)` into `out`; returns the decoded length.
 * Handles the five predefined entities plus numeric `&#DD;` / `&#xHH;`. An
 * unrecognized or unterminated `&...` is passed through verbatim (lenient). The
 * decoded form is never longer than the source, so `n` bytes of `out` suffice. */
static int64_t zap_xml_decode(const char *s, int64_t n, char *out) {
    int64_t o = 0;
    for (int64_t i = 0; i < n; i++) {
        char c = s[i];
        if (c != '&') {
            out[o++] = c;
            continue;
        }
        /* locate the terminating `;` within a small window. */
        int64_t semi = -1;
        for (int64_t k = i + 1; k < n && k < i + 12; k++) {
            if (s[k] == ';') {
                semi = k;
                break;
            }
        }
        if (semi < 0) {
            out[o++] = '&';
            continue;
        }
        const char *e = s + i + 1;
        int64_t elen = semi - (i + 1);
        if (elen == 2 && memcmp(e, "lt", 2) == 0) {
            out[o++] = '<';
        } else if (elen == 2 && memcmp(e, "gt", 2) == 0) {
            out[o++] = '>';
        } else if (elen == 3 && memcmp(e, "amp", 3) == 0) {
            out[o++] = '&';
        } else if (elen == 4 && memcmp(e, "quot", 4) == 0) {
            out[o++] = '"';
        } else if (elen == 4 && memcmp(e, "apos", 4) == 0) {
            out[o++] = '\'';
        } else if (elen >= 2 && e[0] == '#') {
            unsigned long cp = 0;
            int ok = 1;
            if (e[1] == 'x' || e[1] == 'X') {
                if (elen < 3) {
                    ok = 0;
                }
                for (int64_t k = 2; ok && k < elen; k++) {
                    char hx = e[k];
                    cp <<= 4;
                    if (hx >= '0' && hx <= '9') {
                        cp |= (unsigned long)(hx - '0');
                    } else if (hx >= 'a' && hx <= 'f') {
                        cp |= (unsigned long)(hx - 'a' + 10);
                    } else if (hx >= 'A' && hx <= 'F') {
                        cp |= (unsigned long)(hx - 'A' + 10);
                    } else {
                        ok = 0;
                    }
                }
            } else {
                for (int64_t k = 1; k < elen; k++) {
                    char dc = e[k];
                    if (dc < '0' || dc > '9') {
                        ok = 0;
                        break;
                    }
                    cp = cp * 10 + (unsigned long)(dc - '0');
                }
            }
            if (ok && cp <= 0x10FFFF) {
                o += zap_xml_utf8(cp, out + o);
            } else {
                out[o++] = '&'; /* leave the rest to copy literally */
                continue;
            }
        } else {
            out[o++] = '&'; /* unknown entity: copy the rest literally */
            continue;
        }
        i = semi; /* advance past the `;` */
    }
    return o;
}

/* The current token's value (ATTR value / TEXT content), entity-decoded. */
void *zap_xml_lex_value(void *h) {
    ZapXmlLex *lx = (ZapXmlLex *)h;
    if (!lx->has_ent) {
        return zap_str_new(lx->vl, lx->vllen);
    }
    char *out = (char *)zap_buf(lx->vllen > 0 ? lx->vllen : 1);
    int64_t o = zap_xml_decode(lx->vl, lx->vllen, out);
    return zap_str_new(out, o);
}

/* Escape text content: `&` `<` `>`. */
void *zap_xml_esc_text(void *sp) {
    ZapStr *s = (ZapStr *)sp;
    int64_t n = s->len;
    const char *in = s->data;
    char *out = (char *)zap_buf(n * 5 > 0 ? n * 5 : 1); /* worst case `&amp;` */
    int64_t o = 0;
    for (int64_t i = 0; i < n; i++) {
        char c = in[i];
        if (c == '&') {
            memcpy(out + o, "&amp;", 5);
            o += 5;
        } else if (c == '<') {
            memcpy(out + o, "&lt;", 4);
            o += 4;
        } else if (c == '>') {
            memcpy(out + o, "&gt;", 4);
            o += 4;
        } else {
            out[o++] = c;
        }
    }
    return zap_str_new(out, o);
}

/* Escape an attribute value and wrap it in double quotes: `&` `<` `"`. */
void *zap_xml_esc_attr(void *sp) {
    ZapStr *s = (ZapStr *)sp;
    int64_t n = s->len;
    const char *in = s->data;
    char *out = (char *)zap_buf(n * 6 + 2); /* worst case `&quot;` + two quotes */
    int64_t o = 0;
    out[o++] = '"';
    for (int64_t i = 0; i < n; i++) {
        char c = in[i];
        if (c == '&') {
            memcpy(out + o, "&amp;", 5);
            o += 5;
        } else if (c == '<') {
            memcpy(out + o, "&lt;", 4);
            o += 4;
        } else if (c == '"') {
            memcpy(out + o, "&quot;", 6);
            o += 6;
        } else {
            out[o++] = c;
        }
    }
    out[o++] = '"';
    return zap_str_new(out, o);
}

