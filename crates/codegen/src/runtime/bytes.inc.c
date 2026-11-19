/* zap runtime fragment: bytes.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * bytes + base64 + hex (ticket #337). `bytes` reuses the ZapStr length-prefixed
 * byte buffer; these entry points give it byte-wise indexing/slicing/decoding,
 * construction from a list of ints, and the base64 / hex transfer codecs. Every
 * result is a freshly allocated ZapStr over a pointer-free leaf buffer, so the
 * precise collector traces it correctly. `bytes` and `str` share the ZapStr
 * object — the only runtime distinction is these byte-wise helpers.
 * =========================================================================== */

/* Raise a catchable Zap exception carrying `msg` (so a bad codec input behaves
 * like `raise`: a `try/except` can catch it, and an uncaught one exits 1 with
 * the message, ticket #337). Never returns. */
__attribute__((noreturn)) static void zap_bytes_raise(const char *msg) {
    zap_panic(zap_str_new(msg, (int64_t)strlen(msg)));
    abort(); /* unreachable: zap_panic unwinds and never returns */
}

/* b[i] -> the byte at byte index `i` as an int64 in 0..=255. Negative indices
 * count from the end; out-of-range faults (matches zap_str_index behavior). */
int64_t zap_bytes_index(void *bp, int64_t idx) {
    const ZapStr *b = (const ZapStr *)bp;
    int64_t n = b->len;
    if (idx < 0) {
        idx += n;
    }
    if (idx < 0 || idx >= n) {
        fprintf(stderr, "zap: bytes index out of range\n");
        abort();
    }
    return (int64_t)(unsigned char)b->data[idx];
}

/* b[lo:hi:step] -> bytes, Python slice semantics by byte. Mirrors
 * zap_str_slice but indexes raw bytes (no UTF-8 decoding). */
void *zap_bytes_slice(void *bp, int64_t lo, int64_t hi, int64_t step, int64_t has_lo,
                      int64_t has_hi, int64_t has_step) {
    const ZapStr *b = (const ZapStr *)bp;
    int64_t n = b->len;
    int64_t st = has_step ? step : 1;
    if (st == 0) {
        fprintf(stderr, "zap: slice step cannot be zero\n");
        abort();
    }
    int64_t start;
    int64_t stop;
    if (st > 0) {
        start = has_lo ? zap_slice_pos(lo, n) : 0;
        stop = has_hi ? zap_slice_pos(hi, n) : n;
    } else {
        start = has_lo ? zap_slice_neg(lo, n) : n - 1;
        stop = has_hi ? zap_slice_neg(hi, n) : -1;
    }
    if (st == 1) {
        if (start >= stop) {
            return zap_str_new("", 0);
        }
        return zap_str_new(b->data + start, stop - start);
    }
    char *buf = (char *)zap_buf(n > 0 ? n : 1);
    int64_t outlen = 0;
    for (int64_t k = start; (st > 0) ? (k < stop) : (k > stop); k += st) {
        buf[outlen++] = b->data[k];
    }
    return zap_str_adopt(buf, outlen);
}

/* bytes([...]): build bytes from a list of int64 element words, each validated
 * to lie in 0..=255 (Python raises ValueError; v1 faults). */
void *zap_bytes_from_list(void *lp) {
    ZapList *l = (ZapList *)lp;
    int64_t n = l->len;
    char *buf = (char *)zap_buf(n > 0 ? n : 1);
    for (int64_t i = 0; i < n; i++) {
        int64_t v = l->data[i];
        if (v < 0 || v > 255) {
            zap_bytes_raise("bytes(): list element out of range(0..256)");
        }
        buf[i] = (char)(unsigned char)v;
    }
    return zap_str_adopt(buf, n);
}

/* Strict UTF-8 well-formedness check (RFC 3629): rejects overlong encodings,
 * surrogates (U+D800..U+DFFF) and code points above U+10FFFF. On failure returns
 * 0 and sets *bad to the offending byte offset. */
static int zap_utf8_valid(const unsigned char *d, int64_t n, int64_t *bad) {
    int64_t i = 0;
    while (i < n) {
        unsigned char c = d[i];
        if (c < 0x80) {
            i++;
            continue;
        }
        int w;
        uint32_t cp;
        uint32_t min;
        if ((c & 0xE0) == 0xC0) {
            w = 2;
            cp = c & 0x1F;
            min = 0x80;
        } else if ((c & 0xF0) == 0xE0) {
            w = 3;
            cp = c & 0x0F;
            min = 0x800;
        } else if ((c & 0xF8) == 0xF0) {
            w = 4;
            cp = c & 0x07;
            min = 0x10000;
        } else {
            *bad = i;
            return 0;
        }
        if (i + w > n) {
            *bad = i;
            return 0;
        }
        for (int k = 1; k < w; k++) {
            unsigned char cc = d[i + k];
            if ((cc & 0xC0) != 0x80) {
                *bad = i + k;
                return 0;
            }
            cp = (cp << 6) | (uint32_t)(cc & 0x3F);
        }
        if (cp < min || cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF)) {
            *bad = i;
            return 0;
        }
        i += w;
    }
    return 1;
}

/* str(bytes): validate the bytes are well-formed UTF-8 (faulting with a clear
 * error otherwise) and return them as a str. str/bytes share the ZapStr layout,
 * so the result is a fresh str over a copy of the same bytes. */
void *zap_bytes_to_str(void *bp) {
    const ZapStr *b = (const ZapStr *)bp;
    int64_t bad = 0;
    if (!zap_utf8_valid((const unsigned char *)b->data, b->len, &bad)) {
        zap_bytes_raise("str(bytes): invalid UTF-8");
    }
    return zap_str_new(b->data, b->len);
}

/* bytes.maketrans(frm, to) -> a freshly-allocated 256-byte translation table:
 * the identity map (table[i] == i) except that each byte frm[i] maps to to[i]
 * (ticket #1099, Python `bytes.maketrans` parity). `frm` and `to` must have
 * equal length; a mismatch raises a catchable error (Python raises ValueError).
 * A later frm[i] overrides an earlier one for the same source byte, matching
 * CPython's left-to-right fill. The result is a fresh 256-byte ZapStr/bytes. */
void *zap_bytes_maketrans(void *fp, void *tp) {
    const ZapStr *frm = (const ZapStr *)fp;
    const ZapStr *to = (const ZapStr *)tp;
    if (frm->len != to->len) {
        const char *m = "bytes.maketrans(): arguments must have equal length";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    char *buf = (char *)zap_buf(256);
    for (int i = 0; i < 256; i++) {
        buf[i] = (char)(unsigned char)i;
    }
    for (int64_t i = 0; i < frm->len; i++) {
        buf[(unsigned char)frm->data[i]] = to->data[i];
    }
    return zap_str_adopt(buf, 256);
}

/* seq.translate(table) / seq.translate(table, delete) -> bytes (ticket #1099,
 * Python `bytes.translate` parity). `table` must be exactly 256 bytes (a mismatch
 * raises a catchable error). The whole buffer is rewritten in a single native
 * pass: out[i] = table[seq[i]]. With a `delete` set (`has_delete` != 0), any
 * input byte whose value appears in `delete` is dropped from the output BEFORE
 * translation — i.e. deletion wins over remapping, matching CPython. Bytes are
 * treated unsigned (0..=255), so the high bytes 0x80..0xFF index the table
 * correctly with no sign extension. The result is a fresh ZapStr/bytes. */
void *zap_bytes_translate(void *sp, void *tp, void *dp, int64_t has_delete) {
    const ZapStr *s = (const ZapStr *)sp;
    const ZapStr *table = (const ZapStr *)tp;
    if (table->len != 256) {
        const char *m = "bytes.translate(): table must be exactly 256 bytes";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    int64_t n = s->len;
    char *buf = (char *)zap_buf(n > 0 ? n : 1);
    int64_t outlen = 0;
    if (has_delete) {
        const ZapStr *del = (const ZapStr *)dp;
        unsigned char drop[256];
        memset(drop, 0, sizeof(drop));
        for (int64_t i = 0; i < del->len; i++) {
            drop[(unsigned char)del->data[i]] = 1;
        }
        for (int64_t i = 0; i < n; i++) {
            unsigned char c = (unsigned char)s->data[i];
            if (drop[c]) {
                continue;
            }
            buf[outlen++] = table->data[c];
        }
    } else {
        for (int64_t i = 0; i < n; i++) {
            unsigned char c = (unsigned char)s->data[i];
            buf[outlen++] = table->data[c];
        }
    }
    return zap_str_adopt(buf, outlen);
}

/* ---- base64 (RFC 4648) ---- */
static const char ZAP_B64_STD[65] =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
static const char ZAP_B64_URL[65] =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/* encode(data, urlsafe, pad) -> str. `urlsafe` picks the `-_` alphabet; `pad`
 * adds the `=` padding to a multiple of 4 chars. */
void *zap_base64_encode(void *bp, int64_t urlsafe, int64_t pad) {
    const ZapStr *b = (const ZapStr *)bp;
    const unsigned char *d = (const unsigned char *)b->data;
    int64_t n = b->len;
    const char *alpha = urlsafe ? ZAP_B64_URL : ZAP_B64_STD;
    int64_t groups = (n + 2) / 3;
    char *out = (char *)zap_buf(groups * 4 + 1);
    int64_t o = 0;
    int64_t i = 0;
    while (i + 3 <= n) {
        uint32_t v =
            ((uint32_t)d[i] << 16) | ((uint32_t)d[i + 1] << 8) | (uint32_t)d[i + 2];
        out[o++] = alpha[(v >> 18) & 0x3F];
        out[o++] = alpha[(v >> 12) & 0x3F];
        out[o++] = alpha[(v >> 6) & 0x3F];
        out[o++] = alpha[v & 0x3F];
        i += 3;
    }
    int64_t rem = n - i;
    if (rem == 1) {
        uint32_t v = (uint32_t)d[i] << 16;
        out[o++] = alpha[(v >> 18) & 0x3F];
        out[o++] = alpha[(v >> 12) & 0x3F];
        if (pad) {
            out[o++] = '=';
            out[o++] = '=';
        }
    } else if (rem == 2) {
        uint32_t v = ((uint32_t)d[i] << 16) | ((uint32_t)d[i + 1] << 8);
        out[o++] = alpha[(v >> 18) & 0x3F];
        out[o++] = alpha[(v >> 12) & 0x3F];
        out[o++] = alpha[(v >> 6) & 0x3F];
        if (pad) {
            out[o++] = '=';
        }
    }
    return zap_str_adopt(out, o);
}

/* The 6-bit value of a base64 character, or -1. Accepts both the standard
 * (`+`/`/`) and URL-safe (`-`/`_`) alphabets, so decode is lenient about which
 * variant it is given. */
static int zap_b64_val(unsigned char c) {
    if (c >= 'A' && c <= 'Z') {
        return c - 'A';
    }
    if (c >= 'a' && c <= 'z') {
        return c - 'a' + 26;
    }
    if (c >= '0' && c <= '9') {
        return c - '0' + 52;
    }
    if (c == '+' || c == '-') {
        return 62;
    }
    if (c == '/' || c == '_') {
        return 63;
    }
    return -1;
}

/* decode(s, urlsafe) -> bytes. Padding is optional (the bit count drives the
 * output), ASCII whitespace/newlines are skipped, and an invalid character
 * faults. The `urlsafe` flag is accepted for API symmetry; both alphabets are
 * always decoded. */
void *zap_base64_decode(void *sp, int64_t urlsafe) {
    (void)urlsafe;
    const ZapStr *s = (const ZapStr *)sp;
    const unsigned char *d = (const unsigned char *)s->data;
    int64_t n = s->len;
    unsigned char *out = (unsigned char *)zap_buf((n / 4 + 1) * 3 + 3);
    int64_t o = 0;
    uint32_t acc = 0;
    int bits = 0;
    for (int64_t i = 0; i < n; i++) {
        unsigned char c = d[i];
        if (c == '=') {
            break;
        }
        if (c == '\n' || c == '\r' || c == ' ' || c == '\t') {
            continue;
        }
        int v = zap_b64_val(c);
        if (v < 0) {
            zap_bytes_raise("base64.decode: invalid character");
        }
        acc = (acc << 6) | (uint32_t)v;
        bits += 6;
        if (bits >= 8) {
            bits -= 8;
            out[o++] = (unsigned char)((acc >> bits) & 0xFF);
        }
    }
    return zap_str_adopt((char *)out, o);
}

/* ---- hex ---- */
/* encode(data) -> lowercase hex str (two chars per byte). */
void *zap_hex_encode(void *bp) {
    static const char HEX[] = "0123456789abcdef";
    const ZapStr *b = (const ZapStr *)bp;
    const unsigned char *d = (const unsigned char *)b->data;
    int64_t n = b->len;
    char *out = (char *)zap_buf(n * 2 > 0 ? n * 2 : 1);
    for (int64_t i = 0; i < n; i++) {
        out[i * 2] = HEX[(d[i] >> 4) & 0xF];
        out[i * 2 + 1] = HEX[d[i] & 0xF];
    }
    return zap_str_adopt(out, n * 2);
}

static int zap_hex_val(unsigned char c) {
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

/* decode(s) -> bytes. Requires an even number of hex digits ([0-9a-fA-F]);
 * malformed input faults. */
void *zap_hex_decode(void *sp) {
    const ZapStr *s = (const ZapStr *)sp;
    const unsigned char *d = (const unsigned char *)s->data;
    int64_t n = s->len;
    if (n % 2 != 0) {
        zap_bytes_raise("hex.decode: odd-length hex string");
    }
    int64_t m = n / 2;
    char *out = (char *)zap_buf(m > 0 ? m : 1);
    for (int64_t i = 0; i < m; i++) {
        int hi = zap_hex_val(d[i * 2]);
        int lo = zap_hex_val(d[i * 2 + 1]);
        if (hi < 0 || lo < 0) {
            zap_bytes_raise("hex.decode: invalid hex digit");
        }
        out[i] = (char)((hi << 4) | lo);
    }
    return zap_str_adopt(out, m);
}

