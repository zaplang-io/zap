/* ===========================================================================
 * csv — RFC 4180-ish field helpers for the @derive(Csv) record codec
 * (ticket #132, D-0059 §4b). A @derive(Csv) struct maps to one CSV record
 * (fields = columns, declaration order). Encode quotes a field only when it
 * contains a delimiter/quote/newline (doubling embedded quotes); decode splits
 * one record line into fields and parses each column to the field's scalar
 * type. These are the pieces Zap lacks a direct surface for; the per-field walk
 * and codegen reuse the shared DeriveFieldKind core (same as @derive(Json)).
 * ======================================================================== */

/* Quote a CSV field iff it contains a comma, double-quote, CR or LF; embedded
 * quotes are doubled (RFC 4180). Otherwise the string passes through. */
void *zap_csv_quote(void *sp) {
    ZapStr *s = (ZapStr *)sp;
    int64_t n = s->len;
    const char *in = s->data;
    int needs = 0;
    for (int64_t i = 0; i < n; i++) {
        char c = in[i];
        if (c == ',' || c == '"' || c == '\n' || c == '\r') {
            needs = 1;
            break;
        }
    }
    if (!needs) {
        return s;
    }
    /* worst case: every char is a quote (doubled) + 2 surrounding quotes. */
    char *out = (char *)zap_buf(n * 2 + 2);
    int64_t o = 0;
    out[o++] = '"';
    for (int64_t i = 0; i < n; i++) {
        if (in[i] == '"') {
            out[o++] = '"';
        }
        out[o++] = in[i];
    }
    out[o++] = '"';
    return zap_str_new(out, o);
}

/* Join a list of ZapStr* fields into one CSV record line, comma-separated. */
void *zap_csv_join(void *lp) {
    ZapList *l = (ZapList *)lp;
    int64_t total = 0;
    for (int64_t i = 0; i < l->len; i++) {
        ZapStr *p = (ZapStr *)zap_list_get_ptr(lp, i);
        total += p->len + 1; /* + separator */
    }
    char *out = (char *)zap_buf(total > 0 ? total : 1);
    int64_t o = 0;
    for (int64_t i = 0; i < l->len; i++) {
        if (i > 0) {
            out[o++] = ',';
        }
        ZapStr *p = (ZapStr *)zap_list_get_ptr(lp, i);
        if (p->len > 0) {
            memcpy(out + o, p->data, (size_t)p->len);
            o += p->len;
        }
    }
    return zap_str_new(out, o);
}

/* Split one CSV record line into a list of ZapStr* fields (RFC 4180 quoting:
 * quoted fields may contain commas/newlines; "" is an embedded quote). */
void *zap_csv_split(void *lp) {
    ZapStr *line = (ZapStr *)lp;
    const char *d = line->data;
    int64_t n = line->len;
    void *out = zap_list_new();
    int64_t i = 0;
    /* A buffer reused per field; sized to the whole line (fields never exceed
     * it after unquoting). */
    char *buf = (char *)zap_buf(n > 0 ? n : 1);
    while (i <= n) {
        int64_t o = 0;
        if (i < n && d[i] == '"') {
            /* quoted field */
            i++;
            while (i < n) {
                if (d[i] == '"') {
                    if (i + 1 < n && d[i + 1] == '"') {
                        buf[o++] = '"';
                        i += 2;
                    } else {
                        i++;
                        break;
                    }
                } else {
                    buf[o++] = d[i++];
                }
            }
        } else {
            while (i < n && d[i] != ',') {
                buf[o++] = d[i++];
            }
        }
        zap_list_push_ptr(out, zap_str_new(buf, o));
        if (i < n && d[i] == ',') {
            i++;
            if (i == n) {
                /* trailing comma -> a final empty field */
                zap_list_push_ptr(out, zap_str_new("", 0));
                break;
            }
            continue;
        }
        break;
    }
    return out;
}

/* Parse a string to i64 / f64 / bool for the @derive(Csv) decode direction.
 * Lenient (v1): unparseable input yields 0 / 0.0 / false. */
int64_t zap_str_to_i64(void *sp) {
    ZapStr *s = (ZapStr *)sp;
    char tmp[32];
    int64_t cl = s->len < 31 ? s->len : 31;
    memcpy(tmp, s->data, (size_t)cl);
    tmp[cl] = 0;
    return (int64_t)strtoll(tmp, NULL, 10);
}
double zap_str_to_f64(void *sp) {
    ZapStr *s = (ZapStr *)sp;
    char tmp[64];
    int64_t cl = s->len < 63 ? s->len : 63;
    memcpy(tmp, s->data, (size_t)cl);
    tmp[cl] = 0;
    return strtod(tmp, NULL);
}
int64_t zap_str_to_bool(void *sp) {
    ZapStr *s = (ZapStr *)sp;
    int64_t n = s->len;
    const char *d = s->data;
    if (n == 4 && (memcmp(d, "true", 4) == 0 || memcmp(d, "True", 4) == 0)) {
        return 1;
    }
    if (n == 1 && d[0] == '1') {
        return 1;
    }
    return 0;
}

