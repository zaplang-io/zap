/* ===========================================================================
 * stdlib: encoding/url — RFC-3986 percent-encoding + decoding (ticket #335)
 * ---------------------------------------------------------------------------
 * Pure, dependency-free percent-encoding over raw bytes. A `ZapStr` is a byte
 * buffer and a `str`'s bytes ARE its UTF-8 encoding, so encoding proceeds
 * byte-by-byte and is automatically correct for multibyte codepoints. The
 * "unreserved" set is RFC-3986 §2.3 (`A-Z a-z 0-9 - . _ ~`); every other byte
 * encodes as `%XX` with UPPERCASE hex. This is exactly AWS SigV4's URI-encode
 * (space -> `%20`, never `+`), so `zap_url_quote` is the primitive the signer
 * calls for query keys/values and single path segments; `zap_url_quote_path`
 * leaves `/` unescaped for a whole path (encodeSlash=0). SigV4's non-S3
 * CanonicalURI double-encodes the path, i.e. quote_path(quote_path(path)); S3
 * encodes once. Decoding is the shared inverse (`%XX` -> byte), with a `_plus`
 * variant that also maps `+` -> space for form-encoded query values.
 * =========================================================================== */

/* RFC-3986 §2.3 unreserved set: never percent-encoded. */
static int zap_url_is_unreserved(unsigned char c) {
    return (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') ||
           (c >= '0' && c <= '9') || c == '-' || c == '.' || c == '_' || c == '~';
}

/* Percent-encode `sp` byte-by-byte. `keep_slash` passes `/` through unescaped
 * (path form); otherwise `/` is encoded too (strict / query / segment form). */
static void *zap_url_encode(void *sp, int keep_slash) {
    ZapStr *s = (ZapStr *)sp;
    int64_t n = s->len;
    const unsigned char *in = (const unsigned char *)s->data;
    static const char hex[] = "0123456789ABCDEF";
    /* worst case: every byte becomes "%XX" (3 bytes). */
    char *out = (char *)zap_buf(n > 0 ? n * 3 : 1);
    int64_t o = 0;
    for (int64_t i = 0; i < n; i++) {
        unsigned char c = in[i];
        if (zap_url_is_unreserved(c) || (keep_slash && c == '/')) {
            out[o++] = (char)c;
        } else {
            out[o++] = '%';
            out[o++] = hex[(c >> 4) & 0xF];
            out[o++] = hex[c & 0xF];
        }
    }
    return zap_str_new(out, o);
}

void *zap_url_quote(void *sp) { return zap_url_encode(sp, 0); }
void *zap_url_quote_path(void *sp) { return zap_url_encode(sp, 1); }

/* Hex digit value, or -1 if `c` is not a hex digit. */
static int zap_url_hexval(unsigned char c) {
    if (c >= '0' && c <= '9') {
        return c - '0';
    }
    if (c >= 'a' && c <= 'f') {
        return c - 'a' + 10;
    }
    if (c >= 'A' && c <= 'F') {
        return c - 'A' + 10;
    }
    return -1;
}

/* Decode `%XX` escapes back to bytes. A `%` not followed by two hex digits is
 * left literal (lenient, like Python's urllib). `plus` maps `+` -> space. */
static void *zap_url_decode(void *sp, int plus) {
    ZapStr *s = (ZapStr *)sp;
    int64_t n = s->len;
    const unsigned char *in = (const unsigned char *)s->data;
    /* output is never longer than the input. */
    char *out = (char *)zap_buf(n > 0 ? n : 1);
    int64_t o = 0;
    for (int64_t i = 0; i < n; i++) {
        unsigned char c = in[i];
        if (c == '%' && i + 2 < n) {
            int hi = zap_url_hexval(in[i + 1]);
            int lo = zap_url_hexval(in[i + 2]);
            if (hi >= 0 && lo >= 0) {
                out[o++] = (char)((hi << 4) | lo);
                i += 2;
                continue;
            }
            out[o++] = (char)c; /* malformed escape: keep the literal '%'. */
        } else if (plus && c == '+') {
            out[o++] = ' ';
        } else {
            out[o++] = (char)c;
        }
    }
    return zap_str_new(out, o);
}

void *zap_url_unquote(void *sp) { return zap_url_decode(sp, 0); }
void *zap_url_unquote_plus(void *sp) { return zap_url_decode(sp, 1); }

/* Byte-lexicographic compare of two ZapStr* (shorter sorts first on a prefix
 * tie) — the ordering AWS SigV4 applies to encoded query components. */
static int zap_url_strcmp(void *ap, void *bp) {
    ZapStr *a = (ZapStr *)ap;
    ZapStr *b = (ZapStr *)bp;
    int64_t m = a->len < b->len ? a->len : b->len;
    int c = memcmp(a->data, b->data, (size_t)m);
    if (c != 0) {
        return c < 0 ? -1 : 1;
    }
    if (a->len == b->len) {
        return 0;
    }
    return a->len < b->len ? -1 : 1;
}

/* Build a query string from a dict[str,str]. Every key and value is percent-
 * encoded with `zap_url_quote`. When `do_sort`, the `k=v` pairs are ordered by
 * encoded key then encoded value (SigV4 CanonicalQueryString); otherwise the
 * dict's iteration order is kept (unspecified — open-addressing hash order).
 * The encoded keys/values are held in traced
 * ZapLists (so a GC during the loop keeps them live) and only an int64 index
 * array — pointer-free, safe in a `zap_buf` leaf — is sorted. */
static void *zap_url_build_query(void *dp, int do_sort) {
    void *keys = zap_dict_keys_ptr(dp);
    int64_t m = zap_list_len(keys);
    void *ek = zap_list_new(); /* encoded keys   (traced) */
    void *ev = zap_list_new(); /* encoded values (traced) */
    for (int64_t i = 0; i < m; i++) {
        void *k = zap_list_get_ptr(keys, i);
        void *v = zap_dict_get_str_ptr(dp, k);
        if (v == NULL) {
            v = zap_str_new("", 0);
        }
        zap_list_push_ptr(ek, zap_url_quote(k));
        zap_list_push_ptr(ev, zap_url_quote(v));
    }
    int64_t *idx = (int64_t *)zap_buf((int64_t)sizeof(int64_t) * (m > 0 ? m : 1));
    for (int64_t i = 0; i < m; i++) {
        idx[i] = i;
    }
    if (do_sort) {
        /* insertion sort (query param counts are small); stable. */
        for (int64_t i = 1; i < m; i++) {
            int64_t cur = idx[i];
            void *ck = zap_list_get_ptr(ek, cur);
            void *cv = zap_list_get_ptr(ev, cur);
            int64_t j = i - 1;
            while (j >= 0) {
                void *jk = zap_list_get_ptr(ek, idx[j]);
                int c = zap_url_strcmp(jk, ck);
                if (c == 0) {
                    c = zap_url_strcmp(zap_list_get_ptr(ev, idx[j]), cv);
                }
                if (c <= 0) {
                    break;
                }
                idx[j + 1] = idx[j];
                j--;
            }
            idx[j + 1] = cur;
        }
    }
    void *parts = zap_list_new();
    for (int64_t t = 0; t < m; t++) {
        int64_t i = idx[t];
        if (t > 0) {
            zap_list_push_ptr(parts, zap_str_new("&", 1));
        }
        zap_list_push_ptr(parts, zap_list_get_ptr(ek, i));
        zap_list_push_ptr(parts, zap_str_new("=", 1));
        zap_list_push_ptr(parts, zap_list_get_ptr(ev, i));
    }
    return zap_json_join(parts); /* generic ZapStr*-list concatenator */
}

void *zap_url_encode_query(void *dp) { return zap_url_build_query(dp, 0); }
void *zap_url_canonical_query(void *dp) { return zap_url_build_query(dp, 1); }

/* ===========================================================================
 * stdlib: url parse / build — urlsplit / parse_qs / urljoin (ticket #970,
 *   D-0209)
 * ---------------------------------------------------------------------------
 * The decomposition half of `encoding/url`: take a URL apart into its RFC-3986
 * components, read a `k=v&...` query string into a map, resolve a relative
 * reference against a base, and rebuild a URL from parts. Companion to the
 * percent codec above; shares `zap_url_decode` (`%XX` / `+`) and `zap_url_quote`.
 *
 * A URL is split lazily per field: `zap_url_split_fields` runs the single RFC
 * parser into slices of the original (immutable) `ZapStr`, and each of the
 * eight prelude-`UrlParts` getters (`zap_url_scheme`/`has_authority`/`userinfo`/
 * `host`/`port`/`path`/`query`/`fragment`) copies out its slice. This mirrors
 * the `KeyInfo` getters (crypto #643): re-parsing a short URL per field is
 * trivial and keeps the parser single-sourced. Only `zap_url_port` validates
 * (a non-numeric / out-of-range port raises a catchable `ValueError`, like
 * Python's `SplitResult.port`); every other getter is lenient like `urlsplit`.
 * =========================================================================== */

/* Parsed slices into a URL's immutable bytes. `port` is -1 when absent; a
 * present-but-malformed port sets `port_valid = 0` so the port getter can raise.
 * `host` is an IPv6 literal WITHOUT its surrounding brackets (Python `.hostname`
 * form); `zap_url_build` re-wraps a host containing ':'. */
typedef struct {
    const char *scheme;
    int64_t scheme_len;
    int has_authority;
    const char *userinfo;
    int64_t userinfo_len;
    const char *host;
    int64_t host_len;
    int64_t port; /* -1 when no `:port` is present */
    int port_valid;
    const char *path;
    int64_t path_len;
    const char *query;
    int64_t query_len;
    const char *fragment;
    int64_t fragment_len;
} ZapUrlFields;

static int zap_url_is_scheme_char(unsigned char c) {
    return (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') ||
           (c >= '0' && c <= '9') || c == '+' || c == '-' || c == '.';
}

/* RFC-3986 §3 split of `sp` into `f`. Never raises (port validity is reported
 * via `port_valid` for the caller to act on). */
static void zap_url_split_fields(const void *sp, ZapUrlFields *f) {
    const ZapStr *s = (const ZapStr *)sp;
    const char *p = s->data;
    int64_t n = s->len;
    memset(f, 0, sizeof(*f));
    f->port = -1;
    f->port_valid = 1;

    /* 1. fragment: everything after the first '#'. */
    int64_t end = n;
    for (int64_t i = 0; i < end; i++) {
        if (p[i] == '#') {
            f->fragment = p + i + 1;
            f->fragment_len = end - (i + 1);
            end = i;
            break;
        }
    }

    /* 2. scheme: ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":" before any of
     *    '/', '?'. A leading digit disqualifies it (RFC-3986 §3.1). */
    int64_t rest = 0;
    if (end > 0 && ((p[0] >= 'A' && p[0] <= 'Z') || (p[0] >= 'a' && p[0] <= 'z'))) {
        int64_t j = 1;
        while (j < end && zap_url_is_scheme_char((unsigned char)p[j])) {
            j++;
        }
        if (j < end && p[j] == ':') {
            f->scheme = p;
            f->scheme_len = j;
            rest = j + 1;
        }
    }

    /* 3. authority: present iff the remainder begins with "//". It runs to the
     *    next '/', '?' or end. */
    int64_t auth_start = -1, auth_end = -1;
    if (end - rest >= 2 && p[rest] == '/' && p[rest + 1] == '/') {
        f->has_authority = 1;
        auth_start = rest + 2;
        auth_end = auth_start;
        while (auth_end < end && p[auth_end] != '/' && p[auth_end] != '?') {
            auth_end++;
        }
        rest = auth_end;
    }

    /* 4. query: after the first '?' in the remainder; path is before it. */
    int64_t path_start = rest, path_end = end;
    for (int64_t i = rest; i < end; i++) {
        if (p[i] == '?') {
            path_end = i;
            f->query = p + i + 1;
            f->query_len = end - (i + 1);
            break;
        }
    }
    f->path = p + path_start;
    f->path_len = path_end - path_start;

    /* 5. decompose the authority: [ userinfo "@" ] host [ ":" port ]. userinfo
     *    is everything up to the RIGHTMOST '@' (Python `rpartition('@')`). */
    if (f->has_authority) {
        int64_t a = auth_start, b = auth_end;
        int64_t at = -1;
        for (int64_t i = a; i < b; i++) {
            if (p[i] == '@') {
                at = i;
            }
        }
        int64_t host_start = a;
        if (at >= 0) {
            f->userinfo = p + a;
            f->userinfo_len = at - a;
            host_start = at + 1;
        }
        /* host + optional port. */
        if (host_start < b && p[host_start] == '[') {
            /* IPv6 / IPvFuture literal: host is between the brackets. */
            int64_t close = host_start + 1;
            while (close < b && p[close] != ']') {
                close++;
            }
            f->host = p + host_start + 1;
            f->host_len = (close < b ? close : b) - (host_start + 1);
            int64_t after = (close < b) ? close + 1 : b;
            if (after < b && p[after] == ':') {
                /* port text after "]:" */
                int64_t ps = after + 1, pe = b;
                int64_t val = 0, ok = 1;
                for (int64_t i = ps; i < pe; i++) {
                    if (p[i] < '0' || p[i] > '9') {
                        ok = 0;
                        break;
                    }
                    val = val * 10 + (p[i] - '0');
                    if (val > 65535) {
                        ok = 0;
                        break;
                    }
                }
                if (ps == pe) {
                    /* "]:" with no port text — lenient: no port. */
                    f->port = -1;
                    f->port_valid = 1;
                } else {
                    /* a non-empty but non-numeric / out-of-range port is invalid
                     * (raises in the getter). */
                    f->port_valid = ok;
                    f->port = ok ? val : -1;
                }
            }
        } else {
            /* reg-name host: split on the FIRST ':' (Python `partition(':')`). */
            int64_t colon = -1;
            for (int64_t i = host_start; i < b; i++) {
                if (p[i] == ':') {
                    colon = i;
                    break;
                }
            }
            if (colon < 0) {
                f->host = p + host_start;
                f->host_len = b - host_start;
            } else {
                f->host = p + host_start;
                f->host_len = colon - host_start;
                int64_t ps = colon + 1, pe = b;
                int64_t val = 0, ok = 1;
                for (int64_t i = ps; i < pe; i++) {
                    if (p[i] < '0' || p[i] > '9') {
                        ok = 0;
                        break;
                    }
                    val = val * 10 + (p[i] - '0');
                    if (val > 65535) {
                        ok = 0;
                        break;
                    }
                }
                if (ps == pe) {
                    /* "host:" with no port text — lenient: no port. */
                    f->port = -1;
                    f->port_valid = 1;
                } else {
                    /* a non-empty but non-numeric / out-of-range port is invalid
                     * (raises in the getter). */
                    f->port_valid = ok;
                    f->port = ok ? val : -1;
                }
            }
        }
    }
}

/* scheme (lowercased, RFC-3986 §3.1 scheme is case-insensitive). */
void *zap_url_scheme(void *sp) {
    ZapUrlFields f;
    zap_url_split_fields(sp, &f);
    if (f.scheme_len <= 0) {
        return zap_str_new("", 0);
    }
    char *out = (char *)zap_buf(f.scheme_len);
    for (int64_t i = 0; i < f.scheme_len; i++) {
        unsigned char c = (unsigned char)f.scheme[i];
        out[i] = (c >= 'A' && c <= 'Z') ? (char)(c - 'A' + 'a') : (char)c;
    }
    return zap_str_new(out, f.scheme_len);
}

int64_t zap_url_has_authority(void *sp) {
    ZapUrlFields f;
    zap_url_split_fields(sp, &f);
    return f.has_authority ? 1 : 0;
}

void *zap_url_userinfo(void *sp) {
    ZapUrlFields f;
    zap_url_split_fields(sp, &f);
    return zap_str_new(f.userinfo ? f.userinfo : "", f.userinfo_len);
}

void *zap_url_host(void *sp) {
    ZapUrlFields f;
    zap_url_split_fields(sp, &f);
    return zap_str_new(f.host ? f.host : "", f.host_len);
}

/* port — -1 when absent; raises a catchable ValueError for a present but
 * non-numeric / out-of-range port (Python `SplitResult.port` semantics). */
int64_t zap_url_port(void *sp) {
    ZapUrlFields f;
    zap_url_split_fields(sp, &f);
    if (!f.port_valid) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("url.split: invalid port in URL", 30));
    }
    return f.port;
}

void *zap_url_path(void *sp) {
    ZapUrlFields f;
    zap_url_split_fields(sp, &f);
    return zap_str_new(f.path ? f.path : "", f.path_len);
}

void *zap_url_query(void *sp) {
    ZapUrlFields f;
    zap_url_split_fields(sp, &f);
    return zap_str_new(f.query ? f.query : "", f.query_len);
}

void *zap_url_fragment(void *sp) {
    ZapUrlFields f;
    zap_url_split_fields(sp, &f);
    return zap_str_new(f.fragment ? f.fragment : "", f.fragment_len);
}

/* Reassemble a URL from the eight prelude-`UrlParts` fields (`url.build`, the
 * inverse of `url.split` / Python `urlunsplit`). A host containing ':' (an IPv6
 * literal, stored unbracketed) is re-wrapped in `[...]`. Round-trips a
 * normalized URL: `build(split(s)) == s`. */
void *zap_url_build(void *scheme_p, int64_t has_authority, void *userinfo_p,
                    void *host_p, int64_t port, void *path_p, void *query_p,
                    void *fragment_p) {
    const ZapStr *scheme = (const ZapStr *)scheme_p;
    const ZapStr *userinfo = (const ZapStr *)userinfo_p;
    const ZapStr *host = (const ZapStr *)host_p;
    const ZapStr *path = (const ZapStr *)path_p;
    const ZapStr *query = (const ZapStr *)query_p;
    const ZapStr *fragment = (const ZapStr *)fragment_p;

    /* generous upper bound: all components + separators + "[]" + ":65535". */
    int64_t cap = scheme->len + userinfo->len + host->len + path->len +
                  query->len + fragment->len + 16;
    char *out = (char *)zap_buf(cap);
    int64_t o = 0;

    if (scheme->len > 0) {
        memcpy(out + o, scheme->data, (size_t)scheme->len);
        o += scheme->len;
        out[o++] = ':';
    }
    if (has_authority) {
        out[o++] = '/';
        out[o++] = '/';
        if (userinfo->len > 0) {
            memcpy(out + o, userinfo->data, (size_t)userinfo->len);
            o += userinfo->len;
            out[o++] = '@';
        }
        int ipv6 = 0;
        for (int64_t i = 0; i < host->len; i++) {
            if (host->data[i] == ':') {
                ipv6 = 1;
                break;
            }
        }
        if (ipv6) {
            out[o++] = '[';
        }
        memcpy(out + o, host->data, (size_t)host->len);
        o += host->len;
        if (ipv6) {
            out[o++] = ']';
        }
        if (port >= 0) {
            out[o++] = ':';
            char tmp[6];
            int t = 0;
            int64_t v = port;
            if (v == 0) {
                tmp[t++] = '0';
            }
            while (v > 0) {
                tmp[t++] = (char)('0' + (v % 10));
                v /= 10;
            }
            while (t > 0) {
                out[o++] = tmp[--t];
            }
        }
    }
    memcpy(out + o, path->data, (size_t)path->len);
    o += path->len;
    if (query->len > 0) {
        out[o++] = '?';
        memcpy(out + o, query->data, (size_t)query->len);
        o += query->len;
    }
    if (fragment->len > 0) {
        out[o++] = '#';
        memcpy(out + o, fragment->data, (size_t)fragment->len);
        o += fragment->len;
    }
    return zap_str_new(out, o);
}

/* Parse `k=v&k=v` into a dict. `multi` accumulates every value under its key in
 * a `list[str]` (Python `parse_qs`); otherwise the first value wins and the map
 * is `dict[str,str]` (a scalar convenience). Keys and values are `unquote_plus`-
 * decoded (`%XX`, `+`->space); a malformed `%XX` is left literal (lenient, like
 * `zap_url_unquote`). A pair with no '=' keeps the key with an empty value, and
 * an empty value is kept (blank-value-friendly, Go `url.ParseQuery` form, not
 * Python's default drop). Empty segments (`&&`, leading/trailing `&`) are
 * skipped. Both ';' and '&' are NOT both separators — only '&' (Python 3.10+). */
static void *zap_url_parse_query_impl(void *qp, int multi) {
    const ZapStr *q = (const ZapStr *)qp;
    const char *p = q->data;
    int64_t n = q->len;
    void *d = zap_dict_new();
    int64_t i = 0;
    while (i < n) {
        /* one `seg` = bytes up to the next '&' (or end). */
        int64_t seg_start = i;
        while (i < n && p[i] != '&') {
            i++;
        }
        int64_t seg_end = i;
        if (i < n) {
            i++; /* skip '&' */
        }
        if (seg_end == seg_start) {
            continue; /* empty segment */
        }
        /* split the segment on the FIRST '='. */
        int64_t eq = -1;
        for (int64_t j = seg_start; j < seg_end; j++) {
            if (p[j] == '=') {
                eq = j;
                break;
            }
        }
        const char *kptr;
        int64_t klen;
        const char *vptr;
        int64_t vlen;
        if (eq < 0) {
            kptr = p + seg_start;
            klen = seg_end - seg_start;
            vptr = p + seg_end;
            vlen = 0;
        } else {
            kptr = p + seg_start;
            klen = eq - seg_start;
            vptr = p + eq + 1;
            vlen = seg_end - (eq + 1);
        }
        /* `zap_url_decode` takes a ZapStr*, so wrap the slices first. */
        void *kraw = zap_str_new(kptr, klen);
        void *vraw = zap_str_new(vptr, vlen);
        void *key = zap_url_decode(kraw, 1);
        void *val = zap_url_decode(vraw, 1);
        if (multi) {
            void *lst = NULL;
            if (zap_dict_contains_str(d, key)) {
                lst = zap_dict_get_str_ptr(d, key);
            }
            if (lst == NULL) {
                lst = zap_list_new();
                zap_dict_set_str_ptr(d, key, lst);
            }
            zap_list_push_ptr(lst, val);
        } else {
            if (!zap_dict_contains_str(d, key)) {
                zap_dict_set_str_ptr(d, key, val);
            }
        }
    }
    return d;
}

void *zap_url_parse_query(void *qp) { return zap_url_parse_query_impl(qp, 1); }
void *zap_url_parse_query_first(void *qp) { return zap_url_parse_query_impl(qp, 0); }

/* Build `k=v&...` from a `dict[str, list[str]]`, percent-encoding every key and
 * value with `zap_url_quote` and emitting one `k=v` per value (repeated keys),
 * in the dict's iteration order (unspecified). Companion to `encode_query`
 * (which is `dict[str,str]`); no sorting (use `canonical_query` for that). */
void *zap_url_encode_query_multi(void *dp) {
    void *keys = zap_dict_keys_ptr(dp);
    int64_t m = zap_list_len(keys);
    void *parts = zap_list_new();
    int first = 1;
    for (int64_t i = 0; i < m; i++) {
        void *k = zap_list_get_ptr(keys, i);
        void *ek = zap_url_quote(k);
        void *vals = zap_dict_get_str_ptr(dp, k); /* ZapList* of ZapStr* */
        int64_t vn = (vals != NULL) ? zap_list_len(vals) : 0;
        if (vn == 0) {
            /* a key mapped to an empty list emits nothing (no `k=`). */
            continue;
        }
        for (int64_t j = 0; j < vn; j++) {
            if (!first) {
                zap_list_push_ptr(parts, zap_str_new("&", 1));
            }
            first = 0;
            zap_list_push_ptr(parts, ek);
            zap_list_push_ptr(parts, zap_str_new("=", 1));
            zap_list_push_ptr(parts, zap_url_quote(zap_list_get_ptr(vals, j)));
        }
    }
    return zap_json_join(parts);
}

/* RFC-3986 §5.2.4 remove_dot_segments over a path (`in`,`n`) into `out`; returns
 * the output length. `out` must have room for at least `n` bytes. */
static int64_t zap_url_remove_dots(const char *in, int64_t n, char *out) {
    int64_t i = 0, o = 0;
    while (i < n) {
        int64_t rem = n - i;
        if (rem >= 3 && in[i] == '.' && in[i + 1] == '.' && in[i + 2] == '/') {
            i += 3; /* A: strip leading "../" */
        } else if (rem >= 2 && in[i] == '.' && in[i + 1] == '/') {
            i += 2; /* A: strip leading "./" */
        } else if (rem >= 3 && in[i] == '/' && in[i + 1] == '.' && in[i + 2] == '/') {
            i += 2; /* B: "/./" -> "/" (keep one leading '/') */
        } else if (rem == 2 && in[i] == '/' && in[i + 1] == '.') {
            /* B: trailing "/." -> "/" */
            out[o++] = '/';
            i += 2;
        } else if (rem >= 4 && in[i] == '/' && in[i + 1] == '.' && in[i + 2] == '.' &&
                   in[i + 3] == '/') {
            /* C: "/../" -> "/", and pop the last output segment. */
            i += 3;
            while (o > 0 && out[o - 1] != '/') {
                o--;
            }
            if (o > 0) {
                o--; /* drop the trailing '/' of the popped segment */
            }
        } else if (rem == 3 && in[i] == '/' && in[i + 1] == '.' && in[i + 2] == '.') {
            /* C: trailing "/.." -> "/", pop last segment. */
            i += 3;
            while (o > 0 && out[o - 1] != '/') {
                o--;
            }
            if (o > 0) {
                o--;
            }
            out[o++] = '/';
        } else if (rem == 1 && in[i] == '.') {
            i += 1; /* D: lone "." */
        } else if (rem == 2 && in[i] == '.' && in[i + 1] == '.') {
            i += 2; /* D: lone ".." */
        } else {
            /* E: move the first path segment (leading '/' + up to next '/'). */
            if (in[i] == '/') {
                out[o++] = '/';
                i++;
            }
            while (i < n && in[i] != '/') {
                out[o++] = in[i++];
            }
        }
    }
    return o;
}

/* Resolve a (possibly relative) reference `ref` against a base URL `base` per
 * RFC-3986 §5.2.2 (`url.join` / Python `urljoin` / Go `ResolveReference`).
 * Returns the target as a fresh `str`. */
void *zap_url_join(void *base_p, void *ref_p) {
    ZapUrlFields b, r;
    zap_url_split_fields(base_p, &b);
    zap_url_split_fields(ref_p, &r);

    /* Transform-reference (§5.2.2). We assemble the target components as slices,
     * with a scratch buffer for any merged/dot-removed path. */
    const char *t_scheme;
    int64_t t_scheme_len;
    int t_auth;
    const char *t_userinfo;
    int64_t t_userinfo_len;
    const char *t_host;
    int64_t t_host_len;
    int64_t t_port;
    const char *t_query;
    int64_t t_query_len;

    /* path scratch: max of base+ref path lengths + a joining '/'. */
    int64_t pbuf_cap = b.path_len + r.path_len + 2;
    char *merged = (char *)zap_buf(pbuf_cap > 0 ? pbuf_cap : 1);
    int64_t merged_len = 0;
    char *t_path_buf;
    int64_t t_path_len;

    int ref_has_scheme = (r.scheme_len > 0);
    if (ref_has_scheme) {
        t_scheme = r.scheme;
        t_scheme_len = r.scheme_len;
        t_auth = r.has_authority;
        t_userinfo = r.userinfo;
        t_userinfo_len = r.userinfo_len;
        t_host = r.host;
        t_host_len = r.host_len;
        t_port = r.port;
        t_query = r.query;
        t_query_len = r.query_len;
        /* path = remove_dot_segments(ref.path) */
        t_path_buf = (char *)zap_buf(r.path_len > 0 ? r.path_len : 1);
        t_path_len = zap_url_remove_dots(r.path, r.path_len, t_path_buf);
    } else {
        t_scheme = b.scheme;
        t_scheme_len = b.scheme_len;
        if (r.has_authority) {
            t_auth = 1;
            t_userinfo = r.userinfo;
            t_userinfo_len = r.userinfo_len;
            t_host = r.host;
            t_host_len = r.host_len;
            t_port = r.port;
            t_query = r.query;
            t_query_len = r.query_len;
            t_path_buf = (char *)zap_buf(r.path_len > 0 ? r.path_len : 1);
            t_path_len = zap_url_remove_dots(r.path, r.path_len, t_path_buf);
        } else {
            t_auth = b.has_authority;
            t_userinfo = b.userinfo;
            t_userinfo_len = b.userinfo_len;
            t_host = b.host;
            t_host_len = b.host_len;
            t_port = b.port;
            if (r.path_len == 0) {
                /* keep base path; query from ref if ref has one, else base. */
                t_path_buf = (char *)(b.path_len > 0 ? zap_buf(b.path_len) : zap_buf(1));
                if (b.path_len > 0) {
                    memcpy(t_path_buf, b.path, (size_t)b.path_len);
                }
                t_path_len = b.path_len;
                if (r.query != NULL) {
                    t_query = r.query;
                    t_query_len = r.query_len;
                } else {
                    t_query = b.query;
                    t_query_len = b.query_len;
                }
            } else {
                t_query = r.query;
                t_query_len = r.query_len;
                if (r.path[0] == '/') {
                    t_path_buf = (char *)zap_buf(r.path_len);
                    t_path_len = zap_url_remove_dots(r.path, r.path_len, t_path_buf);
                } else {
                    /* merge(base): if base has an authority and an empty path,
                     * the merged path is "/" + ref.path; otherwise it is base's
                     * path up to and including its last '/', then ref.path. */
                    if (b.has_authority && b.path_len == 0) {
                        merged[merged_len++] = '/';
                        memcpy(merged + merged_len, r.path, (size_t)r.path_len);
                        merged_len += r.path_len;
                    } else {
                        int64_t last_slash = -1;
                        for (int64_t k = 0; k < b.path_len; k++) {
                            if (b.path[k] == '/') {
                                last_slash = k;
                            }
                        }
                        if (last_slash >= 0) {
                            memcpy(merged, b.path, (size_t)(last_slash + 1));
                            merged_len = last_slash + 1;
                        }
                        memcpy(merged + merged_len, r.path, (size_t)r.path_len);
                        merged_len += r.path_len;
                    }
                    t_path_buf = (char *)zap_buf(merged_len > 0 ? merged_len : 1);
                    t_path_len = zap_url_remove_dots(merged, merged_len, t_path_buf);
                }
            }
        }
    }

    /* fragment is always the reference's. */
    const char *t_frag = r.fragment;
    int64_t t_frag_len = r.fragment_len;

    /* Reassemble (same shape as zap_url_build, but from slices). */
    int64_t cap = t_scheme_len + t_userinfo_len + t_host_len + t_path_len +
                  t_query_len + t_frag_len + 16;
    char *out = (char *)zap_buf(cap);
    int64_t o = 0;
    if (t_scheme_len > 0) {
        memcpy(out + o, t_scheme, (size_t)t_scheme_len);
        o += t_scheme_len;
        out[o++] = ':';
    }
    if (t_auth) {
        out[o++] = '/';
        out[o++] = '/';
        if (t_userinfo_len > 0) {
            memcpy(out + o, t_userinfo, (size_t)t_userinfo_len);
            o += t_userinfo_len;
            out[o++] = '@';
        }
        int ipv6 = 0;
        for (int64_t k = 0; k < t_host_len; k++) {
            if (t_host[k] == ':') {
                ipv6 = 1;
                break;
            }
        }
        if (ipv6) {
            out[o++] = '[';
        }
        memcpy(out + o, t_host, (size_t)t_host_len);
        o += t_host_len;
        if (ipv6) {
            out[o++] = ']';
        }
        if (t_port >= 0) {
            out[o++] = ':';
            char tmp[6];
            int t = 0;
            int64_t v = t_port;
            if (v == 0) {
                tmp[t++] = '0';
            }
            while (v > 0) {
                tmp[t++] = (char)('0' + (v % 10));
                v /= 10;
            }
            while (t > 0) {
                out[o++] = tmp[--t];
            }
        }
    }
    memcpy(out + o, t_path_buf, (size_t)t_path_len);
    o += t_path_len;
    /* emit '?' iff the target carries a query component (NULL => absent; a
     * present-but-empty query "?" is preserved, RFC-3986 §5.3). */
    if (t_query != NULL) {
        out[o++] = '?';
        memcpy(out + o, t_query, (size_t)t_query_len);
        o += t_query_len;
    }
    if (t_frag != NULL) {
        out[o++] = '#';
        memcpy(out + o, t_frag, (size_t)t_frag_len);
        o += t_frag_len;
    }
    return zap_str_new(out, o);
}

