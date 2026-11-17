/* ===========================================================================
 * zlib / gzip — DEFLATE compression (ticket #106, D-0056/D-0060). Thin wrappers
 * over the system zlib (`#include <zlib.h>`, linked `-lz`):
 *   - `zlib.compress`/`decompress` use the zlib wrapper (RFC 1950), whose
 *     integrity check is an **Adler-32** trailer; windowBits = MAX_WBITS (15).
 *   - `gzip.compress`/`decompress` use the gzip wrapper (RFC 1952), whose check
 *     is an **ISO CRC32** trailer (polynomial 0x04C11DB7 — NOT the SSE4.2
 *     CRC32C/Castagnoli); windowBits = MAX_WBITS + 16 (31).
 *   - `zlib.crc32`/`adler32` expose zlib's running checksums as unsigned 32-bit
 *     values widened to i64.
 * Both `str` and `bytes` are the same binary-safe `ZapStr`; results are `bytes`.
 * Compression is single input buffer, output grown in chunks into a malloc
 * scratch then copied into a GC buffer; decompression streams until the wrapper
 * trailer closes the stream. Malformed input raises a Zap exception via
 * `zap_panic` rather than aborting. Streaming compression (incremental
 * feed/flush) is deferred (noted in the PR). Inputs are capped at ~4 GiB (zlib's
 * 32-bit `avail_in`); larger payloads are out of v1 scope.
 * ======================================================================== */

#if !defined(ZAP_NO_ZLIB)
/* One inflate/deflate output step works on at most this many bytes, so zlib's
 * 32-bit `avail_out` never overflows regardless of total output size. */
#define ZAP_Z_CHUNK ((uInt)(1u << 16))

/* Raise a Zap exception carrying `msg` (never returns). */
static void *zap_z_fault(const char *msg) {
    zap_panic(zap_str_new(msg, (int64_t)strlen(msg)));
    return NULL; /* unreachable: zap_panic does not return */
}

/* Copy `len` scratch bytes into a fresh GC `bytes` object (a `ZapStr` whose
 * atomic data buffer is traced through the string descriptor). */
static void *zap_z_finish_bytes(const unsigned char *buf, size_t len) {
    ZapStr *s = zap_str_alloc((int64_t)len);
    if (len > 0) {
        memcpy(s->data, buf, len);
    }
    return s;
}

/* DEFLATE-compress `in` at `level` (0..9, or Z_DEFAULT_COMPRESSION = -1) with
 * the given wrapper (`windowBits`: 15 = zlib, 31 = gzip). */
static void *zap_z_compress(const ZapStr *in, int level, int windowBits) {
    z_stream strm;
    memset(&strm, 0, sizeof strm);
    if (deflateInit2(&strm, level, Z_DEFLATED, windowBits, 8 /* memLevel */,
                     Z_DEFAULT_STRATEGY) != Z_OK) {
        return zap_z_fault("zlib: compressor init failed (bad level?)");
    }
    strm.next_in = (Bytef *)(uintptr_t)in->data;
    strm.avail_in = (uInt)in->len;
    size_t cap = (size_t)deflateBound(&strm, (uLong)in->len) + 16;
    unsigned char *scratch = (unsigned char *)malloc(cap);
    if (!scratch) {
        deflateEnd(&strm);
        return zap_z_fault("zlib: out of memory");
    }
    size_t total = 0;
    int rc;
    do {
        if (total == cap) {
            cap += ZAP_Z_CHUNK;
            unsigned char *grown = (unsigned char *)realloc(scratch, cap);
            if (!grown) {
                free(scratch);
                deflateEnd(&strm);
                return zap_z_fault("zlib: out of memory");
            }
            scratch = grown;
        }
        size_t room = cap - total;
        strm.next_out = scratch + total;
        strm.avail_out = room > ZAP_Z_CHUNK ? ZAP_Z_CHUNK : (uInt)room;
        uInt before = strm.avail_out;
        rc = deflate(&strm, Z_FINISH);
        total += (size_t)(before - strm.avail_out);
    } while (rc == Z_OK);
    deflateEnd(&strm);
    if (rc != Z_STREAM_END) {
        free(scratch);
        return zap_z_fault("zlib: compression failed");
    }
    void *out = zap_z_finish_bytes(scratch, total);
    free(scratch);
    return out;
}

/* Inflate `in` with the given wrapper (`windowBits`: 15 = zlib, 31 = gzip). */
static void *zap_z_decompress(const ZapStr *in, int windowBits) {
    z_stream strm;
    memset(&strm, 0, sizeof strm);
    if (inflateInit2(&strm, windowBits) != Z_OK) {
        return zap_z_fault("zlib: decompressor init failed");
    }
    strm.next_in = (Bytef *)(uintptr_t)in->data;
    strm.avail_in = (uInt)in->len;
    size_t cap = in->len > 0 ? (size_t)in->len * 2 + 64 : 64;
    unsigned char *scratch = (unsigned char *)malloc(cap);
    if (!scratch) {
        inflateEnd(&strm);
        return zap_z_fault("zlib: out of memory");
    }
    size_t total = 0;
    int rc;
    for (;;) {
        if (total == cap) {
            cap = cap * 2 + 64;
            unsigned char *grown = (unsigned char *)realloc(scratch, cap);
            if (!grown) {
                free(scratch);
                inflateEnd(&strm);
                return zap_z_fault("zlib: out of memory");
            }
            scratch = grown;
        }
        size_t room = cap - total;
        strm.next_out = scratch + total;
        strm.avail_out = room > ZAP_Z_CHUNK ? ZAP_Z_CHUNK : (uInt)room;
        uInt before = strm.avail_out;
        rc = inflate(&strm, Z_NO_FLUSH);
        total += (size_t)(before - strm.avail_out);
        if (rc == Z_STREAM_END) {
            break;
        }
        if (rc == Z_OK) {
            continue; /* more to do; grow output on the next pass if needed */
        }
        if (rc == Z_BUF_ERROR) {
            /* No progress possible. With output room always available, this
             * means the compressed data ended mid-stream (truncated input). */
            free(scratch);
            inflateEnd(&strm);
            return zap_z_fault("zlib: truncated compressed data");
        }
        /* Z_DATA_ERROR / Z_NEED_DICT / Z_MEM_ERROR / Z_STREAM_ERROR. */
        free(scratch);
        inflateEnd(&strm);
        return zap_z_fault("zlib: invalid compressed data");
    }
    inflateEnd(&strm);
    void *out = zap_z_finish_bytes(scratch, total);
    free(scratch);
    return out;
}

/* --- zlib wrapper (RFC 1950, Adler-32) --- */
void *zap_zlib_compress(void *data, int64_t level) {
    return zap_z_compress((const ZapStr *)data, (int)level, MAX_WBITS);
}
void *zap_zlib_decompress(void *data) {
    return zap_z_decompress((const ZapStr *)data, MAX_WBITS);
}

/* --- gzip wrapper (RFC 1952, ISO CRC32, poly 0x04C11DB7) --- */
void *zap_gzip_compress(void *data, int64_t level) {
    return zap_z_compress((const ZapStr *)data, (int)level, MAX_WBITS + 16);
}
void *zap_gzip_decompress(void *data) {
    return zap_z_decompress((const ZapStr *)data, MAX_WBITS + 16);
}

/* --- checksums (zlib's, exposed as unsigned 32-bit widened to i64) --- */
int64_t zap_zlib_crc32(void *data) {
    const ZapStr *s = (const ZapStr *)data;
    uLong c = crc32(0L, Z_NULL, 0);
    c = crc32(c, (const Bytef *)s->data, (uInt)s->len);
    return (int64_t)(uint32_t)c;
}
int64_t zap_zlib_adler32(void *data) {
    const ZapStr *s = (const ZapStr *)data;
    uLong a = adler32(0L, Z_NULL, 0);
    a = adler32(a, (const Bytef *)s->data, (uInt)s->len);
    return (int64_t)(uint32_t)a;
}
#else /* ZAP_NO_ZLIB — zlib/gzip backend gated off (#853): the program's import
       * graph never reaches a `zap_zlib_*`/`zap_gzip_*` seam, so libz is not
       * linked and <zlib.h> is not included. The entry points still lower and
       * link, but each raises a catchable Zap exception if somehow reached —
       * the same shape as the portable CPython stubs (D-0153). */
static void *zap_z_unavailable(void) {
    const char *m = "zlib/gzip compression is unavailable in this build "
                    "(the program's import graph reaches no compression use; see #853)";
    zap_panic(zap_str_new(m, (int64_t)strlen(m)));
    return NULL; /* unreachable: zap_panic does not return */
}
void *zap_zlib_compress(void *data, int64_t level) {
    (void)data;
    (void)level;
    return zap_z_unavailable();
}
void *zap_zlib_decompress(void *data) {
    (void)data;
    return zap_z_unavailable();
}
void *zap_gzip_compress(void *data, int64_t level) {
    (void)data;
    (void)level;
    return zap_z_unavailable();
}
void *zap_gzip_decompress(void *data) {
    (void)data;
    return zap_z_unavailable();
}
int64_t zap_zlib_crc32(void *data) {
    (void)data;
    zap_z_unavailable();
    return 0;
}
int64_t zap_zlib_adler32(void *data) {
    (void)data;
    zap_z_unavailable();
    return 0;
}
#endif /* ZAP_NO_ZLIB */

