/* zap runtime fragment: crypto.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
/* ===========================================================================
 * crypto module (ticket #105, D-0056): hashing, HMAC, secure random / secrets.
 * TLS is deferred (late-v1). Self-contained reference implementations of the
 * hashes (FIPS 180-4 SHA-256/SHA-512, FIPS 180-1 SHA-1, RFC 1321 MD5) so no new
 * external dependency is pulled in; HMAC is RFC 2104; the CSPRNG draws from the
 * OS entropy source (getentropy, falling back to /dev/urandom).
 *
 * str/bytes cross as ZapStr* (len-prefixed, binary-safe). Hex digests and raw
 * digests are returned as freshly built ZapStr objects (zap_str_new / a hex
 * helper). A streaming handle is a `bytes` value whose ZapStr `data` buffer
 * holds the hash context; hexdigest/digest finalize a COPY, so a handle may be
 * digested more than once and keeps accepting updates afterwards.
 * ========================================================================== */
#include <fcntl.h> /* open, O_RDONLY (CSPRNG fallback) */
#if defined(ZAP_PLATFORM_POSIX)
#include <sys/random.h> /* getentropy (Windows uses BCryptGenRandom, see below)  */
#endif

#define ZAP_CRY_SHA256 0
#define ZAP_CRY_SHA512 1
#define ZAP_CRY_SHA1 2
#define ZAP_CRY_MD5 3
/* SHA-384 / SHA-224 (ticket #765): truncated SHA-512 / SHA-256 with distinct IVs
 * (FIPS 180-4 §5.3.3/§5.3.2). They reuse the SHA-512 / SHA-256 block machinery
 * verbatim — only the initial hash value and the output length differ. */
#define ZAP_CRY_SHA384 4
#define ZAP_CRY_SHA224 5

#define ZAP_ROR32(x, n) (((x) >> (n)) | ((x) << (32 - (n))))
#define ZAP_ROL32(x, n) (((x) << (n)) | ((x) >> (32 - (n))))
#define ZAP_ROR64(x, n) (((x) >> (n)) | ((x) << (64 - (n))))

/* ---- SHA-256 (FIPS 180-4) ---- */
typedef struct {
    uint32_t h[8];
    uint64_t len; /* total message bytes */
    uint8_t buf[64];
    size_t n; /* bytes buffered */
} zap_sha256;

static const uint32_t ZAP_SHA256_K[64] = {
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1,
    0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
    0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
    0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2};

static void zap_sha256_block(zap_sha256 *c, const uint8_t *p) {
    uint32_t w[64], a, b, cc, d, e, f, g, h, t1, t2;
    for (int i = 0; i < 16; i++) {
        w[i] = ((uint32_t)p[i * 4] << 24) | ((uint32_t)p[i * 4 + 1] << 16) |
               ((uint32_t)p[i * 4 + 2] << 8) | (uint32_t)p[i * 4 + 3];
    }
    for (int i = 16; i < 64; i++) {
        uint32_t s0 = ZAP_ROR32(w[i - 15], 7) ^ ZAP_ROR32(w[i - 15], 18) ^
                      (w[i - 15] >> 3);
        uint32_t s1 =
            ZAP_ROR32(w[i - 2], 17) ^ ZAP_ROR32(w[i - 2], 19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    a = c->h[0];
    b = c->h[1];
    cc = c->h[2];
    d = c->h[3];
    e = c->h[4];
    f = c->h[5];
    g = c->h[6];
    h = c->h[7];
    for (int i = 0; i < 64; i++) {
        uint32_t S1 = ZAP_ROR32(e, 6) ^ ZAP_ROR32(e, 11) ^ ZAP_ROR32(e, 25);
        uint32_t ch = (e & f) ^ (~e & g);
        t1 = h + S1 + ch + ZAP_SHA256_K[i] + w[i];
        uint32_t S0 = ZAP_ROR32(a, 2) ^ ZAP_ROR32(a, 13) ^ ZAP_ROR32(a, 22);
        uint32_t maj = (a & b) ^ (a & cc) ^ (b & cc);
        t2 = S0 + maj;
        h = g;
        g = f;
        f = e;
        e = d + t1;
        d = cc;
        cc = b;
        b = a;
        a = t1 + t2;
    }
    c->h[0] += a;
    c->h[1] += b;
    c->h[2] += cc;
    c->h[3] += d;
    c->h[4] += e;
    c->h[5] += f;
    c->h[6] += g;
    c->h[7] += h;
}

static void zap_sha256_init(zap_sha256 *c) {
    c->h[0] = 0x6a09e667;
    c->h[1] = 0xbb67ae85;
    c->h[2] = 0x3c6ef372;
    c->h[3] = 0xa54ff53a;
    c->h[4] = 0x510e527f;
    c->h[5] = 0x9b05688c;
    c->h[6] = 0x1f83d9ab;
    c->h[7] = 0x5be0cd19;
    c->len = 0;
    c->n = 0;
}

static void zap_sha256_update(zap_sha256 *c, const uint8_t *d, size_t n) {
    c->len += n;
    for (size_t i = 0; i < n; i++) {
        c->buf[c->n++] = d[i];
        if (c->n == 64) {
            zap_sha256_block(c, c->buf);
            c->n = 0;
        }
    }
}

static void zap_sha256_final(zap_sha256 *c, uint8_t *out) {
    uint64_t bits = c->len * 8;
    size_t i = c->n;
    c->buf[i++] = 0x80;
    if (i > 56) {
        while (i < 64)
            c->buf[i++] = 0;
        zap_sha256_block(c, c->buf);
        i = 0;
    }
    while (i < 56)
        c->buf[i++] = 0;
    for (int j = 7; j >= 0; j--)
        c->buf[i++] = (uint8_t)(bits >> (j * 8));
    zap_sha256_block(c, c->buf);
    for (int k = 0; k < 8; k++) {
        out[k * 4] = (uint8_t)(c->h[k] >> 24);
        out[k * 4 + 1] = (uint8_t)(c->h[k] >> 16);
        out[k * 4 + 2] = (uint8_t)(c->h[k] >> 8);
        out[k * 4 + 3] = (uint8_t)c->h[k];
    }
}

/* ---- SHA-512 (FIPS 180-4) ---- */
typedef struct {
    uint64_t h[8];
    uint64_t len; /* total message bytes (< 2^61, so a 64-bit bit count fits) */
    uint8_t buf[128];
    size_t n;
} zap_sha512;

static const uint64_t ZAP_SHA512_K[80] = {
    0x428a2f98d728ae22ULL, 0x7137449123ef65cdULL, 0xb5c0fbcfec4d3b2fULL,
    0xe9b5dba58189dbbcULL, 0x3956c25bf348b538ULL, 0x59f111f1b605d019ULL,
    0x923f82a4af194f9bULL, 0xab1c5ed5da6d8118ULL, 0xd807aa98a3030242ULL,
    0x12835b0145706fbeULL, 0x243185be4ee4b28cULL, 0x550c7dc3d5ffb4e2ULL,
    0x72be5d74f27b896fULL, 0x80deb1fe3b1696b1ULL, 0x9bdc06a725c71235ULL,
    0xc19bf174cf692694ULL, 0xe49b69c19ef14ad2ULL, 0xefbe4786384f25e3ULL,
    0x0fc19dc68b8cd5b5ULL, 0x240ca1cc77ac9c65ULL, 0x2de92c6f592b0275ULL,
    0x4a7484aa6ea6e483ULL, 0x5cb0a9dcbd41fbd4ULL, 0x76f988da831153b5ULL,
    0x983e5152ee66dfabULL, 0xa831c66d2db43210ULL, 0xb00327c898fb213fULL,
    0xbf597fc7beef0ee4ULL, 0xc6e00bf33da88fc2ULL, 0xd5a79147930aa725ULL,
    0x06ca6351e003826fULL, 0x142929670a0e6e70ULL, 0x27b70a8546d22ffcULL,
    0x2e1b21385c26c926ULL, 0x4d2c6dfc5ac42aedULL, 0x53380d139d95b3dfULL,
    0x650a73548baf63deULL, 0x766a0abb3c77b2a8ULL, 0x81c2c92e47edaee6ULL,
    0x92722c851482353bULL, 0xa2bfe8a14cf10364ULL, 0xa81a664bbc423001ULL,
    0xc24b8b70d0f89791ULL, 0xc76c51a30654be30ULL, 0xd192e819d6ef5218ULL,
    0xd69906245565a910ULL, 0xf40e35855771202aULL, 0x106aa07032bbd1b8ULL,
    0x19a4c116b8d2d0c8ULL, 0x1e376c085141ab53ULL, 0x2748774cdf8eeb99ULL,
    0x34b0bcb5e19b48a8ULL, 0x391c0cb3c5c95a63ULL, 0x4ed8aa4ae3418acbULL,
    0x5b9cca4f7763e373ULL, 0x682e6ff3d6b2b8a3ULL, 0x748f82ee5defb2fcULL,
    0x78a5636f43172f60ULL, 0x84c87814a1f0ab72ULL, 0x8cc702081a6439ecULL,
    0x90befffa23631e28ULL, 0xa4506cebde82bde9ULL, 0xbef9a3f7b2c67915ULL,
    0xc67178f2e372532bULL, 0xca273eceea26619cULL, 0xd186b8c721c0c207ULL,
    0xeada7dd6cde0eb1eULL, 0xf57d4f7fee6ed178ULL, 0x06f067aa72176fbaULL,
    0x0a637dc5a2c898a6ULL, 0x113f9804bef90daeULL, 0x1b710b35131c471bULL,
    0x28db77f523047d84ULL, 0x32caab7b40c72493ULL, 0x3c9ebe0a15c9bebcULL,
    0x431d67c49c100d4cULL, 0x4cc5d4becb3e42b6ULL, 0x597f299cfc657e2aULL,
    0x5fcb6fab3ad6faecULL, 0x6c44198c4a475817ULL};

static void zap_sha512_block(zap_sha512 *c, const uint8_t *p) {
    uint64_t w[80], a, b, cc, d, e, f, g, h, t1, t2;
    for (int i = 0; i < 16; i++) {
        w[i] = 0;
        for (int j = 0; j < 8; j++)
            w[i] = (w[i] << 8) | (uint64_t)p[i * 8 + j];
    }
    for (int i = 16; i < 80; i++) {
        uint64_t s0 = ZAP_ROR64(w[i - 15], 1) ^ ZAP_ROR64(w[i - 15], 8) ^
                      (w[i - 15] >> 7);
        uint64_t s1 =
            ZAP_ROR64(w[i - 2], 19) ^ ZAP_ROR64(w[i - 2], 61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    a = c->h[0];
    b = c->h[1];
    cc = c->h[2];
    d = c->h[3];
    e = c->h[4];
    f = c->h[5];
    g = c->h[6];
    h = c->h[7];
    for (int i = 0; i < 80; i++) {
        uint64_t S1 = ZAP_ROR64(e, 14) ^ ZAP_ROR64(e, 18) ^ ZAP_ROR64(e, 41);
        uint64_t ch = (e & f) ^ (~e & g);
        t1 = h + S1 + ch + ZAP_SHA512_K[i] + w[i];
        uint64_t S0 = ZAP_ROR64(a, 28) ^ ZAP_ROR64(a, 34) ^ ZAP_ROR64(a, 39);
        uint64_t maj = (a & b) ^ (a & cc) ^ (b & cc);
        t2 = S0 + maj;
        h = g;
        g = f;
        f = e;
        e = d + t1;
        d = cc;
        cc = b;
        b = a;
        a = t1 + t2;
    }
    c->h[0] += a;
    c->h[1] += b;
    c->h[2] += cc;
    c->h[3] += d;
    c->h[4] += e;
    c->h[5] += f;
    c->h[6] += g;
    c->h[7] += h;
}

static void zap_sha512_init(zap_sha512 *c) {
    c->h[0] = 0x6a09e667f3bcc908ULL;
    c->h[1] = 0xbb67ae8584caa73bULL;
    c->h[2] = 0x3c6ef372fe94f82bULL;
    c->h[3] = 0xa54ff53a5f1d36f1ULL;
    c->h[4] = 0x510e527fade682d1ULL;
    c->h[5] = 0x9b05688c2b3e6c1fULL;
    c->h[6] = 0x1f83d9abfb41bd6bULL;
    c->h[7] = 0x5be0cd19137e2179ULL;
    c->len = 0;
    c->n = 0;
}

static void zap_sha512_update(zap_sha512 *c, const uint8_t *d, size_t n) {
    c->len += n;
    for (size_t i = 0; i < n; i++) {
        c->buf[c->n++] = d[i];
        if (c->n == 128) {
            zap_sha512_block(c, c->buf);
            c->n = 0;
        }
    }
}

static void zap_sha512_final(zap_sha512 *c, uint8_t *out) {
    uint64_t bits = c->len * 8;
    size_t i = c->n;
    c->buf[i++] = 0x80;
    if (i > 112) {
        while (i < 128)
            c->buf[i++] = 0;
        zap_sha512_block(c, c->buf);
        i = 0;
    }
    while (i < 120)
        c->buf[i++] = 0; /* high 64 bits of the 128-bit length are zero */
    for (int j = 7; j >= 0; j--)
        c->buf[i++] = (uint8_t)(bits >> (j * 8));
    zap_sha512_block(c, c->buf);
    for (int k = 0; k < 8; k++) {
        for (int j = 0; j < 8; j++)
            out[k * 8 + j] = (uint8_t)(c->h[k] >> ((7 - j) * 8));
    }
}

/* ---- SHA-1 (FIPS 180-1) ---- */
typedef struct {
    uint32_t h[5];
    uint64_t len;
    uint8_t buf[64];
    size_t n;
} zap_sha1;

static void zap_sha1_block(zap_sha1 *c, const uint8_t *p) {
    uint32_t w[80], a, b, cc, d, e, t;
    for (int i = 0; i < 16; i++) {
        w[i] = ((uint32_t)p[i * 4] << 24) | ((uint32_t)p[i * 4 + 1] << 16) |
               ((uint32_t)p[i * 4 + 2] << 8) | (uint32_t)p[i * 4 + 3];
    }
    for (int i = 16; i < 80; i++) {
        uint32_t v = w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16];
        w[i] = ZAP_ROL32(v, 1);
    }
    a = c->h[0];
    b = c->h[1];
    cc = c->h[2];
    d = c->h[3];
    e = c->h[4];
    for (int i = 0; i < 80; i++) {
        uint32_t f, k;
        if (i < 20) {
            f = (b & cc) | (~b & d);
            k = 0x5a827999;
        } else if (i < 40) {
            f = b ^ cc ^ d;
            k = 0x6ed9eba1;
        } else if (i < 60) {
            f = (b & cc) | (b & d) | (cc & d);
            k = 0x8f1bbcdc;
        } else {
            f = b ^ cc ^ d;
            k = 0xca62c1d6;
        }
        t = ZAP_ROL32(a, 5) + f + e + k + w[i];
        e = d;
        d = cc;
        cc = ZAP_ROL32(b, 30);
        b = a;
        a = t;
    }
    c->h[0] += a;
    c->h[1] += b;
    c->h[2] += cc;
    c->h[3] += d;
    c->h[4] += e;
}

static void zap_sha1_init(zap_sha1 *c) {
    c->h[0] = 0x67452301;
    c->h[1] = 0xefcdab89;
    c->h[2] = 0x98badcfe;
    c->h[3] = 0x10325476;
    c->h[4] = 0xc3d2e1f0;
    c->len = 0;
    c->n = 0;
}

static void zap_sha1_update(zap_sha1 *c, const uint8_t *d, size_t n) {
    c->len += n;
    for (size_t i = 0; i < n; i++) {
        c->buf[c->n++] = d[i];
        if (c->n == 64) {
            zap_sha1_block(c, c->buf);
            c->n = 0;
        }
    }
}

static void zap_sha1_final(zap_sha1 *c, uint8_t *out) {
    uint64_t bits = c->len * 8;
    size_t i = c->n;
    c->buf[i++] = 0x80;
    if (i > 56) {
        while (i < 64)
            c->buf[i++] = 0;
        zap_sha1_block(c, c->buf);
        i = 0;
    }
    while (i < 56)
        c->buf[i++] = 0;
    for (int j = 7; j >= 0; j--)
        c->buf[i++] = (uint8_t)(bits >> (j * 8));
    zap_sha1_block(c, c->buf);
    for (int k = 0; k < 5; k++) {
        out[k * 4] = (uint8_t)(c->h[k] >> 24);
        out[k * 4 + 1] = (uint8_t)(c->h[k] >> 16);
        out[k * 4 + 2] = (uint8_t)(c->h[k] >> 8);
        out[k * 4 + 3] = (uint8_t)c->h[k];
    }
}

/* ---- MD5 (RFC 1321) — little-endian ---- */
typedef struct {
    uint32_t h[4];
    uint64_t len;
    uint8_t buf[64];
    size_t n;
} zap_md5;

static void zap_md5_block(zap_md5 *ctx, const uint8_t *p) {
    static const uint32_t K[64] = {
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a,
        0xa8304613, 0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be,
        0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340,
        0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8,
        0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
        0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92,
        0xffeff47d, 0x85845dd1, 0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1,
        0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391};
    static const uint8_t S[64] = {
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5, 9,  14, 20, 5, 9,  14, 20, 5, 9,  14, 20, 5, 9,  14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21};
    uint32_t m[16];
    for (int i = 0; i < 16; i++) {
        m[i] = (uint32_t)p[i * 4] | ((uint32_t)p[i * 4 + 1] << 8) |
               ((uint32_t)p[i * 4 + 2] << 16) | ((uint32_t)p[i * 4 + 3] << 24);
    }
    uint32_t a = ctx->h[0], b = ctx->h[1], c = ctx->h[2], d = ctx->h[3];
    for (int i = 0; i < 64; i++) {
        uint32_t f, g;
        if (i < 16) {
            f = (b & c) | (~b & d);
            g = (uint32_t)i;
        } else if (i < 32) {
            f = (d & b) | (~d & c);
            g = (uint32_t)(5 * i + 1) & 15;
        } else if (i < 48) {
            f = b ^ c ^ d;
            g = (uint32_t)(3 * i + 5) & 15;
        } else {
            f = c ^ (b | ~d);
            g = (uint32_t)(7 * i) & 15;
        }
        uint32_t tmp = d;
        d = c;
        c = b;
        uint32_t x = a + f + K[i] + m[g];
        b = b + ZAP_ROL32(x, S[i]);
        a = tmp;
    }
    ctx->h[0] += a;
    ctx->h[1] += b;
    ctx->h[2] += c;
    ctx->h[3] += d;
}

static void zap_md5_init(zap_md5 *c) {
    c->h[0] = 0x67452301;
    c->h[1] = 0xefcdab89;
    c->h[2] = 0x98badcfe;
    c->h[3] = 0x10325476;
    c->len = 0;
    c->n = 0;
}

static void zap_md5_update(zap_md5 *c, const uint8_t *d, size_t n) {
    c->len += n;
    for (size_t i = 0; i < n; i++) {
        c->buf[c->n++] = d[i];
        if (c->n == 64) {
            zap_md5_block(c, c->buf);
            c->n = 0;
        }
    }
}

static void zap_md5_final(zap_md5 *c, uint8_t *out) {
    uint64_t bits = c->len * 8;
    size_t i = c->n;
    c->buf[i++] = 0x80;
    if (i > 56) {
        while (i < 64)
            c->buf[i++] = 0;
        zap_md5_block(c, c->buf);
        i = 0;
    }
    while (i < 56)
        c->buf[i++] = 0;
    for (int j = 0; j < 8; j++)
        c->buf[i++] = (uint8_t)(bits >> (j * 8)); /* little-endian length */
    zap_md5_block(c, c->buf);
    for (int k = 0; k < 4; k++) {
        out[k * 4] = (uint8_t)c->h[k];
        out[k * 4 + 1] = (uint8_t)(c->h[k] >> 8);
        out[k * 4 + 2] = (uint8_t)(c->h[k] >> 16);
        out[k * 4 + 3] = (uint8_t)(c->h[k] >> 24);
    }
}

/* ---- SHA-384 / SHA-224 initial hash values (FIPS 180-4) ----
 * SHA-384 runs the SHA-512 compression function from a different IV and emits
 * the leading 48 of the 64 output bytes; SHA-224 runs the SHA-256 compression
 * function from a different IV and emits the leading 28 of the 32 output bytes.
 * Only the IV and the output length change, so update/block/final are shared. */
static void zap_sha384_init(zap_sha512 *c) {
    c->h[0] = 0xcbbb9d5dc1059ed8ULL;
    c->h[1] = 0x629a292a367cd507ULL;
    c->h[2] = 0x9159015a3070dd17ULL;
    c->h[3] = 0x152fecd8f70e5939ULL;
    c->h[4] = 0x67332667ffc00b31ULL;
    c->h[5] = 0x8eb44a8768581511ULL;
    c->h[6] = 0xdb0c2e0d64f98fa7ULL;
    c->h[7] = 0x47b5481dbefa4fa4ULL;
    c->len = 0;
    c->n = 0;
}

static void zap_sha224_init(zap_sha256 *c) {
    c->h[0] = 0xc1059ed8;
    c->h[1] = 0x367cd507;
    c->h[2] = 0x3070dd17;
    c->h[3] = 0xf70e5939;
    c->h[4] = 0xffc00b31;
    c->h[5] = 0x68581511;
    c->h[6] = 0x64f98fa7;
    c->h[7] = 0xbefa4fa4;
    c->len = 0;
    c->n = 0;
}

/* ---- Unified, algorithm-tagged hash context ---- */
typedef struct {
    int32_t algo;
    union {
        zap_sha256 s256;
        zap_sha512 s512;
        zap_sha1 s1;
        zap_md5 m5;
    } u;
} zap_hash_ctx;

static size_t zap_crypto_digest_len(int algo) {
    switch (algo) {
    case ZAP_CRY_SHA256:
        return 32;
    case ZAP_CRY_SHA512:
        return 64;
    case ZAP_CRY_SHA1:
        return 20;
    case ZAP_CRY_MD5:
        return 16;
    case ZAP_CRY_SHA384:
        return 48;
    case ZAP_CRY_SHA224:
        return 28;
    default:
        return 0;
    }
}

static size_t zap_crypto_block_len(int algo) {
    return (algo == ZAP_CRY_SHA512 || algo == ZAP_CRY_SHA384) ? 128 : 64;
}

static void zap_hash_ctx_init(zap_hash_ctx *c, int algo) {
    c->algo = algo;
    switch (algo) {
    case ZAP_CRY_SHA256:
        zap_sha256_init(&c->u.s256);
        break;
    case ZAP_CRY_SHA512:
        zap_sha512_init(&c->u.s512);
        break;
    case ZAP_CRY_SHA1:
        zap_sha1_init(&c->u.s1);
        break;
    case ZAP_CRY_SHA384:
        /* SHA-512 machinery, SHA-384 IV. */
        zap_sha384_init(&c->u.s512);
        break;
    case ZAP_CRY_SHA224:
        /* SHA-256 machinery, SHA-224 IV. */
        zap_sha224_init(&c->u.s256);
        break;
    default:
        zap_md5_init(&c->u.m5);
        break;
    }
}

static void zap_hash_ctx_update(zap_hash_ctx *c, const uint8_t *d, size_t n) {
    switch (c->algo) {
    case ZAP_CRY_SHA256:
    case ZAP_CRY_SHA224:
        zap_sha256_update(&c->u.s256, d, n);
        break;
    case ZAP_CRY_SHA512:
    case ZAP_CRY_SHA384:
        zap_sha512_update(&c->u.s512, d, n);
        break;
    case ZAP_CRY_SHA1:
        zap_sha1_update(&c->u.s1, d, n);
        break;
    default:
        zap_md5_update(&c->u.m5, d, n);
        break;
    }
}

/* Finalize a COPY so the caller's context stays usable (non-destructive). The
 * SHA-384 / SHA-224 cases run the full SHA-512 / SHA-256 finalization into the
 * 64-byte scratch buffer; the caller takes only zap_crypto_digest_len() leading
 * bytes (48 / 28), which is exactly the FIPS 180-4 truncation. */
static void zap_hash_ctx_final(const zap_hash_ctx *c, uint8_t *out) {
    zap_hash_ctx t = *c;
    switch (t.algo) {
    case ZAP_CRY_SHA256:
    case ZAP_CRY_SHA224:
        zap_sha256_final(&t.u.s256, out);
        break;
    case ZAP_CRY_SHA512:
    case ZAP_CRY_SHA384:
        zap_sha512_final(&t.u.s512, out);
        break;
    case ZAP_CRY_SHA1:
        zap_sha1_final(&t.u.s1, out);
        break;
    default:
        zap_md5_final(&t.u.m5, out);
        break;
    }
}

/* Build a lowercase-hex ZapStr from `n` raw bytes. */
static void *zap_crypto_hex_str(const uint8_t *d, size_t n) {
    static const char HEX[] = "0123456789abcdef";
    int64_t outlen = (int64_t)(n * 2);
    ZapStr *s = zap_str_alloc(outlen);
    for (size_t i = 0; i < n; i++) {
        s->data[2 * i] = HEX[d[i] >> 4];
        s->data[2 * i + 1] = HEX[d[i] & 0x0f];
    }
    return s;
}

/* The hash context behind a streaming handle (#837: a `bytes` ZapStr whose inline
 * byte payload IS the pointer-free context). Re-read on each call, so a moving GC
 * relocation of the handle is observed (no allocation happens between calls to a
 * streaming op, so the raw pointer is stable within each call). */
static zap_hash_ctx *zap_crypto_handle_ctx(void *h) {
    return (zap_hash_ctx *)(void *)((ZapStr *)h)->data;
}

static void *zap_crypto_oneshot_hex(int algo, void *data) {
    ZapStr *d = (ZapStr *)data;
    zap_hash_ctx c;
    zap_hash_ctx_init(&c, algo);
    zap_hash_ctx_update(&c, (const uint8_t *)d->data, (size_t)d->len);
    uint8_t out[64];
    zap_hash_ctx_final(&c, out);
    return zap_crypto_hex_str(out, zap_crypto_digest_len(algo));
}

/* One-shot hex digests over a str/bytes value. */
void *zap_crypto_sha256(void *data) {
    return zap_crypto_oneshot_hex(ZAP_CRY_SHA256, data);
}
void *zap_crypto_sha512(void *data) {
    return zap_crypto_oneshot_hex(ZAP_CRY_SHA512, data);
}
void *zap_crypto_sha1(void *data) {
    return zap_crypto_oneshot_hex(ZAP_CRY_SHA1, data);
}
void *zap_crypto_md5(void *data) {
    return zap_crypto_oneshot_hex(ZAP_CRY_MD5, data);
}
void *zap_crypto_sha384(void *data) {
    return zap_crypto_oneshot_hex(ZAP_CRY_SHA384, data);
}
void *zap_crypto_sha224(void *data) {
    return zap_crypto_oneshot_hex(ZAP_CRY_SHA224, data);
}

/* Streaming: a `bytes` handle whose inline payload IS the pointer-free context
 * (#837 one-object layout; `->data` is 8-byte aligned, which suffices for the
 * uint64-based hash state). */
void *zap_crypto_new(int64_t algo) {
    ZapStr *h = zap_str_alloc((int64_t)sizeof(zap_hash_ctx));
    zap_hash_ctx_init((zap_hash_ctx *)(void *)h->data, (int)algo);
    return h;
}

void zap_crypto_update(void *handle, void *data) {
    zap_hash_ctx *c = zap_crypto_handle_ctx(handle);
    ZapStr *d = (ZapStr *)data;
    zap_hash_ctx_update(c, (const uint8_t *)d->data, (size_t)d->len);
}

void *zap_crypto_hexdigest(void *handle) {
    zap_hash_ctx *c = zap_crypto_handle_ctx(handle);
    uint8_t out[64];
    zap_hash_ctx_final(c, out);
    return zap_crypto_hex_str(out, zap_crypto_digest_len(c->algo));
}

void *zap_crypto_digest(void *handle) {
    zap_hash_ctx *c = zap_crypto_handle_ctx(handle);
    uint8_t out[64];
    size_t n = zap_crypto_digest_len(c->algo);
    zap_hash_ctx_final(c, out);
    return zap_str_new((const char *)out, (int64_t)n);
}

/* ---- HMAC (RFC 2104) ---- */
static void zap_crypto_hmac(int algo, const uint8_t *key, size_t keylen,
                            const uint8_t *msg, size_t msglen, uint8_t *out) {
    size_t blk = zap_crypto_block_len(algo);
    size_t dlen = zap_crypto_digest_len(algo);
    uint8_t k[128];
    memset(k, 0, sizeof(k));
    if (keylen > blk) {
        zap_hash_ctx c;
        zap_hash_ctx_init(&c, algo);
        zap_hash_ctx_update(&c, key, keylen);
        zap_hash_ctx_final(&c, k);
    } else {
        memcpy(k, key, keylen);
    }
    uint8_t ipad[128], opad[128];
    for (size_t i = 0; i < blk; i++) {
        ipad[i] = (uint8_t)(k[i] ^ 0x36);
        opad[i] = (uint8_t)(k[i] ^ 0x5c);
    }
    uint8_t inner[64];
    zap_hash_ctx c;
    zap_hash_ctx_init(&c, algo);
    zap_hash_ctx_update(&c, ipad, blk);
    zap_hash_ctx_update(&c, msg, msglen);
    zap_hash_ctx_final(&c, inner);
    zap_hash_ctx_init(&c, algo);
    zap_hash_ctx_update(&c, opad, blk);
    zap_hash_ctx_update(&c, inner, dlen);
    zap_hash_ctx_final(&c, out);
}

static void *zap_crypto_hmac_hex(int algo, void *key, void *msg) {
    ZapStr *k = (ZapStr *)key;
    ZapStr *m = (ZapStr *)msg;
    uint8_t out[64];
    zap_crypto_hmac(algo, (const uint8_t *)k->data, (size_t)k->len,
                    (const uint8_t *)m->data, (size_t)m->len, out);
    return zap_crypto_hex_str(out, zap_crypto_digest_len(algo));
}

void *zap_crypto_hmac_sha256(void *key, void *msg) {
    return zap_crypto_hmac_hex(ZAP_CRY_SHA256, key, msg);
}
void *zap_crypto_hmac_sha512(void *key, void *msg) {
    return zap_crypto_hmac_hex(ZAP_CRY_SHA512, key, msg);
}

/* Raw-bytes HMAC: the digest as a `bytes` value rather than hex. Needed to chain
 * HMAC-SHA256 (e.g. the AWS SigV4 signing-key derivation, ticket #153), where one
 * step's raw digest is the key for the next and a hex round-trip would be lossy. */
static void *zap_crypto_hmac_raw(int algo, void *key, void *msg) {
    ZapStr *k = (ZapStr *)key;
    ZapStr *m = (ZapStr *)msg;
    uint8_t out[64];
    zap_crypto_hmac(algo, (const uint8_t *)k->data, (size_t)k->len,
                    (const uint8_t *)m->data, (size_t)m->len, out);
    return zap_str_new((const char *)out, (int64_t)zap_crypto_digest_len(algo));
}

void *zap_crypto_hmac_sha256_raw(void *key, void *msg) {
    return zap_crypto_hmac_raw(ZAP_CRY_SHA256, key, msg);
}

/* ---- Secure random / secrets ---- */
static void zap_crypto_fill_random(uint8_t *buf, size_t n) {
#if defined(ZAP_PLATFORM_WINDOWS)
    /* CNG system CSPRNG: the Windows replacement for getentropy/_urandom. With the
     * BCRYPT_USE_SYSTEM_PREFERRED_RNG flag no algorithm handle is needed. The call
     * fills the whole buffer in one go (no 256-byte per-call limit). */
    if (BCryptGenRandom(NULL, buf, (ULONG)n, BCRYPT_USE_SYSTEM_PREFERRED_RNG) != 0) {
        fprintf(stderr, "zap: crypto: no CSPRNG available (BCryptGenRandom)\n");
        abort();
    }
    return;
#else
    size_t off = 0;
    while (off < n) {
        size_t chunk = n - off;
        if (chunk > 256)
            chunk = 256; /* getentropy's per-call limit */
        if (getentropy(buf + off, chunk) == 0) {
            off += chunk;
            continue;
        }
        /* Fallback: read the kernel CSPRNG directly. */
        int fd = open("/dev/urandom", O_RDONLY);
        if (fd < 0) {
            fprintf(stderr, "zap: crypto: no CSPRNG available\n");
            abort();
        }
        while (off < n) {
            ssize_t r = read(fd, buf + off, n - off);
            if (r <= 0) {
                fprintf(stderr, "zap: crypto: CSPRNG read failed\n");
                abort();
            }
            off += (size_t)r;
        }
        close(fd);
        return;
    }
#endif /* ZAP_PLATFORM_WINDOWS */
}

void *zap_crypto_random_bytes(int64_t n) {
    if (n < 0)
        n = 0;
    ZapStr *s = zap_str_alloc(n);
    if (n > 0)
        zap_crypto_fill_random((uint8_t *)s->data, (size_t)n);
    return s;
}

void *zap_crypto_token_hex(int64_t n) {
    if (n < 0)
        n = 0;
    uint8_t *rnd = (uint8_t *)zap_buf(n > 0 ? n : 1);
    if (n > 0)
        zap_crypto_fill_random(rnd, (size_t)n);
    return zap_crypto_hex_str(rnd, (size_t)n);
}

/* crypto.token_urlsafe(nbytes) -> str (ticket #764): `nbytes` bytes from the OS
 * CSPRNG, base64url-encoded with NO padding (mirrors Python secrets.token_urlsafe).
 * A negative `nbytes` raises a catchable ValueError; `nbytes == 0` yields "".
 * The URL-safe base64 is encoded inline (the general base64url helper #771 is a
 * separate nice-to-have) over the `-_` alphabet; output length is the unpadded
 * ceil(nbytes*4/3). The random bytes live in a transient stack buffer that never
 * escapes; only the encoded string is a GC object, built last. */
void *zap_crypto_token_urlsafe(int64_t nbytes) {
    static const char B64URL[64] =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    if (nbytes < 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("crypto.token_urlsafe: nbytes must be non-negative",
                                    sizeof("crypto.token_urlsafe: nbytes must be non-negative") - 1));
    }
    size_t n = (size_t)nbytes;
    uint8_t *rnd = (uint8_t *)malloc(n ? n : 1);
    if (!rnd) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("crypto.token_urlsafe: out of memory", 36));
    }
    if (n)
        zap_crypto_fill_random(rnd, n);
    /* Unpadded base64url length: the ceil of n*4/3 (full groups of 3 -> 4 chars,
     * a 1-byte tail -> 2 chars, a 2-byte tail -> 3 chars). */
    int64_t outlen = (int64_t)((n / 3) * 4 + ((n % 3) ? (n % 3) + 1 : 0));
    char *out = (char *)zap_buf(outlen > 0 ? outlen : 1);
    int64_t o = 0;
    size_t i = 0;
    while (i + 3 <= n) {
        uint32_t v = ((uint32_t)rnd[i] << 16) | ((uint32_t)rnd[i + 1] << 8) |
                     (uint32_t)rnd[i + 2];
        out[o++] = B64URL[(v >> 18) & 0x3F];
        out[o++] = B64URL[(v >> 12) & 0x3F];
        out[o++] = B64URL[(v >> 6) & 0x3F];
        out[o++] = B64URL[v & 0x3F];
        i += 3;
    }
    size_t rem = n - i;
    if (rem == 1) {
        uint32_t v = (uint32_t)rnd[i] << 16;
        out[o++] = B64URL[(v >> 18) & 0x3F];
        out[o++] = B64URL[(v >> 12) & 0x3F];
    } else if (rem == 2) {
        uint32_t v = ((uint32_t)rnd[i] << 16) | ((uint32_t)rnd[i + 1] << 8);
        out[o++] = B64URL[(v >> 18) & 0x3F];
        out[o++] = B64URL[(v >> 12) & 0x3F];
        out[o++] = B64URL[(v >> 6) & 0x3F];
    }
    free(rnd);
    return zap_str_adopt(out, o);
}

/* ---- Streaming HMAC (ticket #766) ----
 * Incremental HMAC over the self-contained hash core, mirroring the streaming
 * hash handle (`zap_crypto_new`): the handle is a `bytes` value whose buffer is
 * a pointer-free `zap_hmac_ctx`, so the precise collector traces and frees it.
 * The inner hash context is kept fed with `ipad || data-so-far`; finalize copies
 * it (non-destructive) and applies the outer `opad` pass. Supports the SHA family
 * the hash core implements (sha1/sha224/sha256/sha384/sha512); an unknown name
 * raises a catchable ValueError (md5 is excluded, as for the KDFs). */
typedef struct {
    int32_t algo;
    size_t blk;            /* block length for this algo */
    zap_hash_ctx inner;    /* initialized with ipad, then fed input */
    uint8_t opad[128];     /* key ^ 0x5c, block-padded */
} zap_hmac_ctx;

/* Map a hash name to a self-contained hash-core algo tag, or -1 if unsupported. */
static int zap_crypto_hash_algo_by_name(const ZapStr *name) {
    size_t n = (size_t)name->len;
    const char *s = name->data;
    if (n == 6 && memcmp(s, "sha256", 6) == 0)
        return ZAP_CRY_SHA256;
    if (n == 6 && memcmp(s, "sha512", 6) == 0)
        return ZAP_CRY_SHA512;
    if (n == 6 && memcmp(s, "sha384", 6) == 0)
        return ZAP_CRY_SHA384;
    if (n == 6 && memcmp(s, "sha224", 6) == 0)
        return ZAP_CRY_SHA224;
    if (n == 4 && memcmp(s, "sha1", 4) == 0)
        return ZAP_CRY_SHA1;
    return -1;
}

static zap_hmac_ctx *zap_crypto_hmac_handle_ctx(void *h) {
    return (zap_hmac_ctx *)(void *)((ZapStr *)h)->data;
}

void *zap_crypto_hmac_new(void *hash_name, void *key) {
    int algo = zap_crypto_hash_algo_by_name((const ZapStr *)hash_name);
    if (algo < 0) {
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_str_new("crypto.hmac_new: unsupported hash_name (use "
                        "sha1/sha224/sha256/sha384/sha512)",
                        sizeof("crypto.hmac_new: unsupported hash_name (use "
                               "sha1/sha224/sha256/sha384/sha512)") -
                            1));
    }
    const ZapStr *k = (const ZapStr *)key;
    /* One-object handle (#837): the pointer-free HMAC ctx lives inline in the
     * `bytes` payload. No allocation happens below, so `k` stays put. */
    ZapStr *h = zap_str_alloc((int64_t)sizeof(zap_hmac_ctx));
    zap_hmac_ctx *c = (zap_hmac_ctx *)(void *)h->data;
    c->algo = algo;
    size_t blk = zap_crypto_block_len(algo);
    c->blk = blk;
    /* Derive the block-sized key K0 (RFC 2104): hash it down if longer. */
    uint8_t k0[128];
    memset(k0, 0, sizeof(k0));
    if ((size_t)k->len > blk) {
        zap_hash_ctx hc;
        zap_hash_ctx_init(&hc, algo);
        zap_hash_ctx_update(&hc, (const uint8_t *)k->data, (size_t)k->len);
        zap_hash_ctx_final(&hc, k0);
    } else {
        memcpy(k0, k->data, (size_t)k->len);
    }
    uint8_t ipad[128];
    for (size_t i = 0; i < blk; i++) {
        ipad[i] = (uint8_t)(k0[i] ^ 0x36);
        c->opad[i] = (uint8_t)(k0[i] ^ 0x5c);
    }
    zap_hash_ctx_init(&c->inner, algo);
    zap_hash_ctx_update(&c->inner, ipad, blk);
    return h;
}

void zap_crypto_hmac_update(void *handle, void *data) {
    zap_hmac_ctx *c = zap_crypto_hmac_handle_ctx(handle);
    const ZapStr *d = (const ZapStr *)data;
    zap_hash_ctx_update(&c->inner, (const uint8_t *)d->data, (size_t)d->len);
}

/* Finalize a COPY of the inner context, then run the outer opad pass, so the
 * handle stays usable (non-destructive, matching the hash streaming handle). */
static void zap_crypto_hmac_finalize(const zap_hmac_ctx *c, uint8_t *out) {
    size_t dlen = zap_crypto_digest_len(c->algo);
    uint8_t inner[64];
    zap_hash_ctx_final(&c->inner, inner);
    zap_hash_ctx oc;
    zap_hash_ctx_init(&oc, c->algo);
    zap_hash_ctx_update(&oc, c->opad, c->blk);
    zap_hash_ctx_update(&oc, inner, dlen);
    zap_hash_ctx_final(&oc, out);
}

void *zap_crypto_hmac_hexdigest(void *handle) {
    zap_hmac_ctx *c = zap_crypto_hmac_handle_ctx(handle);
    uint8_t out[64];
    zap_crypto_hmac_finalize(c, out);
    return zap_crypto_hex_str(out, zap_crypto_digest_len(c->algo));
}

void *zap_crypto_hmac_digest(void *handle) {
    zap_hmac_ctx *c = zap_crypto_hmac_handle_ctx(handle);
    uint8_t out[64];
    size_t n = zap_crypto_digest_len(c->algo);
    zap_crypto_hmac_finalize(c, out);
    return zap_str_new((const char *)out, (int64_t)n);
}

/* crypto.randbelow(n) -> int (ticket #767): a uniformly random integer in
 * [0, n), drawn from the OS CSPRNG with rejection sampling to avoid modulo
 * bias. `n <= 0` raises a catchable ValueError (mirrors Python
 * secrets.randbelow, which rejects non-positive n). */
int64_t zap_crypto_randbelow(int64_t n) {
    if (n <= 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("crypto.randbelow: n must be positive",
                                    sizeof("crypto.randbelow: n must be positive") - 1));
    }
    uint64_t un = (uint64_t)n;
    if (un == 1) {
        return 0;
    }
    /* Largest multiple of `un` that fits in a uint64; reject draws at or above
     * it so the kept range is an exact whole number of `un`-sized buckets. */
    uint64_t limit = (UINT64_MAX / un) * un;
    for (;;) {
        uint64_t r = 0;
        zap_crypto_fill_random((uint8_t *)&r, sizeof(r));
        if (r < limit) {
            return (int64_t)(r % un);
        }
    }
}

/* A uniformly random valid index into a non-empty list, drawn from the OS
 * CSPRNG (backs crypto.choice, ticket #767). Reuses zap_crypto_randbelow's
 * unbiased rejection sampling. Unlike random.choice's zap_random_list_index,
 * which aborts on an empty sequence, this raises a catchable ValueError so a
 * caller can guard an empty list with try/except (mirrors Python
 * secrets.choice raising IndexError on an empty sequence). */
int64_t zap_crypto_list_index(void *l) {
    ZapList *list = (ZapList *)l;
    if (list->len <= 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("crypto.choice from an empty sequence",
                                    sizeof("crypto.choice from an empty sequence") - 1));
    }
    /* list->len > 0, so randbelow never raises here; it only draws the index. */
    return zap_crypto_randbelow(list->len);
}

