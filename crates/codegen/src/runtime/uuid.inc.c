/* ===========================================================================
 * uuid.inc.c — the `uuid` stdlib module runtime (ticket #975, D-0232/D-0233).
 *
 * RFC 4122 / RFC 9562 UUIDs (v4/v7/v1/v3/v5, parse, nil, max, the Appendix C
 * namespace constants) plus NanoID (ai/nanoid semantics). A UUID value is 16
 * raw bytes returned as a `bytes` ZapStr; the Zap-level `Uuid` prelude struct
 * (crates/syntax/src/parser.rs PRELUDE_SRC) wraps those bytes and the accessors
 * below format / inspect them.
 *
 * This fragment is textually `#include`d by zaprt.c AFTER crypto.inc.c, so it
 * reuses that file's primitives directly rather than duplicating them:
 *   - entropy      : zap_crypto_fill_random()  (the OS CSPRNG: getentropy /
 *                    /dev/urandom / BCryptGenRandom)
 *   - MD5 / SHA-1  : zap_hash_ctx_{init,update,final}() with ZAP_CRY_MD5 /
 *                    ZAP_CRY_SHA1 (used by uuid3 / uuid5)
 * Core runtime helpers (ZapStr, zap_str_alloc/new, zap_panic_typed, the
 * ZAP_EXC_* codes) come from earlier in the single translation unit. <time.h>
 * is included at the top of zaprt.c, so clock_gettime is available here.
 * =========================================================================== */

/* Set the RFC 4122 version nibble (high nibble of byte 6) and the variant bits
 * (top two bits of byte 8 = 10b), in place, on a 16-byte buffer. */
static void zap_uuid_stamp(uint8_t *b, int version) {
    b[6] = (uint8_t)((b[6] & 0x0f) | ((version & 0x0f) << 4));
    b[8] = (uint8_t)((b[8] & 0x3f) | 0x80);
}

/* Wall-clock milliseconds since the Unix epoch (for uuid7). */
static int64_t zap_uuid_unix_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (int64_t)ts.tv_sec * 1000LL + (int64_t)ts.tv_nsec / 1000000LL;
}

/* ---- generators (each returns a 16-byte `bytes` ZapStr) ---- */

void *zap_uuid_v4(void) {
    ZapStr *s = zap_str_alloc(16);
    zap_crypto_fill_random((uint8_t *)s->data, 16);
    zap_uuid_stamp((uint8_t *)s->data, 4);
    return s;
}

void *zap_uuid_v7(void) {
    ZapStr *s = zap_str_alloc(16);
    uint8_t *b = (uint8_t *)s->data;
    /* rand_a / rand_b: the whole thing is random, then the 48-bit big-endian
     * unix-ms timestamp overwrites bytes 0..6 and the version/variant nibbles
     * are stamped (RFC 9562 §5.7). */
    zap_crypto_fill_random(b, 16);
    int64_t ms = zap_uuid_unix_ms();
    b[0] = (uint8_t)(ms >> 40);
    b[1] = (uint8_t)(ms >> 32);
    b[2] = (uint8_t)(ms >> 24);
    b[3] = (uint8_t)(ms >> 16);
    b[4] = (uint8_t)(ms >> 8);
    b[5] = (uint8_t)(ms);
    zap_uuid_stamp(b, 7);
    return s;
}

void *zap_uuid_v1(void) {
    /* RFC 4122 §4.2 / §4.5: 60-bit timestamp in 100-ns intervals since the
     * Gregorian epoch (1582-10-15 00:00 UTC), a random 14-bit clock sequence,
     * and a random 48-bit node with the multicast/local bit set so no hardware
     * MAC is ever disclosed (the §4.5 "no IEEE 802 address available" path). */
    ZapStr *s = zap_str_alloc(16);
    uint8_t *b = (uint8_t *)s->data;
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    /* 100-ns ticks since 1582-10-15; offset from the Unix epoch is
     * 0x01B21DD213814000 (122192928000000000 ticks). */
    uint64_t ticks = (uint64_t)ts.tv_sec * 10000000ULL +
                     (uint64_t)ts.tv_nsec / 100ULL + 0x01B21DD213814000ULL;
    uint32_t time_low = (uint32_t)(ticks & 0xffffffffULL);
    uint16_t time_mid = (uint16_t)((ticks >> 32) & 0xffffULL);
    uint16_t time_hi = (uint16_t)((ticks >> 48) & 0x0fffULL);
    b[0] = (uint8_t)(time_low >> 24);
    b[1] = (uint8_t)(time_low >> 16);
    b[2] = (uint8_t)(time_low >> 8);
    b[3] = (uint8_t)(time_low);
    b[4] = (uint8_t)(time_mid >> 8);
    b[5] = (uint8_t)(time_mid);
    b[6] = (uint8_t)(time_hi >> 8);
    b[7] = (uint8_t)(time_hi);
    /* clock_seq (14 bits) + node (48 bits): random. */
    uint8_t rnd[8];
    zap_crypto_fill_random(rnd, 8);
    b[8] = rnd[0];
    b[9] = rnd[1];
    b[10] = (uint8_t)(rnd[2] | 0x01); /* multicast/local bit → random node */
    b[11] = rnd[3];
    b[12] = rnd[4];
    b[13] = rnd[5];
    b[14] = rnd[6];
    b[15] = rnd[7];
    zap_uuid_stamp(b, 1); /* version 1 + variant 10b */
    return s;
}

/* Namespaced v3 (MD5) / v5 (SHA-1): hash(namespace-bytes || name), take the
 * leading 16 bytes, stamp the version + variant (RFC 4122 §4.3). */
static void *zap_uuid_namespaced(int algo, int version, void *ns_raw, void *name) {
    ZapStr *ns = (ZapStr *)ns_raw;
    ZapStr *nm = (ZapStr *)name;
    zap_hash_ctx c;
    zap_hash_ctx_init(&c, algo);
    zap_hash_ctx_update(&c, (const uint8_t *)ns->data, (size_t)ns->len);
    zap_hash_ctx_update(&c, (const uint8_t *)nm->data, (size_t)nm->len);
    uint8_t digest[64];
    zap_hash_ctx_final(&c, digest);
    ZapStr *s = zap_str_alloc(16);
    memcpy(s->data, digest, 16);
    zap_uuid_stamp((uint8_t *)s->data, version);
    return s;
}

void *zap_uuid_v3(void *ns_raw, void *name) {
    return zap_uuid_namespaced(ZAP_CRY_MD5, 3, ns_raw, name);
}

void *zap_uuid_v5(void *ns_raw, void *name) {
    return zap_uuid_namespaced(ZAP_CRY_SHA1, 5, ns_raw, name);
}

void *zap_uuid_nil(void) {
    ZapStr *s = zap_str_alloc(16);
    memset(s->data, 0x00, 16);
    return s;
}

void *zap_uuid_max(void) {
    ZapStr *s = zap_str_alloc(16);
    memset(s->data, 0xff, 16);
    return s;
}

/* The RFC 4122 Appendix C namespace UUIDs, returned as live 16-byte values so
 * `uuid.uuid3(uuid.namespace_dns(), name)` works out of the box. */
static void *zap_uuid_const16(const uint8_t src[16]) {
    ZapStr *s = zap_str_alloc(16);
    memcpy(s->data, src, 16);
    return s;
}
void *zap_uuid_ns_dns(void) {
    static const uint8_t v[16] = {0x6b, 0xa7, 0xb8, 0x10, 0x9d, 0xad, 0x11, 0xd1,
                                  0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8};
    return zap_uuid_const16(v);
}
void *zap_uuid_ns_url(void) {
    static const uint8_t v[16] = {0x6b, 0xa7, 0xb8, 0x11, 0x9d, 0xad, 0x11, 0xd1,
                                  0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8};
    return zap_uuid_const16(v);
}
void *zap_uuid_ns_oid(void) {
    static const uint8_t v[16] = {0x6b, 0xa7, 0xb8, 0x12, 0x9d, 0xad, 0x11, 0xd1,
                                  0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8};
    return zap_uuid_const16(v);
}
void *zap_uuid_ns_x500(void) {
    static const uint8_t v[16] = {0x6b, 0xa7, 0xb8, 0x14, 0x9d, 0xad, 0x11, 0xd1,
                                  0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8};
    return zap_uuid_const16(v);
}

/* ---- accessors over the 16 raw bytes ---- */

static const char ZAP_UUID_HEX[16] = {'0', '1', '2', '3', '4', '5', '6', '7',
                                      '8', '9', 'a', 'b', 'c', 'd', 'e', 'f'};

/* Canonical 8-4-4-4-12 lowercase hyphenated form. */
void *zap_uuid_format(void *raw) {
    ZapStr *r = (ZapStr *)raw;
    const uint8_t *b = (const uint8_t *)r->data;
    ZapStr *out = zap_str_alloc(36);
    char *o = out->data;
    static const int groups[5] = {4, 2, 2, 2, 6}; /* bytes per group */
    size_t bi = 0, oi = 0;
    for (int g = 0; g < 5; g++) {
        if (g != 0)
            o[oi++] = '-';
        for (int k = 0; k < groups[g]; k++) {
            uint8_t x = b[bi++];
            o[oi++] = ZAP_UUID_HEX[x >> 4];
            o[oi++] = ZAP_UUID_HEX[x & 0x0f];
        }
    }
    return out;
}

/* 32 lowercase hex chars, no hyphens. */
void *zap_uuid_hex(void *raw) {
    ZapStr *r = (ZapStr *)raw;
    const uint8_t *b = (const uint8_t *)r->data;
    ZapStr *out = zap_str_alloc(32);
    for (int i = 0; i < 16; i++) {
        out->data[2 * i] = ZAP_UUID_HEX[b[i] >> 4];
        out->data[2 * i + 1] = ZAP_UUID_HEX[b[i] & 0x0f];
    }
    return out;
}

/* "urn:uuid:" + canonical. */
void *zap_uuid_urn(void *raw) {
    ZapStr *canon = (ZapStr *)zap_uuid_format(raw);
    ZapStr *out = zap_str_alloc(9 + 36);
    memcpy(out->data, "urn:uuid:", 9);
    memcpy(out->data + 9, canon->data, 36);
    return out;
}

int64_t zap_uuid_version(void *raw) {
    ZapStr *r = (ZapStr *)raw;
    const uint8_t *b = (const uint8_t *)r->data;
    return (int64_t)((b[6] >> 4) & 0x0f);
}

/* 0 = NCS (0xx), 1 = RFC 4122 (10x), 2 = Microsoft (110), 3 = future (111). */
int64_t zap_uuid_variant(void *raw) {
    ZapStr *r = (ZapStr *)raw;
    uint8_t v = (uint8_t)r->data[8];
    if ((v & 0x80) == 0x00)
        return 0;
    if ((v & 0xc0) == 0x80)
        return 1;
    if ((v & 0xe0) == 0xc0)
        return 2;
    return 3;
}

/* ---- parse ---- */

static int zap_uuid_hexval(char c) {
    if (c >= '0' && c <= '9')
        return c - '0';
    if (c >= 'a' && c <= 'f')
        return c - 'a' + 10;
    if (c >= 'A' && c <= 'F')
        return c - 'A' + 10;
    return -1;
}

static void zap_uuid_parse_fail(void) {
    const char *m = "uuid.parse: not a valid UUID string";
    zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
}

/* Accepts the canonical hyphenated form, a `urn:uuid:` prefix, surrounding
 * braces `{...}`, and a bare 32-hex-digit form. Case-insensitive. Any other
 * shape (wrong length, stray characters, misplaced hyphens) raises ValueError. */
void *zap_uuid_parse(void *str) {
    ZapStr *s = (ZapStr *)str;
    const char *p = s->data;
    size_t n = (size_t)s->len;
    /* strip an optional urn:uuid: prefix (case-insensitive). */
    if (n >= 9) {
        const char *pre = "urn:uuid:";
        int match = 1;
        for (int i = 0; i < 9; i++) {
            char c = p[i];
            if (c >= 'A' && c <= 'Z')
                c = (char)(c - 'A' + 'a');
            if (c != pre[i]) {
                match = 0;
                break;
            }
        }
        if (match) {
            p += 9;
            n -= 9;
        }
    }
    /* strip surrounding braces. */
    if (n >= 2 && p[0] == '{' && p[n - 1] == '}') {
        p += 1;
        n -= 2;
    }
    uint8_t out[16];
    size_t bi = 0;
    if (n == 36) {
        /* hyphenated: hyphens must sit at 8,13,18,23. */
        static const int hy[4] = {8, 13, 18, 23};
        int hk = 0;
        for (size_t i = 0; i < 36;) {
            if (hk < 4 && i == (size_t)hy[hk]) {
                if (p[i] != '-')
                    zap_uuid_parse_fail();
                hk++;
                i++;
                continue;
            }
            int hi = zap_uuid_hexval(p[i]);
            int lo = zap_uuid_hexval(p[i + 1]);
            if (hi < 0 || lo < 0)
                zap_uuid_parse_fail();
            out[bi++] = (uint8_t)((hi << 4) | lo);
            i += 2;
        }
    } else if (n == 32) {
        for (size_t i = 0; i < 32; i += 2) {
            int hi = zap_uuid_hexval(p[i]);
            int lo = zap_uuid_hexval(p[i + 1]);
            if (hi < 0 || lo < 0)
                zap_uuid_parse_fail();
            out[bi++] = (uint8_t)((hi << 4) | lo);
        }
    } else {
        zap_uuid_parse_fail();
    }
    if (bi != 16)
        zap_uuid_parse_fail();
    return zap_str_new((const char *)out, 16);
}

/* ---- NanoID (ai/nanoid semantics) ----
 * The default URL-safe alphabet (ai/nanoid `urlAlphabet`, 64 chars) is supplied
 * by the compiler as a string constant for the 0-arg / size-only forms (see the
 * lowering facet crates/ir/src/lower/modules/uuid.rs), so it lives in exactly
 * one place. This function only ever sees an explicit alphabet. */

/* generate(alphabet, size): `size` characters drawn uniformly from `alphabet`
 * via masked rejection sampling over the OS CSPRNG. alphabet length must be in
 * 1..256 and size must be >= 0 (0 yields ""). */
void *zap_uuid_nanoid(void *alphabet, int64_t size) {
    ZapStr *al = (ZapStr *)alphabet;
    size_t len = (size_t)al->len;
    const uint8_t *chars = (const uint8_t *)al->data;
    if (size < 0) {
        const char *m = "uuid.nanoid: size must be non-negative";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    if (len < 1 || len > 256) {
        const char *m = "uuid.nanoid: alphabet length must be between 1 and 256";
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
    }
    ZapStr *out = zap_str_alloc(size);
    if (size == 0)
        return out;
    /* smallest (2^k - 1) >= len-1 */
    size_t k = 0;
    while (((size_t)1 << k) < len)
        k++;
    size_t mask = ((size_t)1 << k) - 1;
    /* step ~ ceil(1.6 * mask * size / len); clamp to >= 1 so len==1 terminates. */
    size_t step = (size_t)((1.6 * (double)mask * (double)size) / (double)len) + 1;
    if (step < 1)
        step = 1;
    uint8_t buf[256];
    size_t pos = 0;
    while (pos < (size_t)size) {
        size_t want = step;
        if (want > sizeof(buf))
            want = sizeof(buf);
        zap_crypto_fill_random(buf, want);
        for (size_t i = 0; i < want && pos < (size_t)size; i++) {
            size_t idx = (size_t)buf[i] & mask;
            if (idx < len)
                out->data[pos++] = (char)chars[idx];
        }
    }
    return out;
}
