/* zap runtime fragment: crypto_jose.inc.c -- one translation unit: #include'd by zaprt.c at a
 * fixed position (order is semantic). NOT a standalone TU; it relies on declarations
 * made earlier in zaprt.c (and may leave a preprocessor #if open for a later
 * fragment). Registered in crates/codegen/src/runtime_manifest.rs; see
 * crates/codegen/FOLDER_ARCHITECTURE.md for the add-a-fragment checklist.
 */
#if !defined(ZAP_NO_OPENSSL)
#include <openssl/bn.h>
#include <openssl/crypto.h> /* CRYPTO_memcmp: constant-time compare (#729)     */
#include <openssl/ec.h>
#include <openssl/ecdsa.h>
#include <openssl/err.h>
#include <openssl/evp.h>
#include <openssl/kdf.h> /* PBKDF2 / HKDF key derivation (#762, #763)           */
#include <openssl/obj_mac.h>
#include <openssl/objects.h>
#include <openssl/pem.h>
#include <openssl/rsa.h>
#include <openssl/ssl.h>
#include <openssl/x509v3.h>

/* ==========================================================================
 * crypto: constant-time byte comparison (ticket #729, D-0131)
 * --------------------------------------------------------------------------
 * `crypto.constant_time_eq(a, b)` returns 1 iff the two byte sequences are
 * equal, comparing them in time independent of their byte *contents*, so a
 * verifier cannot learn through timing how many leading bytes matched (the
 * classic MAC-comparison side channel the downstream JWT library needs, #729).
 *
 * Backed by OpenSSL's `CRYPTO_memcmp`, which is written to compare every byte
 * with no early-out and no data-dependent branch (unlike libc `memcmp`, which
 * the compiler/CPU may short-circuit). OpenSSL is already linked (`-lssl
 * -lcrypto`) for `tls`/RSA, so this needs no new dependency and lives here with
 * the other OpenSSL-backed crypto entries rather than in the self-contained
 * reference-hash section above.
 *
 * Guarantee (see D-0131): for EQUAL-LENGTH inputs the running time is
 * independent of the byte contents. A LENGTH DIFFERENCE is reported as a
 * mismatch (return 0) and the length itself is observable — in the HMAC use
 * case the digest length is a public constant, so this leaks nothing secret.
 * Crucially there is NO branch on byte *contents*: the only early return is on
 * the (non-secret) length. `str` and `bytes` share the `ZapStr*` ABI; empty
 * inputs are valid (two empty inputs are equal). No GC safepoint runs across
 * the call and no allocation is performed. */
int64_t zap_crypto_constant_time_eq(void *a, void *b) {
    const ZapStr *x = (const ZapStr *)a;
    const ZapStr *y = (const ZapStr *)b;
    if (x->len != y->len) {
        /* Length is not secret (it is public in the MAC-comparison use case);
         * this is the only early-out and it never inspects byte contents. */
        return 0;
    }
    /* CRYPTO_memcmp compares all `len` bytes with data-independent timing and
     * returns 0 iff they are equal. With len == 0 it inspects no bytes and
     * returns 0, so two empty inputs compare equal. */
    return CRYPTO_memcmp(x->data, y->data, (size_t)x->len) == 0 ? 1 : 0;
}

/* ==========================================================================
 * crypto: key derivation functions — PBKDF2-HMAC (#762) and HKDF (#763, D-0132)
 * --------------------------------------------------------------------------
 * Both ride OpenSSL (already linked). PBKDF2 (PKCS5_PBKDF2_HMAC) is the only
 * sound password-storage primitive in the stdlib; HKDF (RFC 5869, via the
 * EVP_PKEY_HKDF derive interface) turns one high-entropy secret into one or
 * more sub-keys. Both accept the SHA family by name — "sha1", "sha224",
 * "sha256", "sha384", "sha512" — matching the hashes the module already
 * exposes; an unknown name raises a catchable ValueError. `password`/`salt`/
 * `ikm`/`info` are str|bytes (ZapStr ABI); the derived key is a fresh `bytes`
 * value built last, after the OpenSSL objects are freed. No GC safepoint runs
 * across the derive calls. */

/* Map a Zap hash name to an OpenSSL digest, or NULL if unsupported. Deliberately
 * excludes md5 (never appropriate for a KDF). */
static const EVP_MD *zap_crypto_kdf_md(const ZapStr *name) {
    size_t n = (size_t)name->len;
    const char *s = name->data;
    if (n == 6 && memcmp(s, "sha256", 6) == 0)
        return EVP_sha256();
    if (n == 6 && memcmp(s, "sha512", 6) == 0)
        return EVP_sha512();
    if (n == 6 && memcmp(s, "sha384", 6) == 0)
        return EVP_sha384();
    if (n == 6 && memcmp(s, "sha224", 6) == 0)
        return EVP_sha224();
    if (n == 4 && memcmp(s, "sha1", 4) == 0)
        return EVP_sha1();
    return NULL;
}

__attribute__((noreturn)) static void zap_crypto_kdf_raise(const char *msg) {
    zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new(msg, (int64_t)strlen(msg)));
    abort(); /* unreachable: zap_panic_typed unwinds */
}

/* crypto.pbkdf2_hmac(hash_name, password, salt, iterations, dklen) -> bytes.
 * Mirrors Python hashlib.pbkdf2_hmac. `dklen == 0` means the backing hash's
 * native digest length. `iterations <= 0`, `dklen < 0`, or an unknown hash name
 * raise ValueError. */
void *zap_crypto_pbkdf2_hmac(void *hash_name, void *password, void *salt,
                             int64_t iterations, int64_t dklen) {
    const EVP_MD *md = zap_crypto_kdf_md((const ZapStr *)hash_name);
    if (!md) {
        zap_crypto_kdf_raise(
            "crypto.pbkdf2_hmac: unsupported hash_name (use sha1/sha224/sha256/sha384/sha512)");
    }
    if (iterations <= 0) {
        zap_crypto_kdf_raise("crypto.pbkdf2_hmac: iterations must be positive");
    }
    if (dklen < 0) {
        zap_crypto_kdf_raise("crypto.pbkdf2_hmac: dklen must be non-negative");
    }
    size_t keylen = (size_t)dklen;
    if (keylen == 0) {
        int dl = EVP_MD_get_size(md);
        keylen = dl > 0 ? (size_t)dl : 0;
    }
    const ZapStr *p = (const ZapStr *)password;
    const ZapStr *s = (const ZapStr *)salt;
    uint8_t *out = (uint8_t *)malloc(keylen ? keylen : 1);
    if (!out) {
        zap_crypto_kdf_raise("crypto.pbkdf2_hmac: out of memory");
    }
    int ok = PKCS5_PBKDF2_HMAC(p->data, (int)p->len,
                               (const unsigned char *)s->data, (int)s->len,
                               (int)iterations, md, (int)keylen, out) == 1;
    if (!ok) {
        free(out);
        zap_crypto_kdf_raise("crypto.pbkdf2_hmac: derivation failed");
    }
    void *result = zap_str_new((const char *)out, (int64_t)keylen);
    free(out);
    return result;
}

/* crypto.hkdf(hash_name, ikm, length, salt, info) -> bytes. RFC 5869
 * extract-and-expand. `length` must be in 1..=255*HashLen (else ValueError). An
 * empty `salt` is replaced with the RFC-mandated HashLen zero string explicitly,
 * so the result is identical across OpenSSL versions regardless of their
 * no-salt default. An unknown hash name raises ValueError. */
void *zap_crypto_hkdf(void *hash_name, void *ikm, int64_t length, void *salt,
                      void *info) {
    const EVP_MD *md = zap_crypto_kdf_md((const ZapStr *)hash_name);
    if (!md) {
        zap_crypto_kdf_raise(
            "crypto.hkdf: unsupported hash_name (use sha1/sha224/sha256/sha384/sha512)");
    }
    int dl = EVP_MD_get_size(md);
    size_t hashlen = dl > 0 ? (size_t)dl : 0;
    if (length <= 0) {
        zap_crypto_kdf_raise("crypto.hkdf: length must be positive");
    }
    if (hashlen == 0 || (size_t)length > 255u * hashlen) {
        zap_crypto_kdf_raise("crypto.hkdf: length too large (max 255*HashLen)");
    }
    const ZapStr *k = (const ZapStr *)ikm;
    const ZapStr *s = (const ZapStr *)salt;
    const ZapStr *inf = (const ZapStr *)info;

    /* RFC 5869 §2.2: an unset/empty salt is HashLen zero bytes. */
    uint8_t zero_salt[64];
    const unsigned char *salt_ptr = (const unsigned char *)s->data;
    size_t salt_len = (size_t)s->len;
    if (salt_len == 0) {
        memset(zero_salt, 0, hashlen);
        salt_ptr = zero_salt;
        salt_len = hashlen;
    }

    size_t outlen = (size_t)length;
    uint8_t *out = (uint8_t *)malloc(outlen ? outlen : 1);
    if (!out) {
        zap_crypto_kdf_raise("crypto.hkdf: out of memory");
    }
    EVP_PKEY_CTX *pctx = EVP_PKEY_CTX_new_id(EVP_PKEY_HKDF, NULL);
    int ok = pctx && EVP_PKEY_derive_init(pctx) == 1 &&
             EVP_PKEY_CTX_set_hkdf_md(pctx, md) == 1 &&
             EVP_PKEY_CTX_set1_hkdf_salt(pctx, salt_ptr, (int)salt_len) == 1 &&
             EVP_PKEY_CTX_set1_hkdf_key(pctx, (const unsigned char *)k->data,
                                        (int)k->len) == 1 &&
             EVP_PKEY_CTX_add1_hkdf_info(pctx, (const unsigned char *)inf->data,
                                         (int)inf->len) == 1 &&
             EVP_PKEY_derive(pctx, out, &outlen) == 1 && outlen == (size_t)length;
    if (pctx) {
        EVP_PKEY_CTX_free(pctx);
    }
    if (!ok) {
        free(out);
        zap_crypto_kdf_raise("crypto.hkdf: derivation failed");
    }
    void *result = zap_str_new((const char *)out, (int64_t)length);
    free(out);
    return result;
}

/* crypto.scrypt(password, salt, n, r, p, dklen) -> bytes (ticket #775, D-0134).
 * OpenSSL EVP_PBE_scrypt, mirroring Python hashlib.scrypt. `n` must be a power
 * of two greater than 1; `r`, `p`, and `dklen` must be positive. maxmem is left
 * at OpenSSL's default (~32 MiB), which covers the common parameter ranges; a
 * bad parameter, an over-budget cost, or a derivation failure raises a catchable
 * ValueError. */
void *zap_crypto_scrypt(void *password, void *salt, int64_t n, int64_t r,
                        int64_t p, int64_t dklen) {
    if (n <= 1 || (n & (n - 1)) != 0) {
        zap_crypto_kdf_raise("crypto.scrypt: n must be a power of two greater than 1");
    }
    if (r <= 0) {
        zap_crypto_kdf_raise("crypto.scrypt: r must be positive");
    }
    if (p <= 0) {
        zap_crypto_kdf_raise("crypto.scrypt: p must be positive");
    }
    if (dklen <= 0) {
        zap_crypto_kdf_raise("crypto.scrypt: dklen must be positive");
    }
    const ZapStr *pw = (const ZapStr *)password;
    const ZapStr *s = (const ZapStr *)salt;
    size_t keylen = (size_t)dklen;
    uint8_t *out = (uint8_t *)malloc(keylen);
    if (!out) {
        zap_crypto_kdf_raise("crypto.scrypt: out of memory");
    }
    int ok = EVP_PBE_scrypt(pw->data, (size_t)pw->len,
                            (const unsigned char *)s->data, (size_t)s->len,
                            (uint64_t)n, (uint64_t)r, (uint64_t)p,
                            0 /* maxmem: OpenSSL default */,
                            out, keylen) == 1;
    if (!ok) {
        free(out);
        zap_crypto_kdf_raise("crypto.scrypt: derivation failed (parameters too large?)");
    }
    void *result = zap_str_new((const char *)out, (int64_t)keylen);
    free(out);
    return result;
}

/* AEAD symmetric encryption (ticket #772, D-0135): AES-256-GCM and
 * ChaCha20-Poly1305 over OpenSSL EVP, mirroring the shape of Python's
 * `cryptography` AESGCM/ChaCha20Poly1305. The 256-bit key and 96-bit (12-byte)
 * nonce are fixed-width; the 16-byte authentication tag is appended to the
 * ciphertext (output = ciphertext || tag). A bad key/nonce length raises
 * ValueError; on decrypt, a ciphertext shorter than the tag, or any
 * authentication failure (tampered ciphertext/AAD/nonce/key), raises a catchable
 * ValueError rather than returning unauthenticated plaintext. */
#define ZAP_AEAD_TAG_LEN 16
#define ZAP_AEAD_KEY_LEN 32
#define ZAP_AEAD_NONCE_LEN 12

static void *zap_crypto_aead_seal(const EVP_CIPHER *cipher, void *key, void *nonce,
                                  void *plaintext, void *aad) {
    const ZapStr *k = (const ZapStr *)key;
    const ZapStr *nn = (const ZapStr *)nonce;
    const ZapStr *pt = (const ZapStr *)plaintext;
    const ZapStr *ad = (const ZapStr *)aad;
    if (k->len != ZAP_AEAD_KEY_LEN) {
        zap_crypto_kdf_raise("crypto AEAD: key must be 32 bytes");
    }
    if (nn->len != ZAP_AEAD_NONCE_LEN) {
        zap_crypto_kdf_raise("crypto AEAD: nonce must be 12 bytes");
    }
    size_t ptlen = (size_t)pt->len;
    uint8_t *out = (uint8_t *)malloc(ptlen + ZAP_AEAD_TAG_LEN);
    if (!out) {
        zap_crypto_kdf_raise("crypto AEAD: out of memory");
    }
    EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
    int outl = 0, tmpl = 0;
    int ok = ctx != NULL &&
             EVP_EncryptInit_ex(ctx, cipher, NULL, NULL, NULL) == 1 &&
             EVP_CIPHER_CTX_ctrl(ctx, EVP_CTRL_AEAD_SET_IVLEN, ZAP_AEAD_NONCE_LEN,
                                 NULL) == 1 &&
             EVP_EncryptInit_ex(ctx, NULL, NULL, (const unsigned char *)k->data,
                                (const unsigned char *)nn->data) == 1;
    if (ok && ad->len > 0) {
        ok = EVP_EncryptUpdate(ctx, NULL, &outl, (const unsigned char *)ad->data,
                               (int)ad->len) == 1;
    }
    if (ok && ptlen > 0) {
        ok = EVP_EncryptUpdate(ctx, out, &outl, (const unsigned char *)pt->data,
                               (int)ptlen) == 1;
    }
    if (ok) {
        /* GCM / Poly1305 are stream ciphers: Final emits no extra bytes. */
        ok = EVP_EncryptFinal_ex(ctx, out + ptlen, &tmpl) == 1 &&
             EVP_CIPHER_CTX_ctrl(ctx, EVP_CTRL_AEAD_GET_TAG, ZAP_AEAD_TAG_LEN,
                                 out + ptlen) == 1;
    }
    if (ctx) {
        EVP_CIPHER_CTX_free(ctx);
    }
    if (!ok) {
        free(out);
        zap_crypto_kdf_raise("crypto AEAD: encryption failed");
    }
    void *result = zap_str_new((const char *)out, (int64_t)(ptlen + ZAP_AEAD_TAG_LEN));
    free(out);
    return result;
}

static void *zap_crypto_aead_open(const EVP_CIPHER *cipher, void *key, void *nonce,
                                  void *ciphertext, void *aad) {
    const ZapStr *k = (const ZapStr *)key;
    const ZapStr *nn = (const ZapStr *)nonce;
    const ZapStr *ct = (const ZapStr *)ciphertext;
    const ZapStr *ad = (const ZapStr *)aad;
    if (k->len != ZAP_AEAD_KEY_LEN) {
        zap_crypto_kdf_raise("crypto AEAD: key must be 32 bytes");
    }
    if (nn->len != ZAP_AEAD_NONCE_LEN) {
        zap_crypto_kdf_raise("crypto AEAD: nonce must be 12 bytes");
    }
    if (ct->len < ZAP_AEAD_TAG_LEN) {
        zap_crypto_kdf_raise("crypto AEAD: ciphertext too short (missing tag)");
    }
    size_t ctlen = (size_t)ct->len - ZAP_AEAD_TAG_LEN;
    const unsigned char *tag = (const unsigned char *)ct->data + ctlen;
    uint8_t *out = (uint8_t *)malloc(ctlen ? ctlen : 1);
    if (!out) {
        zap_crypto_kdf_raise("crypto AEAD: out of memory");
    }
    EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
    int outl = 0, tmpl = 0;
    int ok = ctx != NULL &&
             EVP_DecryptInit_ex(ctx, cipher, NULL, NULL, NULL) == 1 &&
             EVP_CIPHER_CTX_ctrl(ctx, EVP_CTRL_AEAD_SET_IVLEN, ZAP_AEAD_NONCE_LEN,
                                 NULL) == 1 &&
             EVP_DecryptInit_ex(ctx, NULL, NULL, (const unsigned char *)k->data,
                                (const unsigned char *)nn->data) == 1;
    if (ok && ad->len > 0) {
        ok = EVP_DecryptUpdate(ctx, NULL, &outl, (const unsigned char *)ad->data,
                               (int)ad->len) == 1;
    }
    if (ok && ctlen > 0) {
        ok = EVP_DecryptUpdate(ctx, out, &outl, (const unsigned char *)ct->data,
                               (int)ctlen) == 1;
    }
    if (ok) {
        ok = EVP_CIPHER_CTX_ctrl(ctx, EVP_CTRL_AEAD_SET_TAG, ZAP_AEAD_TAG_LEN,
                                 (void *)tag) == 1;
    }
    /* The tag is verified here; a mismatch makes Final return 0. */
    int authed = ok && EVP_DecryptFinal_ex(ctx, out + ctlen, &tmpl) == 1;
    if (ctx) {
        EVP_CIPHER_CTX_free(ctx);
    }
    if (!ok) {
        free(out);
        zap_crypto_kdf_raise("crypto AEAD: decryption setup failed");
    }
    if (!authed) {
        free(out);
        zap_crypto_kdf_raise("crypto AEAD: authentication failed (tag mismatch)");
    }
    void *result = zap_str_new((const char *)out, (int64_t)ctlen);
    free(out);
    return result;
}

void *zap_crypto_aes256gcm_encrypt(void *key, void *nonce, void *pt, void *aad) {
    return zap_crypto_aead_seal(EVP_aes_256_gcm(), key, nonce, pt, aad);
}
void *zap_crypto_aes256gcm_decrypt(void *key, void *nonce, void *ct, void *aad) {
    return zap_crypto_aead_open(EVP_aes_256_gcm(), key, nonce, ct, aad);
}
void *zap_crypto_chacha20poly1305_encrypt(void *key, void *nonce, void *pt, void *aad) {
    return zap_crypto_aead_seal(EVP_chacha20_poly1305(), key, nonce, pt, aad);
}
void *zap_crypto_chacha20poly1305_decrypt(void *key, void *nonce, void *ct, void *aad) {
    return zap_crypto_aead_open(EVP_chacha20_poly1305(), key, nonce, ct, aad);
}

/* ==========================================================================
 * crypto: RSA PKCS#1 v1.5 signing (ticket #628, D-0102)
 * --------------------------------------------------------------------------
 * The only OpenSSL-backed entries in the `crypto` module — the hashing/HMAC/
 * CSPRNG section above is deliberately self-contained, but RSA (bignum
 * modular exponentiation + ASN.1 key parsing) would be a large, security-
 * sensitive reimplementation, and OpenSSL is already linked (`-lssl -lcrypto`)
 * for `tls`. They live here, below the OpenSSL includes, rather than in the
 * reference-hash section.
 *
 * `zap_crypto_rsa_sha1_sign` / `..._sha256_sign` load a PEM private key
 * (PKCS#1 `BEGIN RSA PRIVATE KEY` or PKCS#8 `BEGIN PRIVATE KEY`, both handled
 * transparently by PEM_read_bio_PrivateKey) and return the RAW PKCS#1 v1.5
 * signature over the message digest as a `bytes` value — callers base64 /
 * CloudFront-encode as needed (CloudFront signed URLs, #628). A malformed key
 * or a signing failure raises a catchable ValueError. No GC safepoint runs
 * across the EVP calls, and the OpenSSL objects are non-GC heap freed before
 * return; the GC `bytes` result is the last allocation, mirroring the hash
 * one-shots. */
/* Shared RSA contract: keep key-family and padding policy identical for
 * SHA-1/SHA-256 signing and SHA-256 verification (#642). RSA-PSS-only keys
 * have their own base id and are deliberately excluded. */
static int zap_crypto_is_rsa_key(EVP_PKEY *pkey) {
    return pkey && EVP_PKEY_base_id(pkey) == EVP_PKEY_RSA;
}

static int zap_crypto_rsa_init(EVP_MD_CTX *ctx, EVP_PKEY *pkey,
                              const EVP_MD *md, int signing) {
    EVP_PKEY_CTX *pctx = NULL; /* owned by ctx */
    if (!ctx || !zap_crypto_is_rsa_key(pkey)) {
        return 0;
    }
    int ok = signing ? EVP_DigestSignInit(ctx, &pctx, md, NULL, pkey)
                     : EVP_DigestVerifyInit(ctx, &pctx, md, NULL, pkey);
    return ok == 1 && pctx &&
           EVP_PKEY_CTX_set_rsa_padding(pctx, RSA_PKCS1_PADDING) == 1;
}

/* Diagnostics use the caller's logging threshold/stream/format, with no key,
 * message, signature, or OpenSSL error details. The borrowed literal is consumed
 * synchronously by logging; no GC allocation or new public API is needed. */
static int64_t zap_crypto_rsa_verify_reject(const char *reason) {
    /* Consumed synchronously by logging (no park, no safepoint between the alloc
     * and the read), so a one-object GC string (#837) is safe here. */
    void *message = zap_str_new(reason, (int64_t)strlen(reason));
    zap_logging_log(10, message); /* DEBUG; default threshold is WARNING */
    return 0;
}

static void *zap_crypto_rsa_sign(const EVP_MD *md, void *pem, void *data) {
    ZapStr *k = (ZapStr *)pem;
    ZapStr *d = (ZapStr *)data;
    BIO *bio = BIO_new_mem_buf(k->data, (int)k->len);
    EVP_PKEY *pkey = bio ? PEM_read_bio_PrivateKey(bio, NULL, NULL, NULL) : NULL;
    if (bio) {
        BIO_free(bio);
    }
    if (!pkey) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("crypto: could not parse RSA private key from PEM", 48));
    }
    /* EVP accepts many key families. The RSA-named API must not silently
     * become ECDSA/DSA or RSA-PSS when handed another PEM key type (#642). */
    if (!zap_crypto_is_rsa_key(pkey)) {
        EVP_PKEY_free(pkey);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("crypto: RSA private key required",
                                    sizeof("crypto: RSA private key required") - 1));
    }
    EVP_MD_CTX *ctx = EVP_MD_CTX_new();
    unsigned char *sig = NULL;
    size_t siglen = 0;
    int ok = zap_crypto_rsa_init(ctx, pkey, md, 1) &&
             EVP_DigestSign(ctx, NULL, &siglen, (const unsigned char *)d->data,
                            (size_t)d->len) == 1 &&
             (sig = (unsigned char *)malloc(siglen ? siglen : 1)) != NULL &&
             EVP_DigestSign(ctx, sig, &siglen, (const unsigned char *)d->data,
                            (size_t)d->len) == 1;
    if (!ok) {
        free(sig);
        EVP_MD_CTX_free(ctx);
        EVP_PKEY_free(pkey);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("crypto: RSA signing failed", 26));
    }
    void *out = zap_str_new((const char *)sig, (int64_t)siglen);
    free(sig);
    EVP_MD_CTX_free(ctx);
    EVP_PKEY_free(pkey);
    return out;
}

void *zap_crypto_rsa_sha1_sign(void *pem, void *data) {
    return zap_crypto_rsa_sign(EVP_sha1(), pem, data);
}
void *zap_crypto_rsa_sha256_sign(void *pem, void *data) {
    return zap_crypto_rsa_sign(EVP_sha256(), pem, data);
}

/* Verify an RSA-SHA256 PKCS#1 v1.5 signature against a PEM public key
 * (`BEGIN PUBLIC KEY`). Returns 1 on a valid signature, 0 otherwise — a bad
 * key or any verification failure is a plain `false`, never a panic (#628). */
int64_t zap_crypto_rsa_sha256_verify(void *pem, void *data, void *sig) {
    ZapStr *k = (ZapStr *)pem;
    ZapStr *d = (ZapStr *)data;
    ZapStr *s = (ZapStr *)sig;
    BIO *bio = BIO_new_mem_buf(k->data, (int)k->len);
    EVP_PKEY *pkey = bio ? PEM_read_bio_PUBKEY(bio, NULL, NULL, NULL) : NULL;
    if (bio) {
        BIO_free(bio);
    }
    if (!pkey) {
        return zap_crypto_rsa_verify_reject("crypto.rsa_sha256_verify: invalid public-key PEM");
    }
    if (!zap_crypto_is_rsa_key(pkey)) {
        EVP_PKEY_free(pkey);
        return zap_crypto_rsa_verify_reject("crypto.rsa_sha256_verify: ordinary RSA public key required");
    }
    EVP_MD_CTX *ctx = EVP_MD_CTX_new();
    int initialized = zap_crypto_rsa_init(ctx, pkey, EVP_sha256(), 0);
    int64_t ok = 0;
    if (initialized) {
        ok = EVP_DigestVerify(ctx, (const unsigned char *)s->data, (size_t)s->len,
                              (const unsigned char *)d->data, (size_t)d->len) == 1
                 ? 1
                 : 0;
    }
    EVP_MD_CTX_free(ctx);
    EVP_PKEY_free(pkey);
    if (!initialized) {
        return zap_crypto_rsa_verify_reject("crypto.rsa_sha256_verify: PKCS#1 v1.5 initialization failed");
    }
    return ok;
}

/* ==========================================================================
 * crypto: validated asymmetric-key metadata/import + P-256 ECDSA + Ed25519
 * (ticket #643, D-0126)
 * --------------------------------------------------------------------------
 * A validated, OpenSSL-backed surface over three key families (RSA, P-256 EC,
 * Ed25519) for the JWT library (zaplang-io/jwt). Everything here is PEM- or
 * raw-component-based and exposes NO raw OpenSSL state:
 *   - key_info getters introspect a supported PEM key (kind/bits/curve/private);
 *     `key_kind` is the validating gate and raises a catchable ValueError on an
 *     unparseable, unsupported, or encrypted key.
 *   - `*_public_key_pem` build a validated SubjectPublicKeyInfo PEM from JWK
 *     integer/octet components (RSA n/e, P-256 x/y, Ed25519 x).
 *   - ES256 (`p256_sha256_*`) uses the JOSE fixed-width 64-byte r||s encoding
 *     exclusively (DER is never surfaced; it is converted internally through
 *     ECDSA_SIG for OpenSSL). Ed25519 (`ed25519_*`) is pure Ed25519 over the
 *     original message (RFC 8032) — no implicit SHA-256, no Ed25519ph.
 * Algorithm/key-family/role binding is strict: signing loads only a PRIVATE key
 * of the exact family (a public PEM fails the private read -> ValueError); a
 * verifier loads only a PUBLIC key of the exact family and returns `false` on any
 * mismatch; a wrong curve, wrong family, or wrong signature length can never
 * silently select another primitive. Key loading NEVER prompts on stdin: a
 * zero-returning passphrase callback makes encrypted keys fail deterministically.
 * Signing raises a catchable ValueError on failure; verification is always a
 * plain boolean (bad key / bad signature -> `false`), mirroring the RSA surface.
 * OpenSSL objects are non-GC heap freed before return; the GC `bytes` result is
 * the last allocation, as in the hash one-shots. The APIs compile on the unified
 * post-1.1.0 OpenSSL API and LibreSSL (some calls are deprecated-but-present on
 * OpenSSL 3.x; the build does not use -Werror). */

/* Upper bounds on imported RSA components, to keep imports bounded (#643). A
 * 1024-byte modulus is an 8192-bit key — larger than any JWK RS256 deployment —
 * and a public exponent needs only a few bytes (65537 is 3). */
#define ZAP_CRYPTO_RSA_N_MAX 1024
#define ZAP_CRYPTO_RSA_E_MAX 16
/* P-256 affine coordinates and Ed25519 public keys are exactly 32 bytes; ES256
 * and Ed25519 signatures are exactly 64 bytes (RFC 7518 §3.4 / RFC 8032). */
#define ZAP_CRYPTO_P256_COORD_LEN 32
#define ZAP_CRYPTO_ED25519_KEY_LEN 32
#define ZAP_CRYPTO_JOSE_SIG_LEN 64

/* Never supply a passphrase: encrypted PEM keys fail deterministically instead
 * of OpenSSL's default of prompting on the controlling terminal (#643). */
static int zap_crypto_no_pw(char *buf, int size, int rwflag, void *u) {
    (void)buf;
    (void)size;
    (void)rwflag;
    (void)u;
    return 0;
}

/* A GC `str` from a NUL-terminated C literal, length measured with strlen so the
 * error-message literals below never carry a miscounted length. */
static void *zap_crypto_cstr(const char *s) {
    return zap_str_new(s, (int64_t)strlen(s));
}

/* Load a PEM key, trying a private key first and then a public key, over
 * independent read-only memory BIOs (a read-only mem BIO is consumed by the
 * first read). `*is_private` is set to 1/0 on success. Returns NULL if neither
 * parse succeeds (including encrypted keys, via the zero-returning callback). */
static EVP_PKEY *zap_crypto_pem_to_pkey(ZapStr *pem, int *is_private) {
    BIO *b1 = BIO_new_mem_buf(pem->data, (int)pem->len);
    EVP_PKEY *pkey =
        b1 ? PEM_read_bio_PrivateKey(b1, NULL, zap_crypto_no_pw, NULL) : NULL;
    if (b1) {
        BIO_free(b1);
    }
    if (pkey) {
        if (is_private) {
            *is_private = 1;
        }
        return pkey;
    }
    BIO *b2 = BIO_new_mem_buf(pem->data, (int)pem->len);
    pkey = b2 ? PEM_read_bio_PUBKEY(b2, NULL, zap_crypto_no_pw, NULL) : NULL;
    if (b2) {
        BIO_free(b2);
    }
    if (pkey && is_private) {
        *is_private = 0;
    }
    return pkey;
}

/* Copy a memory BIO's whole contents into a fresh GC `bytes`/`str` value. */
static void *zap_crypto_bio_to_bytes(BIO *bio) {
    BUF_MEM *bptr = NULL;
    BIO_get_mem_ptr(bio, &bptr);
    if (!bptr || !bptr->data) {
        return zap_crypto_cstr("");
    }
    return zap_str_new(bptr->data, (int64_t)bptr->length);
}

/* The P-256 (prime256v1) group NID of an EC `pkey`, or NID_undef for a non-EC
 * key. Uses the legacy `EVP_PKEY_get0_EC_KEY` accessor (present on OpenSSL
 * 1.1+/3.x and LibreSSL). */
static int zap_crypto_ec_curve_nid(EVP_PKEY *pkey) {
    if (!pkey || EVP_PKEY_base_id(pkey) != EVP_PKEY_EC) {
        return NID_undef;
    }
    const EC_KEY *ec = EVP_PKEY_get0_EC_KEY(pkey);
    if (!ec) {
        return NID_undef;
    }
    const EC_GROUP *grp = EC_KEY_get0_group(ec);
    return grp ? EC_GROUP_get_curve_name(grp) : NID_undef;
}

/* crypto.key_info getters (#643). `key_kind` validates and is the gate. */
void *zap_crypto_key_kind(void *pem) {
    int is_priv = 0;
    EVP_PKEY *pkey = zap_crypto_pem_to_pkey((ZapStr *)pem, &is_priv);
    if (!pkey) {
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.key_info: could not parse a supported "
                        "public/private key from PEM (encrypted keys are "
                        "rejected)"));
    }
    const char *kind = NULL;
    switch (EVP_PKEY_base_id(pkey)) {
    case EVP_PKEY_RSA:
        kind = "RSA";
        break;
    case EVP_PKEY_EC:
        kind = "EC";
        break;
    case EVP_PKEY_ED25519:
        kind = "Ed25519";
        break;
    default:
        break;
    }
    EVP_PKEY_free(pkey);
    if (!kind) {
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.key_info: unsupported key type (expected RSA, "
                        "EC, or Ed25519)"));
    }
    return zap_str_new(kind, (int64_t)strlen(kind));
}

int64_t zap_crypto_key_bits(void *pem) {
    int is_priv = 0;
    EVP_PKEY *pkey = zap_crypto_pem_to_pkey((ZapStr *)pem, &is_priv);
    if (!pkey) {
        return 0;
    }
    int bits = EVP_PKEY_bits(pkey);
    EVP_PKEY_free(pkey);
    return (int64_t)(bits > 0 ? bits : 0);
}

void *zap_crypto_key_curve(void *pem) {
    int is_priv = 0;
    EVP_PKEY *pkey = zap_crypto_pem_to_pkey((ZapStr *)pem, &is_priv);
    const char *curve = "";
    if (pkey) {
        switch (zap_crypto_ec_curve_nid(pkey)) {
        case NID_X9_62_prime256v1:
            curve = "P-256";
            break;
        case NID_secp384r1:
            curve = "P-384";
            break;
        case NID_secp521r1:
            curve = "P-521";
            break;
        case NID_undef:
            curve = "";
            break;
        default: {
            const char *sn = OBJ_nid2sn(zap_crypto_ec_curve_nid(pkey));
            curve = sn ? sn : "";
            break;
        }
        }
    }
    void *out = zap_str_new(curve, (int64_t)strlen(curve));
    if (pkey) {
        EVP_PKEY_free(pkey);
    }
    return out;
}

int64_t zap_crypto_key_is_private(void *pem) {
    int is_priv = 0;
    EVP_PKEY *pkey = zap_crypto_pem_to_pkey((ZapStr *)pem, &is_priv);
    if (pkey) {
        EVP_PKEY_free(pkey);
    }
    return is_priv ? 1 : 0;
}

/* Serialize a built public-key EVP_PKEY as SubjectPublicKeyInfo PEM bytes, then
 * free it. Raises ValueError (via `reason`) if serialization fails. */
static void *zap_crypto_pkey_to_pub_pem(EVP_PKEY *pkey, const char *reason) {
    BIO *bio = BIO_new(BIO_s_mem());
    int ok = bio && PEM_write_bio_PUBKEY(bio, pkey) == 1;
    void *out = ok ? zap_crypto_bio_to_bytes(bio) : NULL;
    if (bio) {
        BIO_free(bio);
    }
    EVP_PKEY_free(pkey);
    if (!ok) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new(reason, (int64_t)strlen(reason)));
    }
    return out;
}

void *zap_crypto_rsa_public_key_pem(void *n_bytes, void *e_bytes) {
    ZapStr *n = (ZapStr *)n_bytes;
    ZapStr *e = (ZapStr *)e_bytes;
    if (n->len <= 0 || e->len <= 0 || n->len > ZAP_CRYPTO_RSA_N_MAX ||
        e->len > ZAP_CRYPTO_RSA_E_MAX) {
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.rsa_public_key_pem: n/e out of bounds"));
    }
    BIGNUM *bn = BN_bin2bn((const unsigned char *)n->data, (int)n->len, NULL);
    BIGNUM *be = BN_bin2bn((const unsigned char *)e->data, (int)e->len, NULL);
    /* BN_bin2bn reads an unsigned big-endian integer, so both are >= 0. A usable
     * RSA public key needs an odd modulus (product of odd primes) and an odd
     * exponent >= 3 (an even e is not invertible mod lambda(n)). */
    int valid = bn && be && !BN_is_zero(bn) && !BN_is_zero(be) &&
                BN_is_odd(bn) && BN_is_odd(be) && BN_cmp(be, BN_value_one()) > 0;
    if (!valid) {
        BN_free(bn);
        BN_free(be);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_crypto_cstr("crypto.rsa_public_key_pem: invalid RSA "
                                    "public parameters (n must be odd, e must "
                                    "be an odd integer > 1)"));
    }
    RSA *rsa = RSA_new();
    EVP_PKEY *pkey = EVP_PKEY_new();
    /* RSA_set0_key takes ownership of bn/be on success; d is NULL for a public
     * key. EVP_PKEY_assign_RSA takes ownership of rsa on success. */
    if (!rsa || !pkey || RSA_set0_key(rsa, bn, be, NULL) != 1) {
        BN_free(bn);
        BN_free(be);
        RSA_free(rsa);
        EVP_PKEY_free(pkey);
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.rsa_public_key_pem: key assembly failed"));
    }
    if (EVP_PKEY_assign_RSA(pkey, rsa) != 1) {
        RSA_free(rsa);
        EVP_PKEY_free(pkey);
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.rsa_public_key_pem: key assembly failed"));
    }
    return zap_crypto_pkey_to_pub_pem(
        pkey, "crypto.rsa_public_key_pem: PEM serialization failed");
}

void *zap_crypto_p256_public_key_pem(void *x_bytes, void *y_bytes) {
    ZapStr *x = (ZapStr *)x_bytes;
    ZapStr *y = (ZapStr *)y_bytes;
    if (x->len != ZAP_CRYPTO_P256_COORD_LEN ||
        y->len != ZAP_CRYPTO_P256_COORD_LEN) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_crypto_cstr("crypto.p256_public_key_pem: each "
                                    "coordinate must be exactly 32 bytes"));
    }
    EC_KEY *ec = EC_KEY_new_by_curve_name(NID_X9_62_prime256v1);
    BIGNUM *bx = BN_bin2bn((const unsigned char *)x->data, (int)x->len, NULL);
    BIGNUM *by = BN_bin2bn((const unsigned char *)y->data, (int)y->len, NULL);
    /* set_public_key_affine_coordinates rejects a point not on the curve; the
     * follow-up EC_KEY_check_key rejects the point at infinity and bad order. */
    int on_curve = ec && bx && by &&
                   EC_KEY_set_public_key_affine_coordinates(ec, bx, by) == 1 &&
                   EC_KEY_check_key(ec) == 1;
    BN_free(bx);
    BN_free(by);
    if (!on_curve) {
        EC_KEY_free(ec);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_crypto_cstr("crypto.p256_public_key_pem: (x, y) is not "
                                    "a valid point on P-256"));
    }
    EVP_PKEY *pkey = EVP_PKEY_new();
    if (!pkey || EVP_PKEY_assign_EC_KEY(pkey, ec) != 1) {
        EC_KEY_free(ec);
        EVP_PKEY_free(pkey);
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.p256_public_key_pem: key assembly failed"));
    }
    return zap_crypto_pkey_to_pub_pem(
        pkey, "crypto.p256_public_key_pem: PEM serialization failed");
}

void *zap_crypto_ed25519_public_key_pem(void *x_bytes) {
    ZapStr *x = (ZapStr *)x_bytes;
    if (x->len != ZAP_CRYPTO_ED25519_KEY_LEN) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_crypto_cstr("crypto.ed25519_public_key_pem: public key "
                                    "must be exactly 32 bytes"));
    }
    EVP_PKEY *pkey = EVP_PKEY_new_raw_public_key(
        EVP_PKEY_ED25519, NULL, (const unsigned char *)x->data, (size_t)x->len);
    if (!pkey) {
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.ed25519_public_key_pem: invalid Ed25519 public "
                        "key"));
    }
    return zap_crypto_pkey_to_pub_pem(
        pkey, "crypto.ed25519_public_key_pem: PEM serialization failed");
}

/* Private-key PEM builders from private JWK components (ticket #778, D-0136):
 * the symmetric counterparts to the *_public_key_pem builders above. Each
 * assembles an EVP_PKEY from raw components, lets OpenSSL validate it, and emits
 * an unencrypted PKCS#8 `BEGIN PRIVATE KEY` PEM. All ASN.1/DER stays in OpenSSL
 * (D-0126). Malformed/inconsistent components raise a catchable ValueError. */

/* Serialize a built private-key EVP_PKEY as PKCS#8 PEM bytes, then free it. */
static void *zap_crypto_pkey_to_priv_pem(EVP_PKEY *pkey, const char *reason) {
    BIO *bio = BIO_new(BIO_s_mem());
    int ok =
        bio && PEM_write_bio_PrivateKey(bio, pkey, NULL, NULL, 0, NULL, NULL) == 1;
    void *out = ok ? zap_crypto_bio_to_bytes(bio) : NULL;
    if (bio) {
        BIO_free(bio);
    }
    EVP_PKEY_free(pkey);
    if (!ok) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new(reason, (int64_t)strlen(reason)));
    }
    return out;
}

void *zap_crypto_rsa_private_key_pem(void *n_b, void *e_b, void *d_b, void *p_b,
                                     void *q_b, void *dp_b, void *dq_b,
                                     void *qi_b) {
    ZapStr *parts[8] = {(ZapStr *)n_b,  (ZapStr *)e_b,  (ZapStr *)d_b,
                        (ZapStr *)p_b,  (ZapStr *)q_b,  (ZapStr *)dp_b,
                        (ZapStr *)dq_b, (ZapStr *)qi_b};
    for (int i = 0; i < 8; i++) {
        if (parts[i]->len <= 0 || parts[i]->len > ZAP_CRYPTO_RSA_N_MAX) {
            zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                            zap_crypto_cstr("crypto.rsa_private_key_pem: a "
                                            "component is empty or too large"));
        }
    }
    BIGNUM *bn[8];
    int built = 1;
    for (int i = 0; i < 8; i++) {
        bn[i] = BN_bin2bn((const unsigned char *)parts[i]->data,
                          (int)parts[i]->len, NULL);
        if (!bn[i]) {
            built = 0;
        }
    }
    RSA *rsa = built ? RSA_new() : NULL;
    EVP_PKEY *pkey = rsa ? EVP_PKEY_new() : NULL;
    /* set0_* take ownership of their BIGNUMs only on success; track transfers so
     * cleanup frees exactly the untransferred ones (RSA_free frees the rest). */
    int owned_key = 0, owned_fac = 0, owned_crt = 0;
    if (rsa && pkey) {
        owned_key = RSA_set0_key(rsa, bn[0], bn[1], bn[2]) == 1;
        if (owned_key) {
            owned_fac = RSA_set0_factors(rsa, bn[3], bn[4]) == 1;
        }
        if (owned_fac) {
            owned_crt = RSA_set0_crt_params(rsa, bn[5], bn[6], bn[7]) == 1;
        }
    }
    /* RSA_check_key verifies p*q == n, e*d ≡ 1, and the CRT exponents/coeff. */
    int ok = owned_crt && RSA_check_key(rsa) == 1;
    if (!ok) {
        if (!owned_key) {
            for (int i = 0; i < 8; i++) {
                BN_free(bn[i]);
            }
        } else if (!owned_fac) {
            for (int i = 3; i < 8; i++) {
                BN_free(bn[i]);
            }
        } else if (!owned_crt) {
            for (int i = 5; i < 8; i++) {
                BN_free(bn[i]);
            }
        }
        RSA_free(rsa);
        EVP_PKEY_free(pkey);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_crypto_cstr("crypto.rsa_private_key_pem: inconsistent "
                                        "or invalid RSA private components"));
    }
    if (EVP_PKEY_assign_RSA(pkey, rsa) != 1) {
        RSA_free(rsa);
        EVP_PKEY_free(pkey);
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.rsa_private_key_pem: key assembly failed"));
    }
    return zap_crypto_pkey_to_priv_pem(
        pkey, "crypto.rsa_private_key_pem: PEM serialization failed");
}

void *zap_crypto_p256_private_key_pem(void *d_bytes) {
    ZapStr *d = (ZapStr *)d_bytes;
    if (d->len != ZAP_CRYPTO_P256_COORD_LEN) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_crypto_cstr("crypto.p256_private_key_pem: scalar d "
                                        "must be exactly 32 bytes"));
    }
    EC_KEY *ec = EC_KEY_new_by_curve_name(NID_X9_62_prime256v1);
    BIGNUM *bd = BN_bin2bn((const unsigned char *)d->data, (int)d->len, NULL);
    const EC_GROUP *grp = ec ? EC_KEY_get0_group(ec) : NULL;
    BIGNUM *order = BN_new();
    BN_CTX *ctx = BN_CTX_new();
    EC_POINT *pub = grp ? EC_POINT_new(grp) : NULL;
    /* d must lie in [1, n-1]; derive the public point pub = d*G and let
     * EC_KEY_check_key confirm the pair is consistent and on the curve. */
    int ok = ec && bd && grp && order && ctx && pub &&
             EC_GROUP_get_order(grp, order, ctx) == 1 && !BN_is_zero(bd) &&
             BN_cmp(bd, order) < 0 && EC_KEY_set_private_key(ec, bd) == 1 &&
             EC_POINT_mul(grp, pub, bd, NULL, NULL, ctx) == 1 &&
             EC_KEY_set_public_key(ec, pub) == 1 && EC_KEY_check_key(ec) == 1;
    BN_free(bd);
    BN_free(order);
    BN_CTX_free(ctx);
    EC_POINT_free(pub);
    if (!ok) {
        EC_KEY_free(ec);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_crypto_cstr("crypto.p256_private_key_pem: invalid "
                                        "P-256 private scalar"));
    }
    EVP_PKEY *pkey = EVP_PKEY_new();
    if (!pkey || EVP_PKEY_assign_EC_KEY(pkey, ec) != 1) {
        EC_KEY_free(ec);
        EVP_PKEY_free(pkey);
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.p256_private_key_pem: key assembly failed"));
    }
    return zap_crypto_pkey_to_priv_pem(
        pkey, "crypto.p256_private_key_pem: PEM serialization failed");
}

void *zap_crypto_ed25519_private_key_pem(void *d_bytes) {
    ZapStr *d = (ZapStr *)d_bytes;
    if (d->len != ZAP_CRYPTO_ED25519_KEY_LEN) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_crypto_cstr("crypto.ed25519_private_key_pem: seed "
                                        "must be exactly 32 bytes"));
    }
    EVP_PKEY *pkey = EVP_PKEY_new_raw_private_key(
        EVP_PKEY_ED25519, NULL, (const unsigned char *)d->data, (size_t)d->len);
    if (!pkey) {
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.ed25519_private_key_pem: invalid Ed25519 seed"));
    }
    return zap_crypto_pkey_to_priv_pem(
        pkey, "crypto.ed25519_private_key_pem: PEM serialization failed");
}

/* ES256 signing (#643): load a P-256 private key, sign SHA-256 of the message,
 * and return the signature as JOSE fixed-width 64-byte r||s. Raises ValueError
 * on a non-private key, a non-P-256 key, or a signing failure. */
void *zap_crypto_p256_sha256_sign(void *pem, void *msg) {
    ZapStr *k = (ZapStr *)pem;
    ZapStr *m = (ZapStr *)msg;
    BIO *bio = BIO_new_mem_buf(k->data, (int)k->len);
    EVP_PKEY *pkey =
        bio ? PEM_read_bio_PrivateKey(bio, NULL, zap_crypto_no_pw, NULL) : NULL;
    if (bio) {
        BIO_free(bio);
    }
    if (!pkey) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_crypto_cstr("crypto.p256_sha256_sign: could not parse a "
                                    "P-256 private key from PEM"));
    }
    if (zap_crypto_ec_curve_nid(pkey) != NID_X9_62_prime256v1) {
        EVP_PKEY_free(pkey);
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.p256_sha256_sign: a P-256 (prime256v1) private "
                        "key is required"));
    }
    EVP_MD_CTX *ctx = EVP_MD_CTX_new();
    unsigned char *der = NULL;
    size_t derlen = 0;
    int ok =
        ctx &&
        EVP_DigestSignInit(ctx, NULL, EVP_sha256(), NULL, pkey) == 1 &&
        EVP_DigestSign(ctx, NULL, &derlen, (const unsigned char *)m->data,
                       (size_t)m->len) == 1 &&
        (der = (unsigned char *)malloc(derlen ? derlen : 1)) != NULL &&
        EVP_DigestSign(ctx, der, &derlen, (const unsigned char *)m->data,
                       (size_t)m->len) == 1;
    EVP_MD_CTX_free(ctx);
    EVP_PKEY_free(pkey);
    /* Convert the DER ECDSA-Sig-Value to JOSE fixed-width 64-byte r||s. */
    unsigned char jose[ZAP_CRYPTO_JOSE_SIG_LEN];
    const unsigned char *dp = der;
    ECDSA_SIG *sig = ok ? d2i_ECDSA_SIG(NULL, &dp, (long)derlen) : NULL;
    if (sig) {
        const BIGNUM *r = NULL;
        const BIGNUM *s = NULL;
        ECDSA_SIG_get0(sig, &r, &s);
        if (BN_bn2binpad(r, jose, 32) != 32 ||
            BN_bn2binpad(s, jose + 32, 32) != 32) {
            ok = 0;
        }
        ECDSA_SIG_free(sig);
    } else {
        ok = 0;
    }
    free(der);
    if (!ok) {
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.p256_sha256_sign: ECDSA signing failed"));
    }
    return zap_str_new((const char *)jose, ZAP_CRYPTO_JOSE_SIG_LEN);
}

int64_t zap_crypto_p256_sha256_verify(void *pem, void *msg, void *sig) {
    ZapStr *k = (ZapStr *)pem;
    ZapStr *m = (ZapStr *)msg;
    ZapStr *s = (ZapStr *)sig;
    BIO *bio = BIO_new_mem_buf(k->data, (int)k->len);
    EVP_PKEY *pkey =
        bio ? PEM_read_bio_PUBKEY(bio, NULL, zap_crypto_no_pw, NULL) : NULL;
    if (bio) {
        BIO_free(bio);
    }
    if (!pkey) {
        return zap_crypto_rsa_verify_reject(
            "crypto.p256_sha256_verify: invalid public-key PEM");
    }
    if (zap_crypto_ec_curve_nid(pkey) != NID_X9_62_prime256v1) {
        EVP_PKEY_free(pkey);
        return zap_crypto_rsa_verify_reject(
            "crypto.p256_sha256_verify: a P-256 public key is required");
    }
    if (s->len != ZAP_CRYPTO_JOSE_SIG_LEN) {
        EVP_PKEY_free(pkey);
        return zap_crypto_rsa_verify_reject(
            "crypto.p256_sha256_verify: signature must be 64 bytes (JOSE r||s)");
    }
    /* Rebuild the DER ECDSA-Sig-Value from the JOSE r||s halves. */
    ECDSA_SIG *es = ECDSA_SIG_new();
    BIGNUM *r = BN_bin2bn((const unsigned char *)s->data, 32, NULL);
    BIGNUM *sv = BN_bin2bn((const unsigned char *)s->data + 32, 32, NULL);
    unsigned char *der = NULL;
    int derlen = 0;
    int built = es && r && sv && ECDSA_SIG_set0(es, r, sv) == 1;
    if (built) {
        derlen = i2d_ECDSA_SIG(es, &der); /* allocates der */
    } else {
        BN_free(r);
        BN_free(sv);
    }
    int64_t ok = 0;
    if (built && derlen > 0) {
        EVP_MD_CTX *ctx = EVP_MD_CTX_new();
        ok = (ctx &&
              EVP_DigestVerifyInit(ctx, NULL, EVP_sha256(), NULL, pkey) == 1 &&
              EVP_DigestVerify(ctx, der, (size_t)derlen,
                               (const unsigned char *)m->data,
                               (size_t)m->len) == 1)
                 ? 1
                 : 0;
        EVP_MD_CTX_free(ctx);
    }
    OPENSSL_free(der);
    ECDSA_SIG_free(es); /* frees r/sv once set0 took ownership */
    EVP_PKEY_free(pkey);
    if (!ok) {
        return zap_crypto_rsa_verify_reject(
            "crypto.p256_sha256_verify: signature did not verify");
    }
    return 1;
}

/* Pure Ed25519 signing (#643, RFC 8032): one-shot EVP_DigestSign with no digest
 * over the original message. Returns the 64-byte signature. */
void *zap_crypto_ed25519_sign(void *pem, void *msg) {
    ZapStr *k = (ZapStr *)pem;
    ZapStr *m = (ZapStr *)msg;
    BIO *bio = BIO_new_mem_buf(k->data, (int)k->len);
    EVP_PKEY *pkey =
        bio ? PEM_read_bio_PrivateKey(bio, NULL, zap_crypto_no_pw, NULL) : NULL;
    if (bio) {
        BIO_free(bio);
    }
    if (!pkey) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_crypto_cstr("crypto.ed25519_sign: could not parse an "
                                    "Ed25519 private key from PEM"));
    }
    if (EVP_PKEY_base_id(pkey) != EVP_PKEY_ED25519) {
        EVP_PKEY_free(pkey);
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.ed25519_sign: an Ed25519 private key is required"));
    }
    EVP_MD_CTX *ctx = EVP_MD_CTX_new();
    unsigned char sig[ZAP_CRYPTO_JOSE_SIG_LEN];
    size_t siglen = sizeof(sig);
    int ok = ctx &&
             EVP_DigestSignInit(ctx, NULL, NULL, NULL, pkey) == 1 &&
             EVP_DigestSign(ctx, sig, &siglen, (const unsigned char *)m->data,
                            (size_t)m->len) == 1 &&
             siglen == ZAP_CRYPTO_JOSE_SIG_LEN;
    EVP_MD_CTX_free(ctx);
    EVP_PKEY_free(pkey);
    if (!ok) {
        zap_panic_typed(
            ZAP_EXC_VALUE_ERROR,
            zap_crypto_cstr("crypto.ed25519_sign: Ed25519 signing failed"));
    }
    return zap_str_new((const char *)sig, ZAP_CRYPTO_JOSE_SIG_LEN);
}

int64_t zap_crypto_ed25519_verify(void *pem, void *msg, void *sig) {
    ZapStr *k = (ZapStr *)pem;
    ZapStr *m = (ZapStr *)msg;
    ZapStr *s = (ZapStr *)sig;
    BIO *bio = BIO_new_mem_buf(k->data, (int)k->len);
    EVP_PKEY *pkey =
        bio ? PEM_read_bio_PUBKEY(bio, NULL, zap_crypto_no_pw, NULL) : NULL;
    if (bio) {
        BIO_free(bio);
    }
    if (!pkey) {
        return zap_crypto_rsa_verify_reject(
            "crypto.ed25519_verify: invalid public-key PEM");
    }
    if (EVP_PKEY_base_id(pkey) != EVP_PKEY_ED25519) {
        EVP_PKEY_free(pkey);
        return zap_crypto_rsa_verify_reject(
            "crypto.ed25519_verify: an Ed25519 public key is required");
    }
    if (s->len != ZAP_CRYPTO_JOSE_SIG_LEN) {
        EVP_PKEY_free(pkey);
        return zap_crypto_rsa_verify_reject(
            "crypto.ed25519_verify: signature must be exactly 64 bytes");
    }
    EVP_MD_CTX *ctx = EVP_MD_CTX_new();
    int64_t ok = (ctx &&
                  EVP_DigestVerifyInit(ctx, NULL, NULL, NULL, pkey) == 1 &&
                  EVP_DigestVerify(ctx, (const unsigned char *)s->data,
                                   (size_t)s->len,
                                   (const unsigned char *)m->data,
                                   (size_t)m->len) == 1)
                     ? 1
                     : 0;
    EVP_MD_CTX_free(ctx);
    EVP_PKEY_free(pkey);
    if (!ok) {
        return zap_crypto_rsa_verify_reject(
            "crypto.ed25519_verify: signature did not verify");
    }
    return 1;
}

/* ==========================================================================
 * crypto: SHA-3 and BLAKE2 hash families (ticket #770, D-0133)
 * --------------------------------------------------------------------------
 * Unlike the SHA-1/2 + MD5 core above (self-contained FIPS/RFC C), SHA-3 and
 * BLAKE2 are taken from OpenSSL's `EVP_Digest` (both present in OpenSSL
 * 1.1.1+/3.x, already linked) rather than hand-porting Keccak / BLAKE2 — a
 * deliberate trade recorded in D-0133. One-shot only in this cut: the streaming
 * handle model stores a pointer-free context inline in a GC buffer, which an
 * opaque `EVP_MD_CTX*` cannot do without a finalizer, and there is no consumer
 * for streaming SHA-3/BLAKE2 yet. BLAKE2b defaults to 512-bit and BLAKE2s to
 * 256-bit output, matching Python hashlib.blake2b/blake2s defaults. */
static void *zap_crypto_evp_oneshot_hex(const EVP_MD *md, void *data) {
    const ZapStr *d = (const ZapStr *)data;
    unsigned char out[EVP_MAX_MD_SIZE];
    unsigned int outlen = 0;
    if (EVP_Digest((const unsigned char *)d->data, (size_t)d->len, out, &outlen,
                   md, NULL) != 1) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_crypto_cstr("crypto: hash computation failed"));
    }
    return zap_crypto_hex_str(out, (size_t)outlen);
}
void *zap_crypto_sha3_256(void *data) {
    return zap_crypto_evp_oneshot_hex(EVP_sha3_256(), data);
}
void *zap_crypto_sha3_512(void *data) {
    return zap_crypto_evp_oneshot_hex(EVP_sha3_512(), data);
}
void *zap_crypto_blake2b(void *data) {
    return zap_crypto_evp_oneshot_hex(EVP_blake2b512(), data);
}
void *zap_crypto_blake2s(void *data) {
    return zap_crypto_evp_oneshot_hex(EVP_blake2s256(), data);
}

/* ==========================================================================
 * crypto: RSA-PSS (PS256/384/512) and RSA PKCS#1 v1.5 SHA-384/512 (ticket
 * #768, D-0133)
 * --------------------------------------------------------------------------
 * Broadens RSA signing beyond PKCS#1 v1.5/SHA-256 (#628/D-0102). PSS is the
 * JOSE-recommended scheme; RS384/RS512 need the larger SHA-2 digests (#765).
 * Shared contract with the existing RSA entries: the key must be an ordinary
 * RSA key (RSA-PSS-only keys have their own base id and are excluded); signing
 * raises a catchable ValueError on failure; verification returns a plain
 * boolean, false on any mismatch/tamper, never a panic. For PSS the salt length
 * is the digest length and MGF1 uses the same hash (the JOSE convention). */
static void *zap_crypto_rsa_sign_ex(const EVP_MD *md, int padding, void *pem,
                                    void *data, const char *fname) {
    ZapStr *k = (ZapStr *)pem;
    ZapStr *d = (ZapStr *)data;
    BIO *bio = BIO_new_mem_buf(k->data, (int)k->len);
    EVP_PKEY *pkey =
        bio ? PEM_read_bio_PrivateKey(bio, NULL, zap_crypto_no_pw, NULL) : NULL;
    if (bio) {
        BIO_free(bio);
    }
    if (!pkey) {
        char msg[96];
        snprintf(msg, sizeof(msg), "%s: could not parse RSA private key from PEM",
                 fname);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_crypto_cstr(msg));
    }
    if (!zap_crypto_is_rsa_key(pkey)) {
        EVP_PKEY_free(pkey);
        char msg[64];
        snprintf(msg, sizeof(msg), "%s: RSA private key required", fname);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_crypto_cstr(msg));
    }
    EVP_MD_CTX *ctx = EVP_MD_CTX_new();
    EVP_PKEY_CTX *pctx = NULL;
    unsigned char *sig = NULL;
    size_t siglen = 0;
    int ok = ctx && EVP_DigestSignInit(ctx, &pctx, md, NULL, pkey) == 1 && pctx &&
             EVP_PKEY_CTX_set_rsa_padding(pctx, padding) == 1;
    if (ok && padding == RSA_PKCS1_PSS_PADDING) {
        ok = EVP_PKEY_CTX_set_rsa_pss_saltlen(pctx, RSA_PSS_SALTLEN_DIGEST) == 1 &&
             EVP_PKEY_CTX_set_rsa_mgf1_md(pctx, md) == 1;
    }
    ok = ok &&
         EVP_DigestSign(ctx, NULL, &siglen, (const unsigned char *)d->data,
                        (size_t)d->len) == 1 &&
         (sig = (unsigned char *)malloc(siglen ? siglen : 1)) != NULL &&
         EVP_DigestSign(ctx, sig, &siglen, (const unsigned char *)d->data,
                        (size_t)d->len) == 1;
    if (!ok) {
        free(sig);
        EVP_MD_CTX_free(ctx);
        EVP_PKEY_free(pkey);
        char msg[48];
        snprintf(msg, sizeof(msg), "%s: signing failed", fname);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_crypto_cstr(msg));
    }
    void *out = zap_str_new((const char *)sig, (int64_t)siglen);
    free(sig);
    EVP_MD_CTX_free(ctx);
    EVP_PKEY_free(pkey);
    return out;
}

static int64_t zap_crypto_rsa_verify_ex(const EVP_MD *md, int padding, void *pem,
                                        void *data, void *sig,
                                        const char *reason) {
    ZapStr *k = (ZapStr *)pem;
    ZapStr *d = (ZapStr *)data;
    ZapStr *s = (ZapStr *)sig;
    BIO *bio = BIO_new_mem_buf(k->data, (int)k->len);
    EVP_PKEY *pkey =
        bio ? PEM_read_bio_PUBKEY(bio, NULL, zap_crypto_no_pw, NULL) : NULL;
    if (bio) {
        BIO_free(bio);
    }
    if (!pkey) {
        return zap_crypto_rsa_verify_reject(reason);
    }
    if (!zap_crypto_is_rsa_key(pkey)) {
        EVP_PKEY_free(pkey);
        return zap_crypto_rsa_verify_reject(reason);
    }
    EVP_MD_CTX *ctx = EVP_MD_CTX_new();
    EVP_PKEY_CTX *pctx = NULL;
    int init = ctx && EVP_DigestVerifyInit(ctx, &pctx, md, NULL, pkey) == 1 &&
               pctx && EVP_PKEY_CTX_set_rsa_padding(pctx, padding) == 1;
    if (init && padding == RSA_PKCS1_PSS_PADDING) {
        init = EVP_PKEY_CTX_set_rsa_pss_saltlen(pctx, RSA_PSS_SALTLEN_DIGEST) == 1 &&
               EVP_PKEY_CTX_set_rsa_mgf1_md(pctx, md) == 1;
    }
    int64_t ok = 0;
    if (init) {
        ok = EVP_DigestVerify(ctx, (const unsigned char *)s->data, (size_t)s->len,
                              (const unsigned char *)d->data, (size_t)d->len) == 1
                 ? 1
                 : 0;
    }
    EVP_MD_CTX_free(ctx);
    EVP_PKEY_free(pkey);
    return ok ? 1 : zap_crypto_rsa_verify_reject(reason);
}

void *zap_crypto_rsa_sha384_sign(void *pem, void *data) {
    return zap_crypto_rsa_sign_ex(EVP_sha384(), RSA_PKCS1_PADDING, pem, data,
                                  "crypto.rsa_sha384_sign");
}
void *zap_crypto_rsa_sha512_sign(void *pem, void *data) {
    return zap_crypto_rsa_sign_ex(EVP_sha512(), RSA_PKCS1_PADDING, pem, data,
                                  "crypto.rsa_sha512_sign");
}
int64_t zap_crypto_rsa_sha384_verify(void *pem, void *data, void *sig) {
    return zap_crypto_rsa_verify_ex(EVP_sha384(), RSA_PKCS1_PADDING, pem, data,
                                    sig, "crypto.rsa_sha384_verify failed");
}
int64_t zap_crypto_rsa_sha512_verify(void *pem, void *data, void *sig) {
    return zap_crypto_rsa_verify_ex(EVP_sha512(), RSA_PKCS1_PADDING, pem, data,
                                    sig, "crypto.rsa_sha512_verify failed");
}
void *zap_crypto_rsa_pss_sha256_sign(void *pem, void *data) {
    return zap_crypto_rsa_sign_ex(EVP_sha256(), RSA_PKCS1_PSS_PADDING, pem, data,
                                  "crypto.rsa_pss_sha256_sign");
}
void *zap_crypto_rsa_pss_sha384_sign(void *pem, void *data) {
    return zap_crypto_rsa_sign_ex(EVP_sha384(), RSA_PKCS1_PSS_PADDING, pem, data,
                                  "crypto.rsa_pss_sha384_sign");
}
void *zap_crypto_rsa_pss_sha512_sign(void *pem, void *data) {
    return zap_crypto_rsa_sign_ex(EVP_sha512(), RSA_PKCS1_PSS_PADDING, pem, data,
                                  "crypto.rsa_pss_sha512_sign");
}
int64_t zap_crypto_rsa_pss_sha256_verify(void *pem, void *data, void *sig) {
    return zap_crypto_rsa_verify_ex(EVP_sha256(), RSA_PKCS1_PSS_PADDING, pem, data,
                                    sig, "crypto.rsa_pss_sha256_verify failed");
}
int64_t zap_crypto_rsa_pss_sha384_verify(void *pem, void *data, void *sig) {
    return zap_crypto_rsa_verify_ex(EVP_sha384(), RSA_PKCS1_PSS_PADDING, pem, data,
                                    sig, "crypto.rsa_pss_sha384_verify failed");
}
int64_t zap_crypto_rsa_pss_sha512_verify(void *pem, void *data, void *sig) {
    return zap_crypto_rsa_verify_ex(EVP_sha512(), RSA_PKCS1_PSS_PADDING, pem, data,
                                    sig, "crypto.rsa_pss_sha512_verify failed");
}

/* ==========================================================================
 * crypto: additional JOSE ECDSA curves P-384 (ES384) and P-521 (ES512)
 * (ticket #769, D-0133)
 * --------------------------------------------------------------------------
 * Same validated path and DER<->JOSE fixed-width r||s conversion as the P-256
 * code (#643); the only parameters are the curve NID, the coordinate byte width
 * (P-384: 48, P-521: 66 — note 521 bits rounds up to 66 bytes), and the digest.
 * DER is never surfaced. Import validates the point is on the curve. */
static void *zap_crypto_ec_public_key_pem(int nid, size_t coord, void *x_bytes,
                                          void *y_bytes, const char *fname) {
    ZapStr *x = (ZapStr *)x_bytes;
    ZapStr *y = (ZapStr *)y_bytes;
    if ((size_t)x->len != coord || (size_t)y->len != coord) {
        char msg[96];
        snprintf(msg, sizeof(msg), "%s: each coordinate must be exactly %zu bytes",
                 fname, coord);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_crypto_cstr(msg));
    }
    EC_KEY *ec = EC_KEY_new_by_curve_name(nid);
    BIGNUM *bx = BN_bin2bn((const unsigned char *)x->data, (int)x->len, NULL);
    BIGNUM *by = BN_bin2bn((const unsigned char *)y->data, (int)y->len, NULL);
    int on_curve = ec && bx && by &&
                   EC_KEY_set_public_key_affine_coordinates(ec, bx, by) == 1 &&
                   EC_KEY_check_key(ec) == 1;
    BN_free(bx);
    BN_free(by);
    if (!on_curve) {
        EC_KEY_free(ec);
        char msg[80];
        snprintf(msg, sizeof(msg), "%s: (x, y) is not a valid point on the curve",
                 fname);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_crypto_cstr(msg));
    }
    EVP_PKEY *pkey = EVP_PKEY_new();
    if (!pkey || EVP_PKEY_assign_EC_KEY(pkey, ec) != 1) {
        EC_KEY_free(ec);
        EVP_PKEY_free(pkey);
        char msg[64];
        snprintf(msg, sizeof(msg), "%s: key assembly failed", fname);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_crypto_cstr(msg));
    }
    return zap_crypto_pkey_to_pub_pem(pkey, fname);
}

static void *zap_crypto_ecdsa_sign_jose(int nid, size_t coord, const EVP_MD *md,
                                        void *pem, void *msg, const char *fname) {
    ZapStr *k = (ZapStr *)pem;
    ZapStr *m = (ZapStr *)msg;
    BIO *bio = BIO_new_mem_buf(k->data, (int)k->len);
    EVP_PKEY *pkey =
        bio ? PEM_read_bio_PrivateKey(bio, NULL, zap_crypto_no_pw, NULL) : NULL;
    if (bio) {
        BIO_free(bio);
    }
    if (!pkey) {
        char e[80];
        snprintf(e, sizeof(e), "%s: could not parse an EC private key from PEM",
                 fname);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_crypto_cstr(e));
    }
    if (zap_crypto_ec_curve_nid(pkey) != nid) {
        EVP_PKEY_free(pkey);
        char e[80];
        snprintf(e, sizeof(e), "%s: a private key on the matching curve is required",
                 fname);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_crypto_cstr(e));
    }
    EVP_MD_CTX *ctx = EVP_MD_CTX_new();
    unsigned char *der = NULL;
    size_t derlen = 0;
    int ok = ctx && EVP_DigestSignInit(ctx, NULL, md, NULL, pkey) == 1 &&
             EVP_DigestSign(ctx, NULL, &derlen, (const unsigned char *)m->data,
                            (size_t)m->len) == 1 &&
             (der = (unsigned char *)malloc(derlen ? derlen : 1)) != NULL &&
             EVP_DigestSign(ctx, der, &derlen, (const unsigned char *)m->data,
                            (size_t)m->len) == 1;
    EVP_MD_CTX_free(ctx);
    EVP_PKEY_free(pkey);
    unsigned char jose[132]; /* 2 * 66 (P-521) upper bound */
    const unsigned char *dp = der;
    ECDSA_SIG *es = ok ? d2i_ECDSA_SIG(NULL, &dp, (long)derlen) : NULL;
    if (es) {
        const BIGNUM *r = NULL;
        const BIGNUM *s = NULL;
        ECDSA_SIG_get0(es, &r, &s);
        if (BN_bn2binpad(r, jose, (int)coord) != (int)coord ||
            BN_bn2binpad(s, jose + coord, (int)coord) != (int)coord) {
            ok = 0;
        }
        ECDSA_SIG_free(es);
    } else {
        ok = 0;
    }
    free(der);
    if (!ok) {
        char e[64];
        snprintf(e, sizeof(e), "%s: ECDSA signing failed", fname);
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_crypto_cstr(e));
    }
    return zap_str_new((const char *)jose, (int64_t)(2 * coord));
}

static int64_t zap_crypto_ecdsa_verify_jose(int nid, size_t coord,
                                            const EVP_MD *md, void *pem, void *msg,
                                            void *sig, const char *reason) {
    ZapStr *k = (ZapStr *)pem;
    ZapStr *m = (ZapStr *)msg;
    ZapStr *s = (ZapStr *)sig;
    BIO *bio = BIO_new_mem_buf(k->data, (int)k->len);
    EVP_PKEY *pkey =
        bio ? PEM_read_bio_PUBKEY(bio, NULL, zap_crypto_no_pw, NULL) : NULL;
    if (bio) {
        BIO_free(bio);
    }
    if (!pkey) {
        return zap_crypto_rsa_verify_reject(reason);
    }
    if (zap_crypto_ec_curve_nid(pkey) != nid) {
        EVP_PKEY_free(pkey);
        return zap_crypto_rsa_verify_reject(reason);
    }
    if ((size_t)s->len != 2 * coord) {
        EVP_PKEY_free(pkey);
        return zap_crypto_rsa_verify_reject(reason);
    }
    ECDSA_SIG *es = ECDSA_SIG_new();
    BIGNUM *r = BN_bin2bn((const unsigned char *)s->data, (int)coord, NULL);
    BIGNUM *sv = BN_bin2bn((const unsigned char *)s->data + coord, (int)coord, NULL);
    unsigned char *der = NULL;
    int derlen = 0;
    int built = es && r && sv && ECDSA_SIG_set0(es, r, sv) == 1;
    if (built) {
        derlen = i2d_ECDSA_SIG(es, &der);
    } else {
        BN_free(r);
        BN_free(sv);
    }
    int64_t ok = 0;
    if (built && derlen > 0) {
        EVP_MD_CTX *ctx = EVP_MD_CTX_new();
        ok = (ctx && EVP_DigestVerifyInit(ctx, NULL, md, NULL, pkey) == 1 &&
              EVP_DigestVerify(ctx, der, (size_t)derlen,
                               (const unsigned char *)m->data, (size_t)m->len) == 1)
                 ? 1
                 : 0;
        EVP_MD_CTX_free(ctx);
    }
    OPENSSL_free(der);
    ECDSA_SIG_free(es);
    EVP_PKEY_free(pkey);
    return ok ? 1 : zap_crypto_rsa_verify_reject(reason);
}

void *zap_crypto_p384_public_key_pem(void *x_bytes, void *y_bytes) {
    return zap_crypto_ec_public_key_pem(NID_secp384r1, 48, x_bytes, y_bytes,
                                        "crypto.p384_public_key_pem");
}
void *zap_crypto_p521_public_key_pem(void *x_bytes, void *y_bytes) {
    return zap_crypto_ec_public_key_pem(NID_secp521r1, 66, x_bytes, y_bytes,
                                        "crypto.p521_public_key_pem");
}
void *zap_crypto_p384_sha384_sign(void *pem, void *msg) {
    return zap_crypto_ecdsa_sign_jose(NID_secp384r1, 48, EVP_sha384(), pem, msg,
                                      "crypto.p384_sha384_sign");
}
int64_t zap_crypto_p384_sha384_verify(void *pem, void *msg, void *sig) {
    return zap_crypto_ecdsa_verify_jose(NID_secp384r1, 48, EVP_sha384(), pem, msg,
                                        sig, "crypto.p384_sha384_verify failed");
}
void *zap_crypto_p521_sha512_sign(void *pem, void *msg) {
    return zap_crypto_ecdsa_sign_jose(NID_secp521r1, 66, EVP_sha512(), pem, msg,
                                      "crypto.p521_sha512_sign");
}
int64_t zap_crypto_p521_sha512_verify(void *pem, void *msg, void *sig) {
    return zap_crypto_ecdsa_verify_jose(NID_secp521r1, 66, EVP_sha512(), pem, msg,
                                        sig, "crypto.p521_sha512_verify failed");
}

/* OpenSSL's stock socket BIO calls write/send without MSG_NOSIGNAL. A peer
 * reset can therefore terminate the process during SSL_read's alert write or
 * best-effort SSL_shutdown, bypassing Zap's exception/resource cleanup. Keep
 * suppression socket-local: Linux uses MSG_NOSIGNAL, Darwin SO_NOSIGPIPE, and
 * Winsock has no SIGPIPE. Do not alter the application's signal disposition.
 * This BIO never owns the fd; the runtime closes it after SSL_free. */
static pthread_once_t g_tls_bio_once = PTHREAD_ONCE_INIT;
static BIO_METHOD *g_tls_bio_method;

static int zap_tls_bio_create(BIO *bio) {
    BIO_set_init(bio, 0);
    BIO_set_shutdown(bio, BIO_NOCLOSE);
    BIO_set_data(bio, NULL);
    return 1;
}

static int zap_tls_bio_destroy(BIO *bio) {
    if (!bio) {
        return 0;
    }
    BIO_set_init(bio, 0);
    BIO_set_data(bio, NULL);
    return 1;
}

static int zap_tls_bio_read(BIO *bio, char *data, int count) {
    BIO_clear_retry_flags(bio);
    if (!data || count <= 0 || !BIO_get_init(bio)) {
        return 0;
    }
    zap_fd_t fd = (zap_fd_t)(intptr_t)BIO_get_data(bio);
    for (;;) {
        int n = (int)recv(fd, data, count, 0);
        if (n >= 0) {
            return n;
        }
        int e = ZAP_SOCK_ERR();
        if (ZAP_SOCK_EINTR(e)) {
            continue;
        }
        if (ZAP_SOCK_WOULDBLOCK(e)) {
            BIO_set_retry_read(bio);
        }
        return -1;
    }
}

static int zap_tls_bio_write(BIO *bio, const char *data, int count) {
    BIO_clear_retry_flags(bio);
    if (!data || count <= 0 || !BIO_get_init(bio)) {
        return 0;
    }
    zap_fd_t fd = (zap_fd_t)(intptr_t)BIO_get_data(bio);
    for (;;) {
        int n = (int)send(fd, data, count, MSG_NOSIGNAL);
        if (n >= 0) {
            return n;
        }
        int e = ZAP_SOCK_ERR();
        if (ZAP_SOCK_EINTR(e)) {
            continue;
        }
        if (ZAP_SOCK_WOULDBLOCK(e)) {
            BIO_set_retry_write(bio);
        }
        return -1;
    }
}

static long zap_tls_bio_ctrl(BIO *bio, int command, long value, void *out) {
    (void)value;
    switch (command) {
    case BIO_CTRL_FLUSH:
    case BIO_CTRL_SET_CLOSE:
        return 1;
    case BIO_CTRL_GET_CLOSE:
        return BIO_NOCLOSE;
    case BIO_C_GET_FD: {
        int fd = BIO_get_init(bio) ? (int)(intptr_t)BIO_get_data(bio) : -1;
        if (out) {
            *(int *)out = fd;
        }
        return fd;
    }
    default:
        return 0;
    }
}

static void zap_tls_bio_init_once(void) {
    BIO_METHOD *method = BIO_meth_new(BIO_get_new_index() | BIO_TYPE_SOURCE_SINK, "Zap nonblocking socket");
    if (!method) {
        return;
    }
    if (!BIO_meth_set_create(method, zap_tls_bio_create) ||
        !BIO_meth_set_destroy(method, zap_tls_bio_destroy) ||
        !BIO_meth_set_read(method, zap_tls_bio_read) ||
        !BIO_meth_set_write(method, zap_tls_bio_write) ||
        !BIO_meth_set_ctrl(method, zap_tls_bio_ctrl)) {
        BIO_meth_free(method);
        return;
    }
    g_tls_bio_method = method; /* immutable for process lifetime */
}

static int zap_tls_set_fd(SSL *ssl, int fd) {
#if defined(ZAP_PLATFORM_POSIX) && defined(SO_NOSIGPIPE)
    int enabled = 1;
    if (setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &enabled, sizeof(enabled)) != 0) {
        return 0;
    }
#endif
    pthread_once(&g_tls_bio_once, zap_tls_bio_init_once);
    if (!g_tls_bio_method) {
        return 0;
    }
    BIO *bio = BIO_new(g_tls_bio_method);
    if (!bio) {
        return 0;
    }
    BIO_set_data(bio, (void *)(intptr_t)fd);
    BIO_set_init(bio, 1);
    SSL_set_bio(ssl, bio, bio); /* SSL owns the BIO, never the socket */
    return 1;
}

/* A TLS connection: an owned fd (+ netpoller desc) and its OpenSSL session. The
 * leading `kind` tag (ZAP_CONN_TLS) lets the HTTP server transport seam tell it
 * apart from a plaintext `ZapSock` (I#556); `read_timeout_ms` mirrors `ZapSock`
 * so `net.set_read_timeout` + `http.read_request`'s deadline apply over TLS. */
typedef struct {
    int kind; /* ZAP_CONN_TLS; shared leading tag with ZapSock (I#556). */
    int fd;
    void *pd;
    int64_t read_timeout_ms;  /* I#556: per-park TLS read deadline (0 = none). */
    int64_t read_deadline_ms; /* I#567: whole-op TLS read budget (0 = none). */
    SSL *ssl;
    SSL_CTX *ctx;
} ZapTls;

/* ATOMIC: fd is a scalar; pd/ssl/ctx are non-GC heap (netpoller + OpenSSL), so
 * nothing is traced. Freed explicitly by `tls.close_conn` (the net contract). */
static const ZapTypeDesc zap_desc_tls = {(int64_t)sizeof(ZapTls), ZAP_TD_ATOMIC, 0, NULL};
#endif /* !ZAP_NO_OPENSSL — end crypto + TLS-setup region (#853); the shared
        * connection helpers below serve BOTH transports and so stay compiled,
        * with only their TLS branches guarded. */

/* Store a read deadline on either flavor of connection handle, dispatching on the
 * transport tag (I#556). Backs `net.set_read_timeout`, which is declared above
 * the TLS section but must reach `ZapTls.read_timeout_ms` for a TLS conn. */
static void zap_conn_set_read_timeout(void *c_, int64_t ms) {
#if !defined(ZAP_NO_OPENSSL)
    if (((ZapConnHdr *)c_)->kind == ZAP_CONN_TLS) {
        ((ZapTls *)c_)->read_timeout_ms = ms;
        return;
    }
#endif
    ((ZapSock *)c_)->read_timeout_ms = ms;
}

/* Store / read the whole-operation read budget on either connection flavor
 * (I#567), dispatching on the same transport tag. Backs `net.set_read_deadline`
 * and lets `http.read_request` pull the budget for whichever transport it is. */
static void zap_conn_set_read_deadline(void *c_, int64_t ms) {
#if !defined(ZAP_NO_OPENSSL)
    if (((ZapConnHdr *)c_)->kind == ZAP_CONN_TLS) {
        ((ZapTls *)c_)->read_deadline_ms = ms;
        return;
    }
#endif
    ((ZapSock *)c_)->read_deadline_ms = ms;
}

static int64_t zap_conn_read_deadline_ms(void *c_) {
#if !defined(ZAP_NO_OPENSSL)
    if (((ZapConnHdr *)c_)->kind == ZAP_CONN_TLS) {
        return ((ZapTls *)c_)->read_deadline_ms;
    }
#endif
    return ((ZapSock *)c_)->read_deadline_ms;
}

/* Persistent absolute monotonic deadline shared by reads and writes. A zero
 * value clears it. It does not start a new budget for each chunk or operation. */
void zap_net_set_deadline_ns(void *c_, int64_t deadline_ns) {
    if (deadline_ns < 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new("net: deadline must be non-negative", sizeof("net: deadline must be non-negative") - 1));
        return;
    }
#if !defined(ZAP_NO_OPENSSL)
    void *pd_ = ((ZapConnHdr *)c_)->kind == ZAP_CONN_TLS ? ((ZapTls *)c_)->pd : ((ZapSock *)c_)->pd;
#else
    void *pd_ = ((ZapSock *)c_)->pd;
#endif
    if (!pd_) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new("net: deadline requires an open netpoller socket", sizeof("net: deadline requires an open netpoller socket") - 1));
        return;
    }
    PollDesc *pd = pd_;
    zap_np_lock(&pd->mu);
    pd->io_deadline_ns = deadline_ns;
    if (deadline_ns) {
        if (pd->rg && (!pd->r_deadline || deadline_ns < pd->r_deadline)) pd->r_deadline = deadline_ns;
        if (pd->wg && (!pd->w_deadline || deadline_ns < pd->w_deadline)) pd->w_deadline = deadline_ns;
    }
    zap_np_unlock(&pd->mu);
    np_backend_kick();
}

/* OpenSSL TLS transport + the http client/server (#853): compiled only when the
 * program reaches a TLS/crypto/http seam (the shared connection helpers above
 * stay compiled for the plaintext path; everything from here is OpenSSL-only or
 * http, and http embeds the inline `https://` path — see `program_backend_use`).
 * When `-DZAP_NO_OPENSSL`, all of this is absent and no OpenSSL symbol is left. */
#if !defined(ZAP_NO_OPENSSL)
/* Classify a non-positive SSL_* return into an action:
 *    0 -> parked and now ready; retry the SSL_* call
 *   -1 -> the descriptor was closed under us (treat as EOF)
 *   -2 -> a real protocol / certificate-verification error (caller raises)
 *   -3 -> clean peer shutdown via close_notify (treat as EOF). */
static int zap_tls_want2(SSL *ssl, int ret, int fd, void *pd, int64_t deadline_ns) {
    int e = SSL_get_error(ssl, ret);
    if (e == SSL_ERROR_ZERO_RETURN) return -3;
    if (e != SSL_ERROR_WANT_READ && e != SSL_ERROR_WANT_WRITE) return -2;
    int p = zap_sock_park2(fd, pd, e == SSL_ERROR_WANT_READ ? ZAP_POLL_READ : ZAP_POLL_WRITE, 0, deadline_ns);
    if (p == -3) return -4; /* cancellation */
    if (p == -2) return -5; /* deadline */
    return p;
}
static int zap_tls_want(SSL *ssl, int ret, int fd, void *pd) {
    return zap_tls_want2(ssl, ret, fd, pd, 0);
}

/* Raise a Zap exception for a handshake failure, including the top OpenSSL error
 * string when one is queued (e.g. "certificate verify failed"). Captures the
 * error BEFORE any SSL_free so the queue is not disturbed. */
static void zap_tls_handshake_panic(void) {
    unsigned long e = ERR_get_error();
    char msg[384];
    if (e) {
        char b[256];
        ERR_error_string_n(e, b, sizeof b);
        snprintf(msg, sizeof msg, "tls: handshake failed: %s", b);
    } else {
        snprintf(msg, sizeof msg, "tls: handshake failed");
    }
    zap_net_panic(msg);
}

/* Internal client dial WITHOUT a GC handle: TCP-connect to host:port (raises on
 * failure via zap_net_connect), then run the TLS handshake with SNI = host. When
 * `verify`, the chain is checked against the system trust store and the hostname
 * against `host`; an invalid/expired/mismatched/self-signed cert raises. On
 * success returns the SSL session and writes the owning SSL_CTX, fd and poller
 * desc through the out-params. The caller owns the lifetime (the http transport
 * holds the raw session like the plaintext path holds fd/pd; the module wraps it
 * in a `ZapTls`).
 *
 * When `ext_ctx` is non-NULL the caller supplies an already-configured client
 * `SSL_CTX` (a `tls` client context, #560) that it merely borrows (it is one of
 * the context handle's references, NOT pre-up-ref'd). This function does NOT
 * create or mutate that context: once the TCP socket is up it takes exactly one
 * SSL_CTX_up_ref for the connection, `SSL_new`s from it, and on EVERY failure
 * path from that point frees that one ref via the same SSL_CTX_free the
 * own-context path uses — so the caller's context survives for reuse with no
 * leak, and a raise from the TCP connect (before the up-ref) touches no ref at
 * all. The hostname identity check still applies at the SSL level (`verify`
 * controls `SSL_set1_host`), never mutating the shared context. When `ext_ctx`
 * is NULL the legacy behavior holds: a fresh per-dial SSL_CTX is created and
 * configured for system-root (or no) verify. */
static SSL *zap_tls_dial_raw_impl(void *host_, void *ip_, int64_t port, int verify, int64_t deadline_ns, SSL_CTX *ext_ctx, SSL_CTX **out_ctx, int *out_fd, void **out_pd) {
    const ZapStr *hostname = host_;
    char hz[256];
    if (hostname->len <= 0 || hostname->len >= (int64_t)sizeof(hz) ||
        memchr(hostname->data, '\0', (size_t)hostname->len)) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR, zap_str_new("tls: invalid hostname", sizeof("tls: invalid hostname") - 1));
        return NULL;
    }
    memcpy(hz, hostname->data, (size_t)hostname->len); hz[hostname->len] = '\0';
    /* The TCP connect can raise; do it BEFORE taking any ctx ref so an ext_ctx
     * ref is never leaked by an unwinding connect failure. */
    ZapSock *sock = ip_ ? zap_net_connect_impl(ip_, port, deadline_ns, 1) : zap_net_connect(host_, port);
    int fd = sock->fd;
    void *pd = sock->pd;

    SSL_CTX *ctx;
    if (ext_ctx) {
        SSL_CTX_up_ref(ext_ctx); /* the connection's own ref; freed on failure below
                                    or handed to the ZapTls on success. */
        ctx = ext_ctx;           /* already configured (verify mode + trust roots). */
    } else {
        ctx = SSL_CTX_new(TLS_client_method());
        if (!ctx) {
            zap_sock_shutdown(fd, pd);
            zap_net_panic("tls: SSL_CTX_new failed");
            return NULL;
        }
        (void)SSL_CTX_set_min_proto_version(ctx, TLS1_2_VERSION);
        if (verify) {
            SSL_CTX_set_default_verify_paths(ctx); /* system trust store */
            SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER, NULL);
        } else {
            SSL_CTX_set_verify(ctx, SSL_VERIFY_NONE, NULL);
        }
    }
    SSL *ssl = SSL_new(ctx);
    if (!ssl) {
        SSL_CTX_free(ctx);
        zap_sock_shutdown(fd, pd);
        zap_net_panic("tls: SSL_new failed");
        return NULL;
    }
    if (!zap_tls_set_fd(ssl, fd)) {
        SSL_free(ssl);
        SSL_CTX_free(ctx);
        zap_sock_shutdown(fd, pd);
        zap_net_panic("tls: socket BIO initialization failed");
        return NULL;
    }
    (void)SSL_set_tlsext_host_name(ssl, hz); /* SNI */
    if (verify) {
        SSL_set_hostflags(ssl, X509_CHECK_FLAG_NO_PARTIAL_WILDCARDS);
        (void)SSL_set1_host(ssl, hz); /* hostname verification */
    }
    SSL_set_connect_state(ssl);
    for (;;) {
        int status = zap_io_status(pd, deadline_ns);
        if (status) {
            SSL_free(ssl); SSL_CTX_free(ctx); zap_sock_shutdown(fd, pd);
            zap_io_panic(status, "tls: handshake failed"); return NULL;
        }
        int r = SSL_connect(ssl);
        if (r == 1) {
            break;
        }
        int w = zap_tls_want2(ssl, r, fd, pd, deadline_ns);
        if (w == 0) {
            continue; /* parked -> ready, retry */
        }
        if (w == -4 || w == -5) {
            SSL_free(ssl); SSL_CTX_free(ctx); zap_sock_shutdown(fd, pd);
            zap_io_panic(w == -4 ? -3 : -2, "tls: handshake failed"); return NULL;
        }
        /* -1 closed, -2 protocol/verification error, -3 early shutdown */
        if (w == -2) {
            char saved[384];
            unsigned long e = ERR_get_error();
            if (e) {
                char b[256];
                ERR_error_string_n(e, b, sizeof b);
                snprintf(saved, sizeof saved, "tls: handshake failed: %s", b);
            } else {
                snprintf(saved, sizeof saved, "tls: handshake failed");
            }
            SSL_free(ssl);
            SSL_CTX_free(ctx);
            zap_sock_shutdown(fd, pd);
                zap_io_panic(-1, saved);
            return NULL;
        }
        SSL_free(ssl);
        SSL_CTX_free(ctx);
        zap_sock_shutdown(fd, pd);
        zap_net_panic("tls: connection closed during handshake");
        return NULL;
    }
    *out_ctx = ctx;
    *out_fd = fd;
    *out_pd = pd;
    return ssl;
}

static SSL *zap_tls_dial_raw(void *host_, int64_t port, int verify, SSL_CTX **out_ctx, int *out_fd, void **out_pd) {
    return zap_tls_dial_raw_impl(host_, NULL, port, verify, 0, NULL, out_ctx, out_fd, out_pd);
}

/* Shared body of tls.connect / tls.connect_insecure: dial and wrap in a GC
 * handle built only after the handshake's last park. */
static void *zap_tls_dial(void *host_, int64_t port, int verify) {
    SSL_CTX *ctx = NULL;
    int fd = -1;
    void *pd = NULL;
    SSL *ssl = zap_tls_dial_raw(host_, port, verify, &ctx, &fd, &pd);
    ZapTls *t = (ZapTls *)zap_obj(&zap_desc_tls);
    t->kind = ZAP_CONN_TLS;  /* ATOMIC objects are not zeroed; init explicitly. */
    t->fd = fd;
    t->pd = pd;
    t->read_timeout_ms = 0;  /* ATOMIC objects are not zeroed. */
    t->read_deadline_ms = 0; /* ATOMIC objects are not zeroed. */
    t->ssl = ssl;
    t->ctx = ctx;
    return t;
}

/* tls.connect(host, port) -> handle. Chain + hostname verification ON. */
void *zap_tls_connect(void *host_, int64_t port) { return zap_tls_dial(host_, port, 1); }
/* Verified bounded dial: DNS/SNI identity and already-resolved numeric IP are
 * separate. The same absolute budget covers TCP connect and TLS handshake. */
void *zap_tls_connect_resolved(void *host_, void *ip_, int64_t port, int64_t deadline_ns) {
    SSL_CTX *ctx = NULL; int fd = -1; void *pd = NULL;
    SSL *ssl = zap_tls_dial_raw_impl(host_, ip_, port, 1, deadline_ns, NULL, &ctx, &fd, &pd);
    ZapTls *t = (ZapTls *)zap_obj(&zap_desc_tls);
    t->kind = ZAP_CONN_TLS; t->fd = fd; t->pd = pd;
    t->read_timeout_ms = 0; t->read_deadline_ms = 0; t->ssl = ssl; t->ctx = ctx;
    return t;
}

/* tls.connect_insecure(host, port) -> handle. Verification OFF (explicit opt-in;
 * never the default). Documented as unsafe. */
void *zap_tls_connect_insecure(void *host_, int64_t port) { return zap_tls_dial(host_, port, 0); }

/* ---- TLS client contexts (ticket #560, D-0144) ---------------------------- *
 * A per-connection, isolated TLS client configuration mirroring Python's
 * `ssl.SSLContext`. Each context owns its OWN `SSL_CTX` (and thus its own
 * `X509_STORE` of trust anchors), so two contexts loading different private CAs
 * never share trust — no process-global `SSL_CERT_FILE`/`SSL_CERT_DIR` mutation,
 * so concurrent dials with different CAs cannot bleed into one another.
 *
 *   tls.create_default_context()  (≈ ssl.create_default_context()) — PRELOADS the
 *       system trust store; verify + hostname checking ON. The "augment" path:
 *       load_verify_locations ADDS a private CA on top of the system roots.
 *   tls.SSLContext()              (≈ ssl.SSLContext(PROTOCOL_TLS_CLIENT)) — starts
 *       with an EMPTY trust store (trusts ONLY what is later loaded); verify +
 *       hostname checking still ON. The "replace" path.
 *   tls.load_verify_locations(ctx, cafile, capath) — ADDITIVELY add CA trust from
 *       a PEM bundle file and/or a hashed-cert directory (like Python; either may
 *       be "" to mean "omitted", but at least one must be given).
 *   tls.load_cert_chain(ctx, certfile, keyfile) — a client certificate(-chain)
 *       and its private key (mutual TLS). `keyfile` "" means the key is in
 *       `certfile` (Python's keyfile-defaults-to-certfile behavior).
 *   tls.connect(host, port, ctx) — dial host:port using the context (SNI +
 *       hostname identity against `host`), taking a ref on the context's SSL_CTX
 *       for the connection (released by tls.close_conn). The 2-arg
 *       tls.connect(host, port) is unchanged (fresh system-roots context).
 *
 * Hostname verification is applied per-connection (SSL_set1_host against `host`
 * in the dial path), so a context is reusable across many hosts and never has a
 * host baked into it. The SSL_CTX is reference counted exactly as the server
 * listener's is: the context handle holds one ref (released by a GC finalizer,
 * matching Python's no-explicit-close contexts) and each connection takes its
 * own, so a conn safely outlives the context. */
typedef struct {
    SSL_CTX *ctx;
} ZapTlsCtx;

/* ATOMIC: the sole field is a non-GC OpenSSL pointer. A finalizer releases the
 * context's own SSL_CTX ref when the handle becomes unreachable. */
static const ZapTypeDesc zap_desc_tls_ctx = {(int64_t)sizeof(ZapTlsCtx), ZAP_TD_ATOMIC, 0, NULL};

/* GC finalizer: drop the context's own ref (connections hold their own). */
static void zap_tls_ctx_finalize(void *obj, void *cd) {
    (void)cd;
    ZapTlsCtx *c = (ZapTlsCtx *)obj;
    if (c->ctx) {
        SSL_CTX_free(c->ctx);
        c->ctx = NULL;
    }
}

/* Build a client SSL_CTX floored at TLS 1.2 with peer verification on. When
 * `load_system`, the system trust store is preloaded (the default-context path);
 * otherwise the trust store starts empty (the bare-context path). Returns NULL
 * on OpenSSL failure, with the error queue populated for the caller's raise. */
static SSL_CTX *zap_tls_new_client_ctx(int load_system) {
    SSL_CTX *ctx = SSL_CTX_new(TLS_client_method());
    if (!ctx) {
        return NULL;
    }
    (void)SSL_CTX_set_min_proto_version(ctx, TLS1_2_VERSION);
    if (load_system && SSL_CTX_set_default_verify_paths(ctx) != 1) {
        SSL_CTX_free(ctx);
        return NULL;
    }
    SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER, NULL);
    return ctx;
}

/* Wrap a freshly built client SSL_CTX in a GC handle with a finalizer. */
static void *zap_tls_wrap_ctx(SSL_CTX *ctx) {
    ZapTlsCtx *c = (ZapTlsCtx *)zap_obj(&zap_desc_tls_ctx);
    c->ctx = ctx; /* ATOMIC objects are not zeroed; init explicitly. */
    zap_gc_register_finalizer(c, zap_tls_ctx_finalize, NULL);
    return c;
}

/* tls.create_default_context() -> ctx. System roots preloaded; verify + hostname
 * on (the "augment" base — load_verify_locations adds private CAs on top). */
void *zap_tls_create_default_context(void) {
    SSL_CTX *ctx = zap_tls_new_client_ctx(1);
    if (!ctx) {
        zap_net_panic("tls: create_default_context: SSL_CTX init failed");
        return NULL;
    }
    return zap_tls_wrap_ctx(ctx);
}

/* tls.SSLContext() -> ctx. Empty trust store (trusts only what is loaded); verify
 * + hostname still on (the "replace" base). */
void *zap_tls_ssl_context(void) {
    SSL_CTX *ctx = zap_tls_new_client_ctx(0);
    if (!ctx) {
        zap_net_panic("tls: SSLContext: SSL_CTX init failed");
        return NULL;
    }
    return zap_tls_wrap_ctx(ctx);
}

/* Raise a tls error carrying the top OpenSSL error-queue string, prefixed with
 * `what`. Used by the context loaders so a bad PEM / mismatched key is a clear,
 * catchable failure rather than a silent misconfiguration. */
static void zap_tls_ssl_panic(const char *what) {
    char msg[384];
    unsigned long e = ERR_get_error();
    if (e) {
        char b[256];
        ERR_error_string_n(e, b, sizeof b);
        snprintf(msg, sizeof msg, "%s: %s", what, b);
    } else {
        snprintf(msg, sizeof msg, "%s", what);
    }
    zap_net_panic(msg);
}

/* tls.load_verify_locations(ctx, cafile, capath). Additively add CA trust to the
 * context's store from a PEM bundle file and/or a hashed-cert directory. Either
 * argument may be "" (omitted), but at least one must be given. A nonexistent or
 * malformed path raises (with the OpenSSL reason) and leaves the context's prior
 * trust untouched. */
void zap_tls_load_verify_locations(void *ctx_, void *cafile_, void *capath_) {
    ZapTlsCtx *c = (ZapTlsCtx *)ctx_;
    const ZapStr *caf = (const ZapStr *)cafile_;
    const ZapStr *cap = (const ZapStr *)capath_;
    int have_file = caf && caf->len > 0;
    int have_path = cap && cap->len > 0;
    if (!have_file && !have_path) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("tls: load_verify_locations requires cafile or capath",
                                    sizeof("tls: load_verify_locations requires cafile or capath") - 1));
        return;
    }
    char *cafile = have_file ? zap_net_dupz(caf) : NULL;
    char *capath = have_path ? zap_net_dupz(cap) : NULL;
    if ((have_file && !cafile) || (have_path && !capath)) {
        free(cafile);
        free(capath);
        zap_net_panic("tls: out of memory");
        return;
    }
    int ok = SSL_CTX_load_verify_locations(c->ctx, cafile, capath) == 1;
    free(cafile);
    free(capath);
    if (!ok) {
        zap_tls_ssl_panic("tls: load_verify_locations failed");
    }
}

/* tls.load_cert_chain(ctx, certfile, keyfile). Load a client certificate(-chain)
 * and its private key (mutual TLS). `keyfile` "" means the key is in `certfile`.
 * A bad cert path, bad key path, or a key that does not match the certificate
 * each raise a clear, catchable error. */
void zap_tls_load_cert_chain(void *ctx_, void *certfile_, void *keyfile_) {
    ZapTlsCtx *c = (ZapTlsCtx *)ctx_;
    const ZapStr *cf = (const ZapStr *)certfile_;
    const ZapStr *kf = (const ZapStr *)keyfile_;
    if (!cf || cf->len <= 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("tls: load_cert_chain requires a certfile",
                                    sizeof("tls: load_cert_chain requires a certfile") - 1));
        return;
    }
    char *certfile = zap_net_dupz(cf);
    char *keyfile = (kf && kf->len > 0) ? zap_net_dupz(kf) : NULL;
    if (!certfile || ((kf && kf->len > 0) && !keyfile)) {
        free(certfile);
        free(keyfile);
        zap_net_panic("tls: out of memory");
        return;
    }
    const char *key_path = keyfile ? keyfile : certfile; /* Python: key defaults to cert. */
    if (SSL_CTX_use_certificate_chain_file(c->ctx, certfile) != 1) {
        free(certfile);
        free(keyfile);
        zap_tls_ssl_panic("tls: load_cert_chain: certificate load failed");
        return;
    }
    if (SSL_CTX_use_PrivateKey_file(c->ctx, key_path, SSL_FILETYPE_PEM) != 1) {
        free(certfile);
        free(keyfile);
        zap_tls_ssl_panic("tls: load_cert_chain: private key load failed");
        return;
    }
    if (SSL_CTX_check_private_key(c->ctx) != 1) {
        free(certfile);
        free(keyfile);
        zap_tls_ssl_panic("tls: load_cert_chain: key does not match certificate");
        return;
    }
    free(certfile);
    free(keyfile);
}

/* tls.connect(host, port, ctx) -> handle. Dial host:port and run the TLS
 * handshake over the supplied client context (SNI + hostname identity against
 * `host`, the context's trust roots + any client cert). The connection takes its
 * own ref on the context's SSL_CTX (balanced by tls.close_conn), so it outlives
 * the context. On any failure the borrowed ref is released and the socket torn
 * down inside the shared dial path. */
void *zap_tls_connect_ctx(void *ctx_, void *host_, int64_t port) {
    ZapTlsCtx *c = (ZapTlsCtx *)ctx_;
    if (!c->ctx) {
        zap_net_panic("tls: connect: context is not initialized");
        return NULL;
    }
    SSL_CTX *ectx = NULL;
    int fd = -1;
    void *pd = NULL;
    /* verify=1 => SSL_set1_host(host) is applied; the context carries the trust
     * roots and verify mode. The dial takes exactly one up_ref on c->ctx after
     * the socket is up and frees it on any failure. */
    SSL *ssl = zap_tls_dial_raw_impl(host_, NULL, port, 1, 0, c->ctx, &ectx, &fd, &pd);
    ZapTls *t = (ZapTls *)zap_obj(&zap_desc_tls);
    t->kind = ZAP_CONN_TLS; /* ATOMIC objects are not zeroed; init explicitly. */
    t->fd = fd;
    t->pd = pd;
    t->read_timeout_ms = 0;
    t->read_deadline_ms = 0;
    t->ssl = ssl;
    t->ctx = ectx; /* the connection's own up-ref; close_conn's SSL_CTX_free balances it. */
    return t;
}

/* Write all `len` bytes of `data` over `ssl`, parking on WANT_*. `data` is
 * caller-owned (GC-free) scratch that survives the parks. Returns 0 on success,
 * -1 on error. Shared by tls.send and the https client transport. */
static int zap_tls_write_all(SSL *ssl, int fd, void *pd, const char *data, size_t len) {
    size_t off = 0;
    int initial = zap_io_status(pd, 0);
    if (initial) return initial == -3 ? -4 : -5;
    while (off < len) {
        int status = zap_io_status(pd, 0);
        if (status) return status == -3 ? -4 : -5;
        size_t chunk = len - off;
        if (chunk > (1u << 20)) {
            chunk = 1u << 20; /* keep within SSL_write's int count */
        }
        int r = SSL_write(ssl, data + off, (int)chunk);
        if (r > 0) {
            off += (size_t)r;
            continue;
        }
        int action = zap_tls_want(ssl, r, fd, pd);
        if (action != 0) return action;
    }
    return 0;
}

/* tls.send(conn, data) -> int64 plaintext bytes written. Payload copied to
 * malloc scratch first so no GC pointer is held across the write parks. */
int64_t zap_tls_send(void *t_, void *data_) {
    ZapTls *t = (ZapTls *)t_;
    int fd = t->fd;
    void *pd = t->pd;
    SSL *ssl = t->ssl;
    int status = zap_io_status(pd, 0);
    if (status) { zap_io_panic(status, "net: send failed"); return 0; }
    const ZapStr *data = (const ZapStr *)data_;
    size_t len = (size_t)(data->len < 0 ? 0 : data->len);
    char *scratch = (char *)malloc(len + 1);
    if (!scratch) {
        zap_net_panic("tls: out of memory");
        return 0;
    }
    if (len > 0) {
        memcpy(scratch, data->data, len);
    }
    int rc = zap_tls_write_all(ssl, fd, pd, scratch, len);
    free(scratch);
    if (rc < 0) {
        zap_io_panic(rc == -4 ? -3 : rc == -5 ? -2 : -1, "tls: send failed");
        return 0;
    }
    return (int64_t)len;
}

/* tls.recv(conn, n) -> str: up to `n` decrypted bytes (one readiness-gated
 * read); an empty str on a clean peer shutdown or close. Scratch is malloc (it
 * lives across the parks); the result is copied into a GC str at the end. */
void *zap_tls_recv(void *t_, int64_t n) {
    ZapTls *t = (ZapTls *)t_;
    int fd = t->fd;
    void *pd = t->pd;
    SSL *ssl = t->ssl;
    /* Honor the per-conn read deadline on the raw TLS read path (I#566): the
     * per-park timeout (set_read_timeout) and the whole-op deadline
     * (set_read_deadline, I#567) both bound the parks here, exactly as they do for
     * plaintext `net.recv` and `http.read_request`. The WANT_* handling is inlined
     * (rather than via `zap_tls_want`, which hardcodes an unbounded park and whose
     * -2 return means a protocol error) so a timed-out park is told apart from a
     * protocol error and turned into a catchable TimeoutError. */
    int64_t timeout_ms = t->read_timeout_ms;
    int64_t whole_deadline_ns =
        (t->read_deadline_ms > 0) ? zap_deadline_after_ms(t->read_deadline_ms) : 0;
    if (n <= 0) {
        return zap_str_new("", 0);
    }
    if (n > (1 << 30)) {
        n = 1 << 30; /* SSL_read's count is an int; keep it positive */
    }
    int initial = zap_io_status(pd, whole_deadline_ns);
    if (initial) { zap_io_panic(initial, "net: recv failed"); return NULL; }
    char *buf = (char *)malloc((size_t)n + 1);
    if (!buf) {
        zap_net_panic("tls: out of memory");
        return NULL;
    }
    ssize_t got = 0;
    for (;;) {
        int status = zap_io_status(pd, whole_deadline_ns);
        if (status) { free(buf); zap_io_panic(status, "tls: recv failed"); return NULL; }
        int r = SSL_read(ssl, buf, (int)n);
        if (r > 0) {
            got = r;
            break;
        }
        int e = SSL_get_error(ssl, r);
        if (e == SSL_ERROR_WANT_READ || e == SSL_ERROR_WANT_WRITE) {
            int mode = (e == SSL_ERROR_WANT_WRITE) ? ZAP_POLL_WRITE : ZAP_POLL_READ;
            int p = zap_sock_park2(fd, pd, mode, timeout_ms, whole_deadline_ns);
            if (p == -2 || p == -3) {
                free(buf);
                zap_io_panic(p, "tls: recv failed"); /* read deadline elapsed -> TimeoutError */
                return NULL;
            }
            if (p < 0) {
                got = 0; /* closed under us -> EOF */
                break;
            }
            continue; /* parked -> ready, retry SSL_read */
        }
        if (e == SSL_ERROR_ZERO_RETURN) {
            got = 0; /* clean TLS shutdown -> EOF */
            break;
        }
        free(buf);
        zap_io_panic(-1, "tls: recv failed");
        return NULL;
    }
    void *out = zap_str_new(buf, (int64_t)got);
    free(buf);
    zap_check_cancel();
    return out;
}

/* tls.close_conn(conn). Sends a close_notify (best-effort, single pass — does
 * not park for the peer's), frees the OpenSSL session + context and the socket.
 * Idempotent: a second close is a no-op. */
void zap_tls_close(void *t_) {
    ZapTls *t = (ZapTls *)t_;
    if (t->ssl) {
        SSL_shutdown(t->ssl);
        SSL_free(t->ssl); /* BIO_NOCLOSE: does not close the fd */
        t->ssl = NULL;
    }
    if (t->ctx) {
        SSL_CTX_free(t->ctx);
        t->ctx = NULL;
    }
    if (t->fd >= 0) {
        zap_sock_shutdown(t->fd, t->pd);
        t->pd = NULL;
        t->fd = -1;
    }
}

/* ---- public TLS server (ticket #488, D-0093) ------------------------------ *
 * The server half of D-0062, generalized from the `_test_server` scaffold into a
 * caller-driven surface: `tls.listen` loads a cert+key from PEM files on disk
 * into a server SSL_CTX and binds host:port; `tls.accept` accepts a socket and
 * runs the SERVER-side handshake over the netpoller (WANT_READ/WANT_WRITE park
 * the coroutine), returning the SAME `ZapTls` handle the client produces — so
 * `tls.send`/`recv`/`close_conn` drive it directly, and `http.read_request` /
 * `http.respond` accept it through their transport seam (I#556; the handle's
 * `kind` tag selects the TLS fill/send path). Closing the listener wakes a coroutine parked in
 * `tls.accept` with CLOSED so it RAISES, exactly as `net.close_listener`
 * unblocks `net.accept` (the staged graceful-shutdown contract #488 needs).
 *
 * The shared server SSL_CTX is reference-counted: the listener holds one ref
 * (released by `tls.close_listener`) and each accepted conn takes its own via
 * SSL_CTX_up_ref (released by the existing SSL_CTX_free in `tls.close_conn`), so
 * a conn safely outlives its listener. Same unified post-1.1.0 OpenSSL API as
 * the client (TLS_server_method, SSL_CTX_set_min_proto_version) — no new crypto.
 *
 * A TLS listener handle: an owned listening fd (+ netpoller desc) and the shared
 * server SSL_CTX every accepted connection is cloned from. ATOMIC like
 * ZapSock/ZapTls (fd is a scalar; pd/ctx are non-GC infra). Freed explicitly by
 * `tls.close_listener`. */
typedef struct {
    int fd;
    void *pd;
    SSL_CTX *ctx;
} ZapTlsListener;

static const ZapTypeDesc zap_desc_tls_listener = {(int64_t)sizeof(ZapTlsListener), ZAP_TD_ATOMIC, 0,
                                                  NULL};

/* Build a server SSL_CTX from a PEM certificate(-chain) file and a PEM private
 * key file, both on disk. Floors at TLS 1.2, as the client. Returns NULL on any
 * failure (bad path, malformed PEM, key/cert mismatch); the OpenSSL error queue
 * carries the reason for the caller's raise. */
static SSL_CTX *zap_tls_make_server_ctx(const char *cert_path, const char *key_path) {
    SSL_CTX *ctx = SSL_CTX_new(TLS_server_method());
    if (!ctx) {
        return NULL;
    }
    (void)SSL_CTX_set_min_proto_version(ctx, TLS1_2_VERSION);
    if (SSL_CTX_use_certificate_chain_file(ctx, cert_path) != 1 ||
        SSL_CTX_use_PrivateKey_file(ctx, key_path, SSL_FILETYPE_PEM) != 1 ||
        SSL_CTX_check_private_key(ctx) != 1) {
        SSL_CTX_free(ctx);
        return NULL;
    }
    return ctx;
}

/* Run the SERVER-side TLS handshake on an already-accepted, non-blocking client
 * fd `cfd` (+ netpoller desc `cpd`) using `ctx` (whose ref the caller already
 * holds). Parks on WANT_READ/WANT_WRITE. On success returns the SSL session, now
 * owned by the caller; on failure returns NULL having freed its own SSL but
 * leaving cfd/cpd and the ctx ref for the caller to clean up. Shared by
 * `tls.accept` and the `_test_server` scaffold. */
static SSL *zap_tls_server_handshake(int cfd, void *cpd, SSL_CTX *ctx) {
    SSL *ssl = SSL_new(ctx);
    if (!ssl) {
        return NULL;
    }
    if (!zap_tls_set_fd(ssl, cfd)) {
        SSL_free(ssl);
        return NULL;
    }
    SSL_set_accept_state(ssl);
    for (;;) {
        int r = SSL_accept(ssl);
        if (r == 1) {
            return ssl;
        }
        if (zap_tls_want(ssl, r, cfd, cpd) != 0) {
            SSL_free(ssl); /* BIO_NOCLOSE: leaves cfd for the caller */
            return NULL;
        }
    }
}

/* tls.listen(host, port, cert_path, key_path) -> listener handle. Loads the
 * cert+key into a server SSL_CTX, then binds+listens via the shared net path
 * (SO_REUSEADDR; port 0 => OS-assigned ephemeral, read back with tls.port). */
void *zap_tls_listen(void *host_, int64_t port, void *cert_, void *key_) {
    char *cert = zap_net_dupz((const ZapStr *)cert_);
    char *key = zap_net_dupz((const ZapStr *)key_);
    if (!cert || !key) {
        free(cert);
        free(key);
        zap_net_panic("tls: out of memory");
        return NULL;
    }
    SSL_CTX *ctx = zap_tls_make_server_ctx(cert, key);
    if (!ctx) {
        char saved[384];
        unsigned long e = ERR_get_error();
        if (e) {
            char b[256];
            ERR_error_string_n(e, b, sizeof b);
            snprintf(saved, sizeof saved, "tls: listen: loading cert/key failed: %s", b);
        } else {
            snprintf(saved, sizeof saved, "tls: listen: loading cert/key failed");
        }
        free(cert);
        free(key);
        zap_net_panic(saved);
        return NULL;
    }
    free(cert);
    free(key);
    /* Bind+listen (raises on failure). Pull fd/pd out of the GC ZapSock; the
     * listener is a distinct ATOMIC handle that also owns the shared server ctx. */
    void *l_v = zap_net_listen(host_, port);
    ZapSock *s = (ZapSock *)l_v;
    int fd = s->fd;
    void *pd = s->pd;
    ZapTlsListener *l = (ZapTlsListener *)zap_obj(&zap_desc_tls_listener);
    l->fd = fd;
    l->pd = pd;
    l->ctx = ctx;
    return l;
}

/* tls.accept(listener) -> conn handle. Accepts a socket (parking on the listener
 * until a peer connects) and runs the server-side handshake over the netpoller,
 * looping past any client whose handshake fails so one bad peer never kills the
 * accept loop. Callable repeatedly — one wrapped conn per accepted socket.
 * Closing the listener wakes the parked accept with CLOSED, which RAISES (the
 * net contract). fd/pd/ctx are pulled into C locals up front (the GC listener is
 * never touched across a park), and the GC ZapTls is built only after the last
 * handshake park. */
void *zap_tls_accept(void *l_) {
    ZapTlsListener *l = (ZapTlsListener *)l_;
    zap_fd_t lfd = l->fd;
    void *lpd = l->pd;
    SSL_CTX *ctx = l->ctx;
    for (;;) {
        /* SSL_set_fd takes an int fd on every platform; a Windows socket handle
         * fits (INVALID_SOCKET truncates to -1, so the `< 0` check holds). */
        int cfd = (int)accept(lfd, NULL, NULL);
        if (cfd < 0) {
            int e = ZAP_SOCK_ERR();
            if (ZAP_SOCK_EINTR(e)) {
                continue;
            }
            if (ZAP_SOCK_WOULDBLOCK(e)) {
                int status = zap_sock_park(lfd, lpd, ZAP_POLL_READ, 0);
                if (status < 0) {
                    zap_io_panic(status, "tls: accept on a closed listener");
                    return NULL;
                }
                continue;
            }
            zap_net_panic("tls: accept failed");
            return NULL;
        }
        zap_sock_make_nonblocking(cfd);
        void *cpd = zap_netpoll_open((int64_t)cfd);
        SSL_CTX_up_ref(ctx); /* the conn's own ref, released by tls.close_conn */
        SSL *ssl = zap_tls_server_handshake(cfd, cpd, ctx);
        if (!ssl) {
            SSL_CTX_free(ctx); /* undo the up-ref for this failed conn */
            zap_sock_shutdown(cfd, cpd);
            zap_check_cancel();
            continue; /* bad handshake — skip this peer, keep accepting */
        }
        ZapTls *t = (ZapTls *)zap_obj(&zap_desc_tls);
        t->kind = ZAP_CONN_TLS; /* ATOMIC objects are not zeroed; init explicitly. */
        t->fd = cfd;
        t->pd = cpd;
        t->read_timeout_ms = 0;  /* ATOMIC objects are not zeroed. */
        t->read_deadline_ms = 0; /* ATOMIC objects are not zeroed. */
        t->ssl = ssl;
        t->ctx = ctx; /* up-ref'd above; close_conn's SSL_CTX_free balances it */
        return t;
    }
}

/* tls.port(listener) -> int64 bound local port (resolves an ephemeral 0), so a
 * `tls.listen(host, 0, ...)` can be discovered. Mirrors net.port. */
int64_t zap_tls_port(void *l_) {
    ZapTlsListener *l = (ZapTlsListener *)l_;
    struct sockaddr_storage ss;
    socklen_t sl = sizeof ss;
    if (getsockname(l->fd, (struct sockaddr *)&ss, &sl) != 0) {
        return 0;
    }
    if (ss.ss_family == AF_INET6) {
        return (int64_t)ntohs(((struct sockaddr_in6 *)&ss)->sin6_port);
    }
    return (int64_t)ntohs(((struct sockaddr_in *)&ss)->sin_port);
}

/* tls.close_listener(listener). Deregisters the listening fd from the netpoller
 * — waking any coroutine parked in tls.accept with CLOSED so it RAISES, the
 * graceful-shutdown contract (#488) — closes the fd and releases the listener's
 * server-ctx ref. Accepted conns hold their own ctx refs, so they survive.
 * Idempotent: a second close is a no-op. */
void zap_tls_close_listener(void *l_) {
    ZapTlsListener *l = (ZapTlsListener *)l_;
    if (l->fd < 0) {
        return;
    }
    zap_sock_shutdown(l->fd, l->pd);
    l->pd = NULL;
    l->fd = -1;
    if (l->ctx) {
        SSL_CTX_free(l->ctx);
        l->ctx = NULL;
    }
}

/* ---- internal self-signed loopback echo server (test scaffold) ------------ *
 * Behind `tls._test_server()`. Predates the public server above (#488); this
 * exists only so the golden suite can drive the real client end to end against a
 * server-generated self-signed cert with no outbound network. It generates an
 * ephemeral RSA key + self-signed X509 (CN=localhost), binds 127.0.0.1:0, and
 * spawns a coroutine that accepts ONE connection, handshakes, echoes one record
 * and tears down. A client that verifies (the default) rejects the self-signed
 * cert mid-handshake — the server side then just cleans up, never crashing. */
typedef struct {
    int fd;  /* listener fd */
    void *pd; /* listener netpoller desc */
    SSL_CTX *ctx;
} ZapTlsTestSrv;

/* Build a server SSL_CTX holding a fresh self-signed localhost cert, or NULL. */
static SSL_CTX *zap_tls_test_make_ctx(void) {
    EVP_PKEY *pkey = EVP_RSA_gen(2048);
    if (!pkey) {
        return NULL;
    }
    X509 *x = X509_new();
    if (!x) {
        EVP_PKEY_free(pkey);
        return NULL;
    }
    ASN1_INTEGER_set(X509_get_serialNumber(x), 1);
    X509_gmtime_adj(X509_getm_notBefore(x), 0);
    X509_gmtime_adj(X509_getm_notAfter(x), 60L * 60L * 24L * 365L);
    X509_set_pubkey(x, pkey);
    X509_NAME *nm = X509_get_subject_name(x);
    X509_NAME_add_entry_by_txt(nm, "CN", MBSTRING_ASC, (const unsigned char *)"localhost", -1, -1,
                               0);
    X509_set_issuer_name(x, nm); /* self-signed: issuer == subject */
    SSL_CTX *ctx = NULL;
    if (X509_sign(x, pkey, EVP_sha256()) && (ctx = SSL_CTX_new(TLS_server_method())) != NULL) {
        (void)SSL_CTX_set_min_proto_version(ctx, TLS1_2_VERSION);
        if (SSL_CTX_use_certificate(ctx, x) != 1 || SSL_CTX_use_PrivateKey(ctx, pkey) != 1) {
            SSL_CTX_free(ctx);
            ctx = NULL;
        }
    }
    X509_free(x);        /* SSL_CTX_use_certificate up-refs on success */
    EVP_PKEY_free(pkey); /* SSL_CTX_use_PrivateKey up-refs on success */
    return ctx;
}

/* Coroutine body: accept one conn, server-handshake, echo one record, clean up.
 * Any failure just tears down — it must never abort the process. */
static void *zap_tls_test_serve(void *arg) {
    ZapTlsTestSrv *s = (ZapTlsTestSrv *)arg;
    int lfd = s->fd;
    void *lpd = s->pd;
    SSL_CTX *ctx = s->ctx;
    int cfd = -1;
    for (;;) {
        /* OpenSSL's SSL_set_fd takes an int fd on every platform; a Windows socket
         * handle fits (INVALID_SOCKET truncates to -1, so the `< 0` check holds). */
        cfd = (int)accept(lfd, NULL, NULL);
        if (cfd >= 0) {
            break;
        }
        int e = ZAP_SOCK_ERR();
        if (ZAP_SOCK_EINTR(e)) {
            continue;
        }
        if (ZAP_SOCK_WOULDBLOCK(e)) {
            if (zap_sock_park(lfd, lpd, ZAP_POLL_READ, 0) < 0) {
                goto done;
            }
            continue;
        }
        goto done;
    }
    {
        zap_sock_make_nonblocking(cfd);
        void *cpd = zap_netpoll_open((int64_t)cfd);
        SSL *ssl = zap_tls_server_handshake(cfd, cpd, ctx); /* shared with tls.accept */
        if (ssl) {
            char rb[4096];
            for (;;) {
                int r = SSL_read(ssl, rb, (int)sizeof rb);
                if (r > 0) {
                    (void)zap_tls_write_all(ssl, cfd, cpd, rb, (size_t)r);
                    break; /* echo one record, done */
                }
                if (zap_tls_want(ssl, r, cfd, cpd) != 0) {
                    break;
                }
            }
            SSL_shutdown(ssl);
            SSL_free(ssl);
        }
        zap_sock_shutdown(cfd, cpd);
    }
done:
    SSL_CTX_free(ctx);
    zap_sock_shutdown(lfd, lpd);
    free(s);
    return NULL;
}

/* tls._test_server() -> int64 bound port. Starts the background echo server. */
int64_t zap_tls_test_server(void) {
    SSL_CTX *ctx = zap_tls_test_make_ctx();
    if (!ctx) {
        zap_net_panic("tls: test server certificate generation failed");
        return 0;
    }
    ZAP_STACK_STR(loop, "127.0.0.1", 9);
    void *l_v = zap_net_listen(loop, 0); /* raises on failure */
    ZapSock *l = (ZapSock *)l_v;
    int lfd = l->fd;
    void *lpd = l->pd;
    int64_t port = zap_net_port(l_v);
    ZapTlsTestSrv *s = (ZapTlsTestSrv *)malloc(sizeof(ZapTlsTestSrv));
    if (!s) {
        SSL_CTX_free(ctx);
        zap_sock_shutdown(lfd, lpd);
        zap_net_panic("tls: out of memory");
        return 0;
    }
    s->fd = lfd;
    s->pd = lpd;
    s->ctx = ctx;
    zap_spawn(zap_tls_test_serve, s);
    return port;
}

/* ---- internal mutual-TLS test server (test scaffold, ticket #560) --------- *
 * Behind `tls._test_mtls_server(cert_path, key_path, client_ca_path)`. A client
 * certificate is REQUIRED and verified against `client_ca_path`
 * (SSL_VERIFY_PEER | SSL_VERIFY_FAIL_IF_NO_PEER_CERT), so a client that presents
 * no certificate or one not signed by that CA fails the handshake. This exists
 * ONLY so the golden suite can exercise the public CLIENT context's
 * `load_cert_chain` (mutual TLS) end to end against a real client-auth-requiring
 * server with no outbound network — it is NOT stable public API, and #560 adds
 * no public TLS *server* client-auth surface (that is the client half only). The
 * coroutine loops past failed handshakes (so the rejected probes never kill it)
 * and serves exactly ONE successful connection — one echoed record — then tears
 * down, like `_test_server`. */
typedef struct {
    int fd;
    void *pd;
    SSL_CTX *ctx;
} ZapTlsMtlsSrv;

/* Server coroutine: accept, server-handshake (client cert required+verified);
 * skip any peer whose handshake fails, echo one record for the first that
 * succeeds, then clean up. Never aborts the process. */
static void *zap_tls_test_serve_mtls(void *arg) {
    ZapTlsMtlsSrv *s = (ZapTlsMtlsSrv *)arg;
    int lfd = s->fd;
    void *lpd = s->pd;
    SSL_CTX *ctx = s->ctx;
    for (;;) {
        int cfd = (int)accept(lfd, NULL, NULL);
        if (cfd < 0) {
            int e = ZAP_SOCK_ERR();
            if (ZAP_SOCK_EINTR(e)) {
                continue;
            }
            if (ZAP_SOCK_WOULDBLOCK(e)) {
                if (zap_sock_park(lfd, lpd, ZAP_POLL_READ, 0) < 0) {
                    break;
                }
                continue;
            }
            break;
        }
        zap_sock_make_nonblocking(cfd);
        void *cpd = zap_netpoll_open((int64_t)cfd);
        SSL *ssl = zap_tls_server_handshake(cfd, cpd, ctx); /* shared with tls.accept */
        if (!ssl) {
            zap_sock_shutdown(cfd, cpd);
            zap_check_cancel();
            continue; /* rejected client (no/invalid cert) — keep serving */
        }
        char rb[4096];
        for (;;) {
            int r = SSL_read(ssl, rb, (int)sizeof rb);
            if (r > 0) {
                (void)zap_tls_write_all(ssl, cfd, cpd, rb, (size_t)r);
                break; /* echo one record, done */
            }
            if (zap_tls_want(ssl, r, cfd, cpd) != 0) {
                break;
            }
        }
        SSL_shutdown(ssl);
        SSL_free(ssl);
        zap_sock_shutdown(cfd, cpd);
        break; /* served one successful connection */
    }
    SSL_CTX_free(ctx);
    zap_sock_shutdown(lfd, lpd);
    free(s);
    return NULL;
}

/* tls._test_mtls_server(cert_path, key_path, client_ca_path) -> int64 port. */
int64_t zap_tls_test_mtls_server(void *cert_, void *key_, void *client_ca_) {
    char *cert = zap_net_dupz((const ZapStr *)cert_);
    char *key = zap_net_dupz((const ZapStr *)key_);
    char *cca = zap_net_dupz((const ZapStr *)client_ca_);
    if (!cert || !key || !cca) {
        free(cert);
        free(key);
        free(cca);
        zap_net_panic("tls: out of memory");
        return 0;
    }
    SSL_CTX *ctx = zap_tls_make_server_ctx(cert, key);
    if (ctx) {
        /* Require + verify the client certificate against the given CA bundle. */
        if (SSL_CTX_load_verify_locations(ctx, cca, NULL) != 1) {
            SSL_CTX_free(ctx);
            ctx = NULL;
        } else {
            SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER | SSL_VERIFY_FAIL_IF_NO_PEER_CERT, NULL);
        }
    }
    free(cert);
    free(key);
    free(cca);
    if (!ctx) {
        zap_tls_ssl_panic("tls: _test_mtls_server: context setup failed");
        return 0;
    }
    ZAP_STACK_STR(loop, "127.0.0.1", 9);
    void *l_v = zap_net_listen(loop, 0); /* raises on failure */
    ZapSock *l = (ZapSock *)l_v;
    int lfd = l->fd;
    void *lpd = l->pd;
    int64_t port = zap_net_port(l_v);
    ZapTlsMtlsSrv *s = (ZapTlsMtlsSrv *)malloc(sizeof(ZapTlsMtlsSrv));
    if (!s) {
        SSL_CTX_free(ctx);
        zap_sock_shutdown(lfd, lpd);
        zap_net_panic("tls: out of memory");
        return 0;
    }
    s->fd = lfd;
    s->pd = lpd;
    s->ctx = ctx;
    zap_spawn(zap_tls_test_serve_mtls, s);
    return port;
}

/* ---- HTTP/1.1 ------------------------------------------------------------- *
 * A growable malloc byte buffer used to accumulate a message off a socket. It is
 * NUL-terminated after every append (one spare byte) so the textual header
 * scan can use memmem/strstr safely; the body may still contain NULs (length is
 * tracked separately). Never GC — it lives across recv parks. */
typedef struct {
    char *buf;
    size_t len;
    size_t cap;
} ZapDynBuf;

static int zap_dyn_reserve(ZapDynBuf *b, size_t extra) {
    if (b->len + extra + 1 <= b->cap) {
        return 0;
    }
    size_t cap = b->cap ? b->cap : 256;
    while (b->len + extra + 1 > cap) {
        cap *= 2;
    }
    char *nb = (char *)realloc(b->buf, cap);
    if (!nb) {
        return -1;
    }
    b->buf = nb;
    b->cap = cap;
    return 0;
}

static void zap_dyn_free(ZapDynBuf *b) {
    free(b->buf);
    b->buf = NULL;
    b->len = b->cap = 0;
}

/* Read one chunk from the socket into `b`. Returns bytes read (0 = EOF), -1 on
 * error, or -2 when a read deadline elapses — a sentinel the caller turns into a
 * catchable TimeoutError rather than a silent EOF. `timeout_ms` is the per-park
 * deadline (I#485); `whole_deadline_ns` is the absolute whole-operation deadline
 * (I#567, computed once by the caller, 0 = none) — each park waits the nearer. */
static ssize_t zap_dyn_fill(ZapDynBuf *b, int fd, void *pd, int64_t timeout_ms,
                            int64_t whole_deadline_ns) {
    if (zap_dyn_reserve(b, 4096) != 0) {
        return -1;
    }
    for (;;) {
        int status = zap_io_status(pd, whole_deadline_ns);
        if (status) return status;
        ssize_t r = recv(fd, b->buf + b->len, b->cap - b->len - 1, 0);
        if (r > 0) {
            b->len += (size_t)r;
            b->buf[b->len] = '\0';
            return r;
        }
        if (r == 0) {
            b->buf[b->len] = '\0';
            return 0;
        }
        if (errno == EINTR) {
            continue;
        }
        if (errno == EAGAIN || errno == EWOULDBLOCK) {
            int p = zap_sock_park2(fd, pd, ZAP_POLL_READ, timeout_ms, whole_deadline_ns);
            if (p == -2 || p == -3) return p;
            if (p < 0) {
                return 0; /* closed: treat as EOF */
            }
            continue;
        }
        return -1;
    }
}

/* Case-insensitive lookup of header `name` in the raw header block [hbuf,hlen)
 * (the bytes between the request/status line and the blank line). Writes the
 * trimmed value into a fresh GC str via *out (empty str when absent). The header
 * block is malloc scratch, so this is called before any further park. */
static void *zap_http_header_value(const char *hbuf, size_t hlen, const char *name) {
    size_t nlen = strlen(name);
    size_t i = 0;
    while (i < hlen) {
        /* start of a header line */
        size_t line = i;
        while (i < hlen && hbuf[i] != '\n') {
            i++;
        }
        size_t end = i; /* points at '\n' or hlen */
        if (end > line && hbuf[end - 1] == '\r') {
            end--;
        }
        /* does this line start with "name:" (case-insensitive)? */
        if (end - line > nlen && hbuf[line + nlen] == ':') {
            int match = 1;
            for (size_t k = 0; k < nlen; k++) {
                char a = hbuf[line + k];
                char b = name[k];
                if (a >= 'A' && a <= 'Z') {
                    a = (char)(a - 'A' + 'a');
                }
                if (b >= 'A' && b <= 'Z') {
                    b = (char)(b - 'A' + 'a');
                }
                if (a != b) {
                    match = 0;
                    break;
                }
            }
            if (match) {
                size_t v = line + nlen + 1;
                while (v < end && (hbuf[v] == ' ' || hbuf[v] == '\t')) {
                    v++;
                }
                return zap_str_new(hbuf + v, (int64_t)(end - v));
            }
        }
        i++; /* skip the '\n' */
    }
    return zap_str_new("", 0);
}

/* Content-Length from a header block, or -1 if absent/unparseable. */
static long zap_http_content_length(const char *hbuf, size_t hlen) {
    void *v = zap_http_header_value(hbuf, hlen, "Content-Length");
    ZapStr *s = (ZapStr *)v;
    if (s->len <= 0) {
        return -1;
    }
    long n = 0;
    int any = 0;
    for (int64_t k = 0; k < s->len; k++) {
        char ch = s->data[k];
        if (ch < '0' || ch > '9') {
            break;
        }
        n = n * 10 + (ch - '0');
        any = 1;
    }
    return any ? n : -1;
}

/* ---- HTTP response handle (client) ---------------------------------------- */
typedef struct {
    int64_t status;
    const ZapStr *headers; /* raw header block */
    const ZapStr *body;
    const ZapStr *final_url; /* URL the final response came from (#987); after any
                              * redirects this is the last URL dialed, else the
                              * original request URL. */
    void *chain;             /* list[str] of redirect targets followed in order
                              * (#987); the empty list when nothing was followed.
                              * A ZapList; typed void* so the GC desc tracks the
                              * slot as a plain pointer. */
} ZapHttpResp;

static const int32_t zap_off_httpresp[] = {
    (int32_t)offsetof(ZapHttpResp, headers),
    (int32_t)offsetof(ZapHttpResp, body),
    (int32_t)offsetof(ZapHttpResp, final_url),
    (int32_t)offsetof(ZapHttpResp, chain),
};
static const ZapTypeDesc zap_desc_httpresp = {(int64_t)sizeof(ZapHttpResp), ZAP_TD_PRECISE, 4,
                                              zap_off_httpresp};

/* ---- HTTP request handle (server) ----------------------------------------- */
typedef struct {
    const ZapStr *method;
    const ZapStr *path;
    const ZapStr *headers; /* raw header block */
    const ZapStr *body;
    const ZapStr *version; /* request-line HTTP version token, e.g. "HTTP/1.1" */
} ZapHttpReq;

static const int32_t zap_off_httpreq[] = {
    (int32_t)offsetof(ZapHttpReq, method),  (int32_t)offsetof(ZapHttpReq, path),
    (int32_t)offsetof(ZapHttpReq, headers), (int32_t)offsetof(ZapHttpReq, body),
    (int32_t)offsetof(ZapHttpReq, version),
};
static const ZapTypeDesc zap_desc_httpreq = {(int64_t)sizeof(ZapHttpReq), ZAP_TD_PRECISE, 5,
                                             zap_off_httpreq};

/* Split a message in `b` at the blank line into header-block length and body
 * offset. Returns 0 and sets *hlen (bytes up to, excluding, the blank line) and
 * *body_off (first body byte) on success; -1 if no header terminator is present. */
static int zap_http_split(const ZapDynBuf *b, size_t *hlen, size_t *body_off) {
    const char *p = zap_memfind(b->buf, b->len, "\r\n\r\n", 4);
    size_t sep = 4;
    if (!p) {
        p = zap_memfind(b->buf, b->len, "\n\n", 2);
        sep = 2;
    }
    if (!p) {
        return -1;
    }
    *hlen = (size_t)(p - b->buf);
    *body_off = *hlen + sep;
    return 0;
}

/* ---- client transport (plaintext or TLS) ---------------------------------- *
 * The client is scheme-agnostic: `http://` dials via `net` and `https://` via
 * `tls` (ticket #147, default port 443, certificate + hostname verification ON).
 * A `ZapHttpConn` holds the raw transport (fd/pd, plus the OpenSSL session when
 * TLS) so the request/response code below is written once over these helpers.
 * The TLS session is held raw — never a GC `ZapTls` handle — so nothing GC is
 * carried across the recv parks, exactly as the plaintext path holds fd/pd. */
typedef struct {
    int tls;
    int fd;
    void *pd;
    SSL *ssl;     /* NULL unless tls */
    SSL_CTX *ctx; /* NULL unless tls */
    int64_t deadline_ns; /* absolute monotonic whole-request deadline (#986), 0 =
                          * unbounded (today's behavior). Bounds every read park
                          * on this connection via the fill seam below. */
} ZapHttpConn;

/* Read one chunk over an established SSL into `b`. Returns bytes read (0 = clean
 * EOF / closed), -1 on error, or -2 when a `timeout_ms > 0` read deadline elapses
 * (I#485 over TLS, I#556) — the sentinel the caller turns into a catchable
 * TimeoutError. The TLS twin of `zap_dyn_fill`. `timeout_ms` is the per-park
 * deadline (0 = park forever — the client always passes 0/0, so its behavior is
 * unchanged); `whole_deadline_ns` is the absolute whole-operation deadline (I#567,
 * computed once by the caller, 0 = none). The server read path threads the
 * connection's `net.set_read_timeout` / `net.set_read_deadline` through. */
static ssize_t zap_http_tls_fill(ZapDynBuf *b, SSL *ssl, int fd, void *pd, int64_t timeout_ms,
                                 int64_t whole_deadline_ns) {
    if (zap_dyn_reserve(b, 4096) != 0) {
        return -1;
    }
    size_t avail = b->cap - b->len - 1;
    if (avail > (1u << 20)) {
        avail = 1u << 20; /* SSL_read's count is an int; bound it */
    }
    for (;;) {
        int status = zap_io_status(pd, whole_deadline_ns);
        if (status) return status;
        int r = SSL_read(ssl, b->buf + b->len, (int)avail);
        if (r > 0) {
            b->len += (size_t)r;
            b->buf[b->len] = '\0';
            return r;
        }
        /* Classify the non-positive return. WANT_READ/WANT_WRITE park (bounded by
         * the per-park timeout AND the whole-op deadline); a read-deadline timeout
         * is the -2 sentinel; a closed fd or a clean close_notify is EOF; anything
         * else is a hard error. This mirrors `zap_tls_want` but threads the
         * deadlines so the park can time out (that helper hardcodes an unbounded
         * park). */
        int e = SSL_get_error(ssl, r);
        if (e == SSL_ERROR_WANT_READ || e == SSL_ERROR_WANT_WRITE) {
            int mode = (e == SSL_ERROR_WANT_WRITE) ? ZAP_POLL_WRITE : ZAP_POLL_READ;
            int p = zap_sock_park2(fd, pd, mode, timeout_ms, whole_deadline_ns);
            if (p == -2 || p == -3) {
                b->buf[b->len] = '\0';
                return p;
            }
            if (p < 0) {
                b->buf[b->len] = '\0';
                return 0; /* closed under us -> EOF */
            }
            continue; /* parked -> ready, retry SSL_read */
        }
        b->buf[b->len] = '\0';
        return (e == SSL_ERROR_ZERO_RETURN) ? 0 : -1; /* clean shutdown -> EOF */
    }
}

/* Send all `len` bytes over the connection. Returns 0 on success, -1 on error. */
static int zap_httpconn_send_all(ZapHttpConn *c, const char *data, size_t len) {
    if (c->tls) {
        int status = zap_tls_write_all(c->ssl, c->fd, c->pd, data, len);
        return status == -4 ? -3 : status == -5 ? -2 : status < 0 ? -1 : 0;
    }
    ssize_t status = zap_sock_send_all(c->fd, c->pd, data, len);
    return status < 0 ? (int)status : 0;
}

/* Read one chunk into `b`. Returns bytes read (0 = EOF), -1 on error, or -2 when
 * the whole-request deadline (#986) elapses — the sentinel the caller turns into
 * a catchable TimeoutError. The per-park timeout stays 0 (the client has no
 * between-byte read-timeout surface); `c->deadline_ns` is the absolute
 * whole-request deadline shared by connect + every read across redirect hops
 * (0 = unbounded, the pre-#986 behavior). */
static ssize_t zap_httpconn_fill(ZapHttpConn *c, ZapDynBuf *b) {
    if (c->tls) {
        return zap_http_tls_fill(b, c->ssl, c->fd, c->pd, 0, c->deadline_ns);
    }
    return zap_dyn_fill(b, c->fd, c->pd, 0, c->deadline_ns);
}

/* Shut down and close the connection (the TLS session + context when present). */
static void zap_httpconn_close(ZapHttpConn *c) {
    if (c->tls && c->ssl) {
        SSL_shutdown(c->ssl);
        SSL_free(c->ssl);
        c->ssl = NULL;
    }
    if (c->tls && c->ctx) {
        SSL_CTX_free(c->ctx);
        c->ctx = NULL;
    }
    zap_sock_shutdown(c->fd, c->pd);
    c->fd = -1;
    c->pd = NULL;
}

/* ---- server transport seam (I#556) ---------------------------------------- *
 * `http.read_request` and `http.respond` take a connection as the opaque `bytes`
 * handle, which is EITHER a plaintext `ZapSock` (net.accept) or a TLS `ZapTls`
 * (tls.accept). These two helpers are the only transport-aware code in the
 * server request/response path: they dispatch on the handle's leading `kind` tag
 * so the ONE request parser and the ONE response writer below serve both
 * transports — plaintext through recv()/send() + the netpoller park, TLS through
 * SSL_read/SSL_write + the WANT_READ/WANT_WRITE park. The per-connection read
 * deadline (net.set_read_timeout, I#485) is read from whichever handle it is and
 * threaded into the fill, so a read timeout fires uniformly over TLS too; the
 * whole-operation deadline (net.set_read_deadline, I#567) is computed once by the
 * caller and passed in, so it bounds a whole multi-chunk read over both. */

/* Read one chunk from a server connection into `b`. Returns bytes read (0 = EOF),
 * -1 on error, -2 when a read deadline elapses (caller raises TimeoutError).
 * `whole_deadline_ns` is the absolute whole-op deadline (0 = none), fixed for the
 * duration of one `http.read_request`; the per-park timeout is read per-handle. */
static ssize_t zap_http_srv_fill(void *c_, ZapDynBuf *b, int64_t whole_deadline_ns) {
    if (((ZapConnHdr *)c_)->kind == ZAP_CONN_TLS) {
        ZapTls *t = (ZapTls *)c_;
        return zap_http_tls_fill(b, t->ssl, t->fd, t->pd, t->read_timeout_ms, whole_deadline_ns);
    }
    ZapSock *s = (ZapSock *)c_;
    return zap_dyn_fill(b, s->fd, s->pd, s->read_timeout_ms, whole_deadline_ns);
}

/* Write all `len` bytes over a server connection. Returns 0 on success, -1 on
 * error. The response-write twin of `zap_http_srv_fill`. */
static int zap_http_srv_send_all(void *c_, const char *data, size_t len) {
    if (((ZapConnHdr *)c_)->kind == ZAP_CONN_TLS) {
        ZapTls *t = (ZapTls *)c_;
        int status = zap_tls_write_all(t->ssl, t->fd, t->pd, data, len);
        return status == -4 ? -3 : status == -5 ? -2 : status < 0 ? -1 : 0;
    }
    ZapSock *s = (ZapSock *)c_;
    ssize_t status = zap_sock_send_all(s->fd, s->pd, data, len);
    return status < 0 ? (int)status : 0;
}

/* http.request(method, url, body) -> response handle. Opens a connection (plain
 * `http://` via net, or `https://` via TLS with verification on — ticket #147),
 * sends a Connection: close request, reads the whole response to EOF, parses it. */
/* Case-insensitive compare of a ZapStr header name against a NUL-terminated
 * ASCII name. Used to spot transport-managed / defaulted headers in a
 * caller-supplied header map so we don't emit duplicates. */
static int zap_http_name_eq(const ZapStr *name, const char *lit) {
    size_t llen = strlen(lit);
    if (name->len != (int64_t)llen) {
        return 0;
    }
    for (size_t k = 0; k < llen; k++) {
        char a = name->data[k];
        char b = lit[k];
        if (a >= 'A' && a <= 'Z') {
            a = (char)(a - 'A' + 'a');
        }
        if (b >= 'A' && b <= 'Z') {
            b = (char)(b - 'A' + 'a');
        }
        if (a != b) {
            return 0;
        }
    }
    return 1;
}

/* Is `name` a header the transport computes itself (Host / Content-Length /
 * Connection)? Such a header from a caller-supplied map is dropped so the wire
 * request carries exactly one. Content-Type is NOT here: callers may override
 * the default (that is the whole point of #280 for JSON/SigV4 clients). */
static int zap_http_hdr_managed(const ZapStr *name) {
    return zap_http_name_eq(name, "Host") || zap_http_name_eq(name, "Content-Length") ||
           zap_http_name_eq(name, "Connection");
}

/* HTTP/1.1 client core (ticket #104; request headers #280). One request/response
 * round-trip: `zap_http_do` (below) layers redirect-following over this. `url_cstr`
 * is a NUL-terminated absolute URL owned by the caller (never freed here).
 * `headers_` is an optional `dict[str,str]` (NULL for the no-header forms): each
 * live entry is emitted as a `Name: Value` header line, except the
 * transport-managed ones (zap_http_hdr_managed). A caller-supplied Content-Type
 * suppresses the default text/plain, unless `suppress_ctype` is set — the
 * redirect path sets it when a method-downgrading redirect has dropped the body,
 * so a stale Content-Type is not carried onto the follow-up GET. Keys and values
 * are copied into the malloc request buffer before any send, so the GC dict is
 * only touched while it is still rooted by the caller's frame. */
static void *zap_http_do_once(const ZapStr *method, const char *url_cstr,
                              const ZapStr *body, const ZapDict *hdrs,
                              int suppress_ctype, int64_t deadline_ns) {
    char *mz = zap_net_dupz(method);
    if (!mz) {
        zap_net_panic("http: out of memory");
        return NULL;
    }
    int is_tls = 0;
    const char *rest = url_cstr;
    if (strncmp(url_cstr, "https://", 8) == 0) {
        is_tls = 1;
        rest = url_cstr + 8;
    } else if (strncmp(url_cstr, "http://", 7) == 0) {
        rest = url_cstr + 7;
    }
    /* host[:port][/path] */
    size_t hostend = 0;
    while (rest[hostend] && rest[hostend] != '/' && rest[hostend] != ':') {
        hostend++;
    }
    char *host = (char *)malloc(hostend + 1);
    if (!host) {
        free(mz);
        zap_net_panic("http: out of memory");
        return NULL;
    }
    memcpy(host, rest, hostend);
    host[hostend] = '\0';
    long port = is_tls ? 443 : 80;
    const char *after = rest + hostend;
    if (*after == ':') {
        after++;
        port = 0;
        while (*after >= '0' && *after <= '9') {
            port = port * 10 + (*after - '0');
            after++;
        }
    }
    const char *path = (*after == '/') ? after : "/";

    /* Dial the transport: plaintext via net, or TLS via tls (verification on).
     * Both take a ZapStr* host; the raw transport (fd/pd, and the OpenSSL session
     * for TLS) is held in C locals so nothing GC is carried across the parks.
     * `hostz` is a one-object string (#837) consumed synchronously by the dial
     * (the host bytes are copied into addrinfo / the SNI name before the first
     * park), and `host` is freed right after — so it never lives across a park. */
    ZapStr *hostz = (ZapStr *)zap_str_new(host, (int64_t)strlen(host));
    ZapHttpConn hc;
    memset(&hc, 0, sizeof hc);
    hc.deadline_ns = deadline_ns; /* bounds connect + every read (#986) */
    if (is_tls) {
        SSL_CTX *tctx = NULL;
        int tfd = -1;
        void *tpd = NULL;
        /* Bound the TLS dial (TCP connect + handshake parks) by the same absolute
         * deadline (#986); raises TimeoutError on expiry, as on plain connect. */
        SSL *ssl = zap_tls_dial_raw_impl(hostz, NULL, port, 1, deadline_ns, NULL, &tctx, &tfd,
                                         &tpd); /* raises on failure */
        hc.tls = 1;
        hc.ssl = ssl;
        hc.ctx = tctx;
        hc.fd = tfd;
        hc.pd = tpd;
    } else {
        /* Bound the plaintext connect by the deadline (#986); `numeric = 0` keeps
         * hostname resolution (getaddrinfo is not itself deadline-bounded, but the
         * connect parks are). Raises TimeoutError on expiry. */
        ZapSock *conn =
            (ZapSock *)zap_net_connect_impl((void *)hostz, port, deadline_ns, 0); /* raises */
        hc.fd = conn->fd;
        hc.pd = conn->pd;
    }
    free(host);

    size_t blen = (size_t)(body->len < 0 ? 0 : body->len);
    /* Size the request buffer: request/Host/Connection line + body + the
     * caller's header map (name + ": " + value + CRLF for each live entry). */
    size_t hdrbytes = 0;
    int caller_content_type = 0;
    if (hdrs) {
        for (int64_t e = 0; e < hdrs->cap; e++) {
            if (zap_d_tags(hdrs)[e] < 2) { /* skip empty / tombstone slots (#840) */
                continue;
            }
            const ZapStr *name = (const ZapStr *)(intptr_t)zap_d_keys(hdrs)[e];
            const ZapStr *val = (const ZapStr *)(intptr_t)zap_d_vals(hdrs)[e];
            if (zap_http_hdr_managed(name)) {
                continue;
            }
            if (zap_http_name_eq(name, "Content-Type")) {
                caller_content_type = 1;
            }
            size_t nl = (size_t)(name->len < 0 ? 0 : name->len);
            size_t vl = (size_t)(val->len < 0 ? 0 : val->len);
            hdrbytes += nl + vl + 4; /* ": " + "\r\n" */
        }
    }
    size_t reqcap = strlen(mz) + strlen(path) + hostend + blen + hdrbytes + 256;
    char *req = (char *)malloc(reqcap);
    if (!req) {
        free(mz);
        zap_httpconn_close(&hc);
        zap_net_panic("http: out of memory");
        return NULL;
    }
    int hdr = snprintf(req, reqcap,
                       "%s %s HTTP/1.1\r\nHost: %.*s\r\nConnection: close\r\n",
                       mz, path, (int)hostend, rest);
    if (blen > 0) {
        hdr += snprintf(req + hdr, reqcap - (size_t)hdr, "Content-Length: %zu\r\n", blen);
        if (!caller_content_type) {
            hdr += snprintf(req + hdr, reqcap - (size_t)hdr,
                            "Content-Type: text/plain; charset=utf-8\r\n");
        }
    }
    /* Caller-supplied headers, copied out of the GC dict into the buffer. */
    if (hdrs) {
        for (int64_t e = 0; e < hdrs->cap; e++) {
            if (zap_d_tags(hdrs)[e] < 2) { /* skip empty / tombstone slots (#840) */
                continue;
            }
            const ZapStr *name = (const ZapStr *)(intptr_t)zap_d_keys(hdrs)[e];
            const ZapStr *val = (const ZapStr *)(intptr_t)zap_d_vals(hdrs)[e];
            if (zap_http_hdr_managed(name)) {
                continue;
            }
            if (suppress_ctype && zap_http_name_eq(name, "Content-Type")) {
                /* a redirect downgraded the method and dropped the body — don't
                 * carry the original Content-Type onto the bodyless follow-up. */
                continue;
            }
            size_t nl = (size_t)(name->len < 0 ? 0 : name->len);
            size_t vl = (size_t)(val->len < 0 ? 0 : val->len);
            hdr += snprintf(req + hdr, reqcap - (size_t)hdr, "%.*s: %.*s\r\n", (int)nl,
                            name->data, (int)vl, val->data);
        }
    }
    hdr += snprintf(req + hdr, reqcap - (size_t)hdr, "\r\n");
    if (blen > 0) {
        memcpy(req + hdr, body->data, blen);
        hdr += (int)blen;
    }
    free(mz);

    int sent = zap_httpconn_send_all(&hc, req, (size_t)hdr);
    free(req);
    if (sent < 0) {
        zap_httpconn_close(&hc);
        zap_io_panic(sent, "http: send failed");
        return NULL;
    }

    /* Read the entire response (server closes after it, Connection: close). */
    ZapDynBuf b = {NULL, 0, 0};
    for (;;) {
        ssize_t r = zap_httpconn_fill(&hc, &b);
        if (r == -2 || r == -3) {
            /* The whole-request deadline elapsed (#986, -2) or the coroutine was
             * cancelled (-3): close the transport, free the buffer, and raise
             * (TimeoutError / cancelled) rather than return a truncated response. */
            zap_httpconn_close(&hc);
            zap_dyn_free(&b);
            zap_io_panic((int)r, "http: read timed out");
            return NULL;
        }
        if (r <= 0) {
            break;
        }
    }
    zap_httpconn_close(&hc);
    if (g_is_cancelled()) { zap_dyn_free(&b); zap_raise(cancelled_exc()); return NULL; }

    size_t hlen = 0, body_off = b.len;
    long status = 0;
    if (b.len > 0 && zap_http_split(&b, &hlen, &body_off) == 0) {
        /* status line: "HTTP/1.1 200 ..." — find the first space, then 3 digits */
        size_t sp = 0;
        while (sp < hlen && b.buf[sp] != ' ') {
            sp++;
        }
        sp++;
        while (sp < hlen && b.buf[sp] >= '0' && b.buf[sp] <= '9') {
            status = status * 10 + (b.buf[sp] - '0');
            sp++;
        }
    }
    /* status line occupies the first line of the header block; the header block
     * for lookups starts after it. Find the end of the first line. */
    size_t hstart = 0;
    while (hstart < hlen && b.buf[hstart] != '\n') {
        hstart++;
    }
    if (hstart < hlen) {
        hstart++; /* past the '\n' */
    }

    /* Build GC results at the end (no park after this point). */
    void *headers = zap_str_new(b.buf + hstart, (int64_t)(hlen > hstart ? hlen - hstart : 0));
    void *bodyobj = zap_str_new(b.buf + body_off, (int64_t)(b.len > body_off ? b.len - body_off : 0));
    zap_dyn_free(&b);

    ZapHttpResp *resp = (ZapHttpResp *)zap_obj(&zap_desc_httpresp);
    resp->status = (int64_t)status;
    resp->headers = (const ZapStr *)headers;
    resp->body = (const ZapStr *)bodyobj;
    resp->final_url = NULL; /* filled by the zap_http_do redirect wrapper */
    resp->chain = NULL;
    return resp;
}

/* ---- redirect following (#987, D-0222) ------------------------------------ *
 * `zap_http_do` wraps the single-shot core in a redirect loop: it follows 3xx
 * responses that carry a `Location`, resolving each (possibly relative) target
 * against the current URL with `zap_url_join` (the same RFC-3986 resolution as
 * `url.join`), up to ZAP_HTTP_MAX_REDIRECTS hops. Method/body across a hop follow
 * the browser/`requests` rules: 303 -> GET (body dropped); 301/302 downgrade only
 * a POST to GET (body dropped), other methods preserved; 307/308 preserve method
 * and body. The chain of resolved targets and the final URL are recorded on the
 * response (`http.redirects` / `http.final_url`).
 *
 * GC: every URL followed is accumulated in malloc'd C strings, never a GC object
 * held across a network park — the GC `chain` list and `final_url` are built only
 * at the very end, after the last response, where no further park occurs. */
#define ZAP_HTTP_MAX_REDIRECTS 10

/* malloc NUL-terminated copy of a C string (chain bookkeeping); NULL on OOM. */
static char *zap_http_cstr_dup(const char *s) {
    size_t n = strlen(s);
    char *o = (char *)malloc(n + 1);
    if (o) {
        memcpy(o, s, n + 1);
    }
    return o;
}

/* Full case-insensitive compare of a NUL-terminated method against a literal. */
static int zap_http_method_is(const char *m, const char *lit) {
    size_t i = 0;
    for (;;) {
        char a = m[i];
        char b = lit[i];
        if (a >= 'A' && a <= 'Z') {
            a = (char)(a - 'A' + 'a');
        }
        if (b >= 'A' && b <= 'Z') {
            b = (char)(b - 'A' + 'a');
        }
        if (a != b) {
            return 0;
        }
        if (!a) {
            return 1;
        }
        i++;
    }
}

/* Is `status` a redirect this client follows? */
static int zap_http_is_redirect(long status) {
    return status == 301 || status == 302 || status == 303 || status == 307 ||
           status == 308;
}

/* malloc'd NUL-terminated copy of response header `name`, or NULL when absent or
 * empty. The transient GC string from zap_http_header_value is copied out before
 * any park, so nothing GC is carried into the next request. */
static char *zap_http_resp_header_cstr(ZapHttpResp *resp, const char *name) {
    const ZapStr *s = (const ZapStr *)zap_http_header_value(
        resp->headers->data, (size_t)resp->headers->len, name);
    if (!s || s->len <= 0) {
        return NULL;
    }
    char *out = (char *)malloc((size_t)s->len + 1);
    if (!out) {
        return NULL;
    }
    memcpy(out, s->data, (size_t)s->len);
    out[s->len] = '\0';
    return out;
}

/* HTTP/1.1 client with redirect following. See the block comment above.
 * `timeout_ms` is the per-request TOTAL deadline (#986): 0 = unbounded (the
 * pre-#986 behavior), a positive value bounds connect + every read across ALL
 * redirect hops by one absolute monotonic deadline computed here once, and a
 * negative value is a ValueError (mirrors `net.set_read_timeout`). On expiry any
 * blocked connect/read raises a catchable TimeoutError. */
void *zap_http_do(void *method_, void *url_, void *body_, void *headers_, int64_t timeout_ms) {
    const ZapStr *body = (const ZapStr *)body_;
    const ZapDict *hdrs = (const ZapDict *)headers_;

    if (timeout_ms < 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("http: timeout must be non-negative",
                                    (int64_t)sizeof("http: timeout must be non-negative") - 1));
        return NULL;
    }
    /* One absolute monotonic deadline shared by every hop, so the budget spans
     * the whole exchange (connect + reads + redirects), not each hop afresh. */
    int64_t deadline_ns = (timeout_ms > 0) ? zap_deadline_after_ms(timeout_ms) : 0;

    char *cur = zap_net_dupz((const ZapStr *)url_);   /* current absolute URL */
    char *curm = zap_net_dupz((const ZapStr *)method_); /* current method */
    if (!cur || !curm) {
        free(cur);
        free(curm);
        zap_net_panic("http: out of memory");
        return NULL;
    }

    char **chain = NULL; /* resolved redirect targets, in order */
    size_t nchain = 0;
    int drop_body = 0;     /* this hop sends an empty body */
    int suppress_ctype = 0; /* and no caller Content-Type */
    ZAP_STACK_STR(empty, "", 0);
    ZapHttpResp *resp = NULL;

    for (int hop = 0;; hop++) {
        /* Method as a one-shot GC string; the body is either the caller's
         * original (still rooted by its frame) or the stack empty. Both are
         * consumed into the malloc request buffer before do_once's first park. */
        ZapStr *mZ = (ZapStr *)zap_str_new(curm, (int64_t)strlen(curm));
        const ZapStr *bodyArg = drop_body ? (const ZapStr *)empty : body;
        resp = (ZapHttpResp *)zap_http_do_once(mZ, cur, bodyArg, hdrs, suppress_ctype, deadline_ns);
        long st = resp->status; /* captured before any later allocation */

        if (!zap_http_is_redirect(st)) {
            break; /* terminal response */
        }
        char *loc = zap_http_resp_header_cstr(resp, "Location");
        if (!loc) {
            break; /* 3xx without a usable Location — return it as-is */
        }
        if (hop >= ZAP_HTTP_MAX_REDIRECTS) {
            free(loc);
            for (size_t k = 0; k < nchain; k++) {
                free(chain[k]);
            }
            free(chain);
            free(cur);
            free(curm);
            zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                            zap_str_new("http: too many redirects",
                                        (int64_t)sizeof("http: too many redirects") - 1));
            return NULL;
        }

        /* Resolve Location against the current URL (RFC-3986 §5.2). The transient
         * ZapStrs and the join result are pure-CPU work with no park, copied to a
         * malloc string before the next request. */
        ZapStr *baseZ = (ZapStr *)zap_str_new(cur, (int64_t)strlen(cur));
        ZapStr *refZ = (ZapStr *)zap_str_new(loc, (int64_t)strlen(loc));
        ZapStr *joined = (ZapStr *)zap_url_join(baseZ, refZ);
        char *newurl = zap_net_dupz(joined);
        free(loc);
        if (!newurl) {
            for (size_t k = 0; k < nchain; k++) {
                free(chain[k]);
            }
            free(chain);
            free(cur);
            free(curm);
            zap_net_panic("http: out of memory");
            return NULL;
        }

        /* Record the hop. */
        char **grown = (char **)realloc(chain, (nchain + 1) * sizeof(char *));
        if (!grown) {
            free(newurl);
            for (size_t k = 0; k < nchain; k++) {
                free(chain[k]);
            }
            free(chain);
            free(cur);
            free(curm);
            zap_net_panic("http: out of memory");
            return NULL;
        }
        chain = grown;
        chain[nchain] = zap_http_cstr_dup(newurl);
        nchain++;

        /* Method/body across the hop (see the block comment). */
        if (st == 303) {
            if (!zap_http_method_is(curm, "HEAD")) {
                free(curm);
                curm = zap_http_cstr_dup("GET");
                drop_body = 1;
            }
        } else if (st == 301 || st == 302) {
            if (zap_http_method_is(curm, "POST")) {
                free(curm);
                curm = zap_http_cstr_dup("GET");
                drop_body = 1;
            }
        } /* 307 / 308: preserve method and body */
        suppress_ctype = drop_body;

        free(cur);
        cur = newurl;
    }

    /* Terminal response in hand: no further park, so build the GC results now. */
    resp->final_url = (const ZapStr *)zap_str_new(cur, (int64_t)strlen(cur));
    void *chainlist = zap_list_new();
    for (size_t k = 0; k < nchain; k++) {
        zap_list_push_ptr(chainlist, zap_str_new(chain[k], (int64_t)strlen(chain[k])));
        free(chain[k]);
    }
    free(chain);
    free(cur);
    free(curm);
    resp->chain = chainlist;
    return resp;
}

/* http.final_url(resp) -> str. The URL the returned response came from: the last
 * URL dialed after any redirects, or the original request URL (#987). */
void *zap_http_final_url(void *resp_) { return (void *)((ZapHttpResp *)resp_)->final_url; }

/* http.redirects(resp) -> list[str]. The redirect targets followed, in order;
 * the empty list when none were (#987). */
void *zap_http_redirects(void *resp_) { return ((ZapHttpResp *)resp_)->chain; }

/* http.request(method, url, body) — no caller headers. */
void *zap_http_request(void *method_, void *url_, void *body_) {
    return zap_http_do(method_, url_, body_, NULL, 0);
}

/* http.request(method, url, body, headers) — with a `dict[str,str]` (#280). */
void *zap_http_request_h(void *method_, void *url_, void *body_, void *headers_) {
    return zap_http_do(method_, url_, body_, headers_, 0);
}

/* http.request(method, url, body, headers, timeout_ms) — with a total deadline
 * (#986). `headers` is always present (the seam binder defaults it to `{}` when a
 * caller supplies only `timeout`). */
void *zap_http_request_ht(void *method_, void *url_, void *body_, void *headers_,
                          int64_t timeout_ms) {
    return zap_http_do(method_, url_, body_, headers_, timeout_ms);
}

void *zap_http_get(void *url_) {
    ZAP_STACK_STR(g, "GET", 3);
    ZAP_STACK_STR(empty, "", 0);
    return zap_http_do(g, url_, empty, NULL, 0);
}

void *zap_http_get_h(void *url_, void *headers_) {
    ZAP_STACK_STR(g, "GET", 3);
    ZAP_STACK_STR(empty, "", 0);
    return zap_http_do(g, url_, empty, headers_, 0);
}

/* http.get(url[, headers], timeout_ms) — total deadline (#986). */
void *zap_http_get_ht(void *url_, void *headers_, int64_t timeout_ms) {
    ZAP_STACK_STR(g, "GET", 3);
    ZAP_STACK_STR(empty, "", 0);
    return zap_http_do(g, url_, empty, headers_, timeout_ms);
}

void *zap_http_post(void *url_, void *body_) {
    ZAP_STACK_STR(p, "POST", 4);
    return zap_http_do(p, url_, body_, NULL, 0);
}

void *zap_http_post_h(void *url_, void *body_, void *headers_) {
    ZAP_STACK_STR(p, "POST", 4);
    return zap_http_do(p, url_, body_, headers_, 0);
}

/* http.post(url, body[, headers], timeout_ms) — total deadline (#986). */
void *zap_http_post_ht(void *url_, void *body_, void *headers_, int64_t timeout_ms) {
    ZAP_STACK_STR(p, "POST", 4);
    return zap_http_do(p, url_, body_, headers_, timeout_ms);
}

/* ===== keep-alive connection pool + single-response framing (#990/#1049) =====
 *
 * `http.Client` (a prelude struct) owns a `ZapHttpPool` addressed by an int64
 * handle. The pool is NON-GC malloc memory — a stable address never moved by the
 * precise-Immix collector — so the C code may hold it across network parks
 * safely; it owns only non-GC resources (the idle-connection list and the live
 * fd/pd/SSL in each), freed by `http.Client.close()`. The GC `ZapHttpResp` is
 * built only at the very end, after the last park, exactly as `zap_http_do_once`.
 *
 * Reuse requires reading EXACTLY one response (so the socket is left at a clean
 * message boundary): `zap_http_read_one` frames the body by `Content-Length` or
 * decodes `Transfer-Encoding: chunked`. A connection is returned to the pool only
 * when the response was fully framed AND keep-alive applies (HTTP/1.1 without
 * `Connection: close`, or HTTP/1.0 with `Connection: keep-alive`). A pooled
 * connection the peer has since closed is detected (EOF before any response
 * byte) and retried once on a fresh dial. */

typedef struct {
    int tls;
    char *host;      /* malloc'd, NUL-terminated, no port */
    long port;
    const char *path; /* into the url_cstr (or the literal "/") */
    size_t hostend;   /* host length (for the Host header) */
    const char *hoststart; /* host start within url_cstr */
} ZapHttpUrl;

/* Parse an absolute URL into scheme/host/port/path. Returns 0, or -1 on OOM. */
static int zap_http_url_parse(const char *url_cstr, ZapHttpUrl *u) {
    u->tls = 0;
    const char *rest = url_cstr;
    if (strncmp(url_cstr, "https://", 8) == 0) {
        u->tls = 1;
        rest = url_cstr + 8;
    } else if (strncmp(url_cstr, "http://", 7) == 0) {
        rest = url_cstr + 7;
    }
    size_t he = 0;
    while (rest[he] && rest[he] != '/' && rest[he] != ':') {
        he++;
    }
    u->host = (char *)malloc(he + 1);
    if (!u->host) {
        return -1;
    }
    memcpy(u->host, rest, he);
    u->host[he] = '\0';
    u->hostend = he;
    u->hoststart = rest;
    long port = u->tls ? 443 : 80;
    const char *after = rest + he;
    if (*after == ':') {
        after++;
        port = 0;
        while (*after >= '0' && *after <= '9') {
            port = port * 10 + (*after - '0');
            after++;
        }
    }
    u->port = port;
    u->path = (*after == '/') ? after : "/";
    return 0;
}

/* Dial a fresh transport for `u`, bounded by the absolute `deadline_ns`. Raises
 * (net/TLS) on failure, exactly like the one-shot client. */
static void zap_http_dial(const ZapHttpUrl *u, int64_t deadline_ns, ZapHttpConn *hc) {
    memset(hc, 0, sizeof *hc);
    hc->deadline_ns = deadline_ns;
    ZapStr *hostz = (ZapStr *)zap_str_new(u->host, (int64_t)strlen(u->host));
    if (u->tls) {
        SSL_CTX *ctx = NULL;
        int fd = -1;
        void *pd = NULL;
        SSL *ssl =
            zap_tls_dial_raw_impl(hostz, NULL, u->port, 1, deadline_ns, NULL, &ctx, &fd, &pd);
        hc->tls = 1;
        hc->ssl = ssl;
        hc->ctx = ctx;
        hc->fd = fd;
        hc->pd = pd;
    } else {
        ZapSock *conn = (ZapSock *)zap_net_connect_impl((void *)hostz, u->port, deadline_ns, 0);
        hc->fd = conn->fd;
        hc->pd = conn->pd;
    }
}

/* Find header `name` (case-insensitive) in a header block, returning its value
 * range [*vp, *vp+*vlen) within `hbuf` (no allocation). Returns 1 if found. */
static int zap_http_hdr_raw(const char *hbuf, size_t hlen, const char *name, const char **vp,
                            size_t *vlen) {
    size_t nlen = strlen(name);
    size_t i = 0;
    while (i < hlen) {
        size_t ls = i;
        while (i < hlen && hbuf[i] != '\n') {
            i++;
        }
        size_t le = i; /* line [ls, le) (may include trailing '\r') */
        if (i < hlen) {
            i++;
        }
        size_t llen = (le > ls && hbuf[le - 1] == '\r') ? (le - 1 - ls) : (le - ls);
        if (llen > nlen && hbuf[ls + nlen] == ':') {
            int eq = 1;
            for (size_t k = 0; k < nlen; k++) {
                char a = hbuf[ls + k];
                char b = name[k];
                if (a >= 'A' && a <= 'Z') {
                    a = (char)(a - 'A' + 'a');
                }
                if (b >= 'A' && b <= 'Z') {
                    b = (char)(b - 'A' + 'a');
                }
                if (a != b) {
                    eq = 0;
                    break;
                }
            }
            if (eq) {
                size_t vs = ls + nlen + 1;
                size_t ve = ls + llen;
                while (vs < ve && (hbuf[vs] == ' ' || hbuf[vs] == '\t')) {
                    vs++;
                }
                *vp = hbuf + vs;
                *vlen = ve - vs;
                return 1;
            }
        }
    }
    return 0;
}

/* Does header `name` carry `token` (case-insensitive, comma/space tolerant)? */
static int zap_http_hdr_has_token(const char *hbuf, size_t hlen, const char *name,
                                  const char *token) {
    const char *vp;
    size_t vlen;
    if (!zap_http_hdr_raw(hbuf, hlen, name, &vp, &vlen)) {
        return 0;
    }
    size_t tlen = strlen(token);
    for (size_t i = 0; i + tlen <= vlen; i++) {
        int eq = 1;
        for (size_t k = 0; k < tlen; k++) {
            char a = vp[i + k];
            char b = token[k];
            if (a >= 'A' && a <= 'Z') {
                a = (char)(a - 'A' + 'a');
            }
            if (b >= 'A' && b <= 'Z') {
                b = (char)(b - 'A' + 'a');
            }
            if (a != b) {
                eq = 0;
                break;
            }
        }
        if (eq) {
            return 1;
        }
    }
    return 0;
}

/* Build and send a request on `hc`, keep-alive (no `Connection: close`). The
 * buffer building mirrors `zap_http_do_once` minus the close header. Returns 0,
 * or a negative transport status (caller maps to an exception). */
static int zap_http_send_req(ZapHttpConn *hc, const char *method, const ZapHttpUrl *u,
                             const ZapStr *body, const ZapDict *hdrs, int suppress_ctype) {
    size_t blen = (size_t)(body->len < 0 ? 0 : body->len);
    size_t hdrbytes = 0;
    int caller_content_type = 0;
    if (hdrs) {
        for (int64_t e = 0; e < hdrs->cap; e++) {
            if (zap_d_tags(hdrs)[e] < 2) {
                continue;
            }
            const ZapStr *name = (const ZapStr *)(intptr_t)zap_d_keys(hdrs)[e];
            const ZapStr *val = (const ZapStr *)(intptr_t)zap_d_vals(hdrs)[e];
            if (zap_http_hdr_managed(name)) {
                continue;
            }
            if (zap_http_name_eq(name, "Content-Type")) {
                caller_content_type = 1;
            }
            size_t nl = (size_t)(name->len < 0 ? 0 : name->len);
            size_t vl = (size_t)(val->len < 0 ? 0 : val->len);
            hdrbytes += nl + vl + 4;
        }
    }
    size_t reqcap = strlen(method) + strlen(u->path) + u->hostend + blen + hdrbytes + 256;
    char *req = (char *)malloc(reqcap);
    if (!req) {
        zap_net_panic("http: out of memory");
        return -1;
    }
    int hdr = snprintf(req, reqcap, "%s %s HTTP/1.1\r\nHost: %.*s\r\n", method, u->path,
                       (int)u->hostend, u->hoststart);
    if (blen > 0) {
        hdr += snprintf(req + hdr, reqcap - (size_t)hdr, "Content-Length: %zu\r\n", blen);
        if (!caller_content_type) {
            hdr += snprintf(req + hdr, reqcap - (size_t)hdr,
                            "Content-Type: text/plain; charset=utf-8\r\n");
        }
    }
    if (hdrs) {
        for (int64_t e = 0; e < hdrs->cap; e++) {
            if (zap_d_tags(hdrs)[e] < 2) {
                continue;
            }
            const ZapStr *name = (const ZapStr *)(intptr_t)zap_d_keys(hdrs)[e];
            const ZapStr *val = (const ZapStr *)(intptr_t)zap_d_vals(hdrs)[e];
            if (zap_http_hdr_managed(name)) {
                continue;
            }
            if (suppress_ctype && zap_http_name_eq(name, "Content-Type")) {
                continue;
            }
            size_t nl = (size_t)(name->len < 0 ? 0 : name->len);
            size_t vl = (size_t)(val->len < 0 ? 0 : val->len);
            hdr += snprintf(req + hdr, reqcap - (size_t)hdr, "%.*s: %.*s\r\n", (int)nl, name->data,
                            (int)vl, val->data);
        }
    }
    hdr += snprintf(req + hdr, reqcap - (size_t)hdr, "\r\n");
    if (blen > 0) {
        memcpy(req + hdr, body->data, blen);
        hdr += (int)blen;
    }
    int sent = zap_httpconn_send_all(hc, req, (size_t)hdr);
    free(req);
    return sent;
}

/* Raise the catchable response-too-large error (#1448): a response body would
 * exceed the client's `max_response_bytes` cap. `ResponseTooLargeError` ⊂
 * `OSError` ⊂ `Exception` (zaprt.c), thrown via the typed-panic/invoke path so
 * it is caught inside a `try` rather than aborting. The caller closes the
 * connection before this fires, so a partial over-limit response never returns
 * to the keep-alive pool. */
static void zap_http_too_large_panic(void) {
    const char *m = "http: response body exceeds max_response_bytes";
    zap_panic_typed(ZAP_EXC_RESPONSE_TOO_LARGE_ERROR, zap_str_new(m, (int64_t)strlen(m)));
}

/* Read exactly one response off `hc`. On success returns 0 and sets the out
 * params: `status`, a malloc'd first-line-stripped header block
 * (`headers`/`headers_len`, like `ZapHttpResp.headers`), a malloc'd decoded body
 * (`body`/`body_len`), and `reusable` (the connection may be pooled). Returns 1
 * — "stale" — when the peer closed before sending any byte (a dead pooled
 * connection the caller retries once). Raises on a read deadline / cancellation /
 * malformed response. The buffer lives across parks (never GC).
 *
 * `max_response_bytes` (#1448) caps the received body: `0` is unlimited;
 * otherwise a body of exactly the limit succeeds and the first byte over it
 * raises `ResponseTooLargeError`. The cap is enforced during the receive in all
 * three framing modes — a `Content-Length` over the cap is rejected before any
 * body byte is read, and a `chunked` / EOF-framed body is cut off the moment its
 * running decoded/buffered size crosses the cap. On an over-limit raise this
 * closes `hc` before unwinding, so the socket is neither released to the pool nor
 * reused. */
static int zap_http_read_one(ZapHttpConn *hc, int64_t max_response_bytes, long *status_out,
                             char **headers_out, size_t *headers_len_out, char **body_out,
                             size_t *body_len_out, int *reusable_out) {
    ZapDynBuf b = {NULL, 0, 0};
    size_t hlen = 0, body_off = 0;
    int have_headers = 0;
    for (;;) {
        if (b.len > 0 && zap_http_split(&b, &hlen, &body_off) == 0) {
            have_headers = 1;
            break;
        }
        ssize_t r = zap_httpconn_fill(hc, &b);
        if (r == -2 || r == -3) {
            zap_dyn_free(&b);
            zap_io_panic((int)r, "http: read timed out");
            return -1;
        }
        if (r == 0) {
            int empty = (b.len == 0);
            zap_dyn_free(&b);
            if (empty) {
                return 1; /* stale: peer closed before any byte */
            }
            zap_io_panic(-1, "http: incomplete response");
            return -1;
        }
        if (r < 0) {
            zap_dyn_free(&b);
            zap_io_panic((int)r, "http: read failed");
            return -1;
        }
    }
    (void)have_headers;

    /* status code: first space, then the digits */
    long status = 0;
    size_t sp = 0;
    while (sp < hlen && b.buf[sp] != ' ') {
        sp++;
    }
    sp++;
    while (sp < hlen && b.buf[sp] >= '0' && b.buf[sp] <= '9') {
        status = status * 10 + (b.buf[sp] - '0');
        sp++;
    }
    int http11 = (hlen >= 8 && memcmp(b.buf, "HTTP/1.1", 8) == 0);

    /* header block for lookups starts after the status line */
    size_t hstart = 0;
    while (hstart < hlen && b.buf[hstart] != '\n') {
        hstart++;
    }
    if (hstart < hlen) {
        hstart++;
    }
    const char *hblk = b.buf + hstart;
    size_t hblk_len = hlen > hstart ? hlen - hstart : 0;

    int chunked = zap_http_hdr_has_token(hblk, hblk_len, "Transfer-Encoding", "chunked");
    int conn_close = zap_http_hdr_has_token(hblk, hblk_len, "Connection", "close");
    int conn_ka = zap_http_hdr_has_token(hblk, hblk_len, "Connection", "keep-alive");
    long clen = chunked ? -1 : zap_http_content_length(hblk, hblk_len);

    char *body = NULL;
    size_t blen = 0;
    int framed = 0;

    if (chunked) {
        ZapDynBuf out = {NULL, 0, 0};
        size_t pos = body_off;
        int ok = 1;
        for (;;) {
            const char *crlf;
            while (!(crlf = zap_memfind(b.buf + pos, b.len - pos, "\r\n", 2))) {
                ssize_t r = zap_httpconn_fill(hc, &b);
                if (r == -2 || r == -3) {
                    zap_dyn_free(&out);
                    zap_dyn_free(&b);
                    zap_io_panic((int)r, "http: read timed out");
                    return -1;
                }
                if (r <= 0) {
                    ok = 0;
                    break;
                }
            }
            if (!ok) {
                break;
            }
            size_t line = (size_t)(crlf - (b.buf + pos));
            long csize = 0;
            for (size_t k = 0; k < line; k++) {
                char c = b.buf[pos + k];
                int d;
                if (c >= '0' && c <= '9') {
                    d = c - '0';
                } else if (c >= 'a' && c <= 'f') {
                    d = c - 'a' + 10;
                } else if (c >= 'A' && c <= 'F') {
                    d = c - 'A' + 10;
                } else {
                    break; /* ';' chunk-ext or stray char ends the size */
                }
                csize = csize * 16 + d;
            }
            if (csize == 0) {
                /* final chunk: consume through the terminating blank line so the
                 * socket is left clean for reuse (handles optional trailers). */
                while (!zap_memfind(b.buf + pos, b.len - pos, "\r\n\r\n", 4)) {
                    ssize_t r = zap_httpconn_fill(hc, &b);
                    if (r == -2 || r == -3) {
                        zap_dyn_free(&out);
                        zap_dyn_free(&b);
                        zap_io_panic((int)r, "http: read timed out");
                        return -1;
                    }
                    if (r <= 0) {
                        break; /* tolerate a missing final CRLF at EOF */
                    }
                }
                framed = 1;
                break;
            }
            size_t data_start = pos + line + 2;
            while (b.len < data_start + (size_t)csize + 2) {
                ssize_t r = zap_httpconn_fill(hc, &b);
                if (r == -2 || r == -3) {
                    zap_dyn_free(&out);
                    zap_dyn_free(&b);
                    zap_io_panic((int)r, "http: read timed out");
                    return -1;
                }
                if (r <= 0) {
                    ok = 0;
                    break;
                }
            }
            if (!ok) {
                break;
            }
            if (zap_dyn_reserve(&out, (size_t)csize) != 0) {
                zap_dyn_free(&out);
                zap_dyn_free(&b);
                zap_net_panic("http: out of memory");
                return -1;
            }
            memcpy(out.buf + out.len, b.buf + data_start, (size_t)csize);
            out.len += (size_t)csize;
            if (max_response_bytes > 0 && out.len > (size_t)max_response_bytes) {
                /* Over the cap mid-stream: evict the socket (never pool it) and
                 * raise. Exactly the limit is fine; this fires only past it. */
                zap_dyn_free(&out);
                zap_dyn_free(&b);
                zap_httpconn_close(hc);
                zap_http_too_large_panic();
                return -1;
            }
            pos = data_start + (size_t)csize + 2; /* data + trailing CRLF */
        }
        if (!framed) {
            zap_dyn_free(&out);
            zap_dyn_free(&b);
            zap_io_panic(-1, "http: malformed chunked response");
            return -1;
        }
        blen = out.len;
        body = out.buf ? out.buf : (char *)malloc(1);
        /* ownership of out.buf transferred to `body` */
    } else if (clen >= 0) {
        if (max_response_bytes > 0 && clen > max_response_bytes) {
            /* Declared length already over the cap: reject off the header alone,
             * before reading a single body byte. Evict the socket. */
            zap_dyn_free(&b);
            zap_httpconn_close(hc);
            zap_http_too_large_panic();
            return -1;
        }
        while ((long)(b.len - body_off) < clen) {
            ssize_t r = zap_httpconn_fill(hc, &b);
            if (r == -2 || r == -3) {
                zap_dyn_free(&b);
                zap_io_panic((int)r, "http: read timed out");
                return -1;
            }
            if (r <= 0) {
                zap_dyn_free(&b);
                zap_io_panic(-1, "http: truncated response body");
                return -1;
            }
        }
        blen = (size_t)clen;
        body = (char *)malloc(blen ? blen : 1);
        if (!body) {
            zap_dyn_free(&b);
            zap_net_panic("http: out of memory");
            return -1;
        }
        memcpy(body, b.buf + body_off, blen);
        framed = 1;
    } else {
        /* no framing: read to EOF (never reusable) */
        for (;;) {
            /* Check the buffered body at the TOP of the loop, before filling, so
             * the cap catches a body that already arrived whole alongside the
             * headers (the common loopback case, where the next fill is an
             * immediate EOF) as well as one that streams in over several fills.
             * Exactly the limit is fine; this fires only past it. */
            if (max_response_bytes > 0 && (b.len - body_off) > (size_t)max_response_bytes) {
                zap_dyn_free(&b);
                zap_httpconn_close(hc);
                zap_http_too_large_panic();
                return -1;
            }
            ssize_t r = zap_httpconn_fill(hc, &b);
            if (r == -2 || r == -3) {
                zap_dyn_free(&b);
                zap_io_panic((int)r, "http: read timed out");
                return -1;
            }
            if (r <= 0) {
                break;
            }
        }
        blen = b.len > body_off ? b.len - body_off : 0;
        body = (char *)malloc(blen ? blen : 1);
        if (!body) {
            zap_dyn_free(&b);
            zap_net_panic("http: out of memory");
            return -1;
        }
        memcpy(body, b.buf + body_off, blen);
        framed = 0;
    }

    char *headers = (char *)malloc(hblk_len ? hblk_len : 1);
    if (!headers) {
        free(body);
        zap_dyn_free(&b);
        zap_net_panic("http: out of memory");
        return -1;
    }
    memcpy(headers, hblk, hblk_len);

    *status_out = status;
    *headers_out = headers;
    *headers_len_out = hblk_len;
    *body_out = body;
    *body_len_out = blen;
    *reusable_out = framed && !conn_close && (http11 || conn_ka);
    zap_dyn_free(&b);
    return 0;
}

/* ---- the pool ------------------------------------------------------------- */
typedef struct ZapHttpIdle {
    char *key; /* "p|host|port" or "t|host|port" */
    ZapHttpConn hc;
    struct ZapHttpIdle *next;
} ZapHttpIdle;

typedef struct {
    ZapHttpIdle *idle; /* LIFO */
    int64_t n_idle;
    int64_t max_idle;
    pthread_mutex_t mu;
} ZapHttpPool;

static char *zap_http_pool_key(const ZapHttpUrl *u) {
    char *k = (char *)malloc(strlen(u->host) + 32);
    if (k) {
        snprintf(k, strlen(u->host) + 32, "%c|%s|%ld", u->tls ? 't' : 'p', u->host, u->port);
    }
    return k;
}

/* Pop an idle connection for `key`, or return 0. */
static int zap_http_pool_acquire(ZapHttpPool *p, const char *key, ZapHttpConn *hc) {
    int got = 0;
    pthread_mutex_lock(&p->mu);
    ZapHttpIdle **pp = &p->idle;
    while (*pp) {
        if (strcmp((*pp)->key, key) == 0) {
            ZapHttpIdle *n = *pp;
            *pp = n->next;
            p->n_idle--;
            *hc = n->hc;
            free(n->key);
            free(n);
            got = 1;
            break;
        }
        pp = &(*pp)->next;
    }
    pthread_mutex_unlock(&p->mu);
    return got;
}

/* Return a connection to the pool, or close it if the pool is full / OOM. */
static void zap_http_pool_release(ZapHttpPool *p, const char *key, const ZapHttpConn *hc) {
    pthread_mutex_lock(&p->mu);
    int room = p->n_idle < p->max_idle;
    pthread_mutex_unlock(&p->mu);
    if (!room) {
        ZapHttpConn tmp = *hc;
        zap_httpconn_close(&tmp);
        return;
    }
    ZapHttpIdle *n = (ZapHttpIdle *)malloc(sizeof *n);
    char *k = (char *)malloc(strlen(key) + 1);
    if (!n || !k) {
        free(n);
        free(k);
        ZapHttpConn tmp = *hc;
        zap_httpconn_close(&tmp);
        return;
    }
    memcpy(k, key, strlen(key) + 1);
    n->key = k;
    n->hc = *hc;
    pthread_mutex_lock(&p->mu);
    n->next = p->idle;
    p->idle = n;
    p->n_idle++;
    pthread_mutex_unlock(&p->mu);
}

/* http.Client pool constructor. Returns the pool as an int64 handle (a stable
 * non-GC pointer), or raises on OOM. `max_idle <= 0` uses the default (8). */
int64_t zap_http_pool_new(int64_t max_idle) {
    ZapHttpPool *p = (ZapHttpPool *)malloc(sizeof *p);
    if (!p) {
        zap_net_panic("http: out of memory");
        return 0;
    }
    p->idle = NULL;
    p->n_idle = 0;
    p->max_idle = max_idle > 0 ? max_idle : 8;
    pthread_mutex_init(&p->mu, NULL);
    return (int64_t)(intptr_t)p;
}

/* Drain and close every idle connection, then free the pool. After this the
 * handle is dangling; the prelude `Client.close()` is the only caller and does
 * not reuse it. */
void zap_http_pool_close(int64_t handle) {
    ZapHttpPool *p = (ZapHttpPool *)(intptr_t)handle;
    if (!p) {
        return;
    }
    pthread_mutex_lock(&p->mu);
    ZapHttpIdle *n = p->idle;
    p->idle = NULL;
    p->n_idle = 0;
    pthread_mutex_unlock(&p->mu);
    while (n) {
        ZapHttpIdle *next = n->next;
        zap_httpconn_close(&n->hc);
        free(n->key);
        free(n);
        n = next;
    }
    pthread_mutex_destroy(&p->mu);
    free(p);
}

/* One request over the pool, with redirect following honoring `follow` /
 * `max_redirects` (#1049) and connection reuse (#990). `timeout_ms` is the #986
 * total deadline. Returns a `ZapHttpResp`. */
void *zap_http_pool_do(int64_t handle, void *method_, void *url_, void *body_, void *headers_,
                       int64_t timeout_ms, int64_t follow, int64_t max_redirects,
                       int64_t max_response_bytes) {
    ZapHttpPool *p = (ZapHttpPool *)(intptr_t)handle;
    const ZapStr *body = (const ZapStr *)body_;
    const ZapDict *hdrs = (const ZapDict *)headers_;

    if (timeout_ms < 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("http: timeout must be non-negative",
                                    (int64_t)sizeof("http: timeout must be non-negative") - 1));
        return NULL;
    }
    if (max_redirects < 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("http: max_redirects must be non-negative",
                                    (int64_t)sizeof("http: max_redirects must be non-negative") - 1));
        return NULL;
    }
    if (max_response_bytes < 0) {
        zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                        zap_str_new("http: max_response_bytes must be non-negative",
                                    (int64_t)sizeof("http: max_response_bytes must be non-negative") - 1));
        return NULL;
    }

    char *cur = zap_net_dupz((const ZapStr *)url_);
    char *curm = zap_net_dupz((const ZapStr *)method_);
    if (!cur || !curm) {
        free(cur);
        free(curm);
        zap_net_panic("http: out of memory");
        return NULL;
    }

    char **chain = NULL;
    size_t nchain = 0;
    int drop_body = 0;
    int suppress_ctype = 0;
    ZAP_STACK_STR(empty, "", 0);

    long status = 0;
    char *resp_headers = NULL;
    size_t resp_headers_len = 0;
    char *resp_body = NULL;
    size_t resp_body_len = 0;

    for (int hop = 0;; hop++) {
        int64_t deadline_ns = (timeout_ms > 0) ? zap_deadline_after_ms(timeout_ms) : 0;
        ZapHttpUrl u;
        if (zap_http_url_parse(cur, &u) != 0) {
            free(cur);
            free(curm);
            zap_net_panic("http: out of memory");
            return NULL;
        }
        char *key = zap_http_pool_key(&u);
        const ZapStr *bodyArg = drop_body ? (const ZapStr *)empty : body;

        /* Try a pooled connection; on a stale one (peer already closed) dial a
         * fresh connection and retry exactly once. */
        int rc = 1; /* 1 == stale, forces the first iteration to (re)send */
        for (int attempt = 0; attempt < 2 && rc == 1; attempt++) {
            ZapHttpConn hc;
            int from_pool = key && zap_http_pool_acquire(p, key, &hc);
            if (from_pool) {
                hc.deadline_ns = deadline_ns;
            } else {
                zap_http_dial(&u, deadline_ns, &hc); /* raises on failure */
            }
            int sent = zap_http_send_req(&hc, curm, &u, bodyArg, hdrs, suppress_ctype);
            if (sent < 0) {
                zap_httpconn_close(&hc);
                if (from_pool && attempt == 0) {
                    rc = 1; /* stale: retry with a fresh dial */
                    continue;
                }
                free(key);
                free(u.host);
                free(cur);
                free(curm);
                for (size_t k = 0; k < nchain; k++) {
                    free(chain[k]);
                }
                free(chain);
                free(resp_headers);
                free(resp_body);
                zap_io_panic(sent, "http: send failed");
                return NULL;
            }
            int reusable = 0;
            free(resp_headers);
            free(resp_body);
            resp_headers = NULL;
            resp_body = NULL;
            rc = zap_http_read_one(&hc, max_response_bytes, &status, &resp_headers,
                                   &resp_headers_len, &resp_body, &resp_body_len, &reusable);
            if (rc == 1) {
                /* stale pooled connection (EOF before a byte) */
                zap_httpconn_close(&hc);
                if (from_pool && attempt == 0) {
                    continue; /* retry fresh */
                }
                free(key);
                free(u.host);
                free(cur);
                free(curm);
                for (size_t k = 0; k < nchain; k++) {
                    free(chain[k]);
                }
                free(chain);
                zap_io_panic(-1, "http: incomplete response");
                return NULL;
            }
            if (reusable && key) {
                zap_http_pool_release(p, key, &hc);
            } else {
                zap_httpconn_close(&hc);
            }
        }
        free(key);

        if (g_is_cancelled()) {
            free(u.host);
            free(cur);
            free(curm);
            for (size_t k = 0; k < nchain; k++) {
                free(chain[k]);
            }
            free(chain);
            free(resp_headers);
            free(resp_body);
            zap_raise(cancelled_exc());
            return NULL;
        }

        int is_redirect = zap_http_is_redirect(status);
        char *loc = NULL;
        if (is_redirect && follow) {
            const char *vp;
            size_t vlen;
            if (zap_http_hdr_raw(resp_headers, resp_headers_len, "Location", &vp, &vlen) &&
                vlen > 0) {
                loc = (char *)malloc(vlen + 1);
                if (loc) {
                    memcpy(loc, vp, vlen);
                    loc[vlen] = '\0';
                }
            }
        }
        if (!loc) {
            free(u.host);
            break; /* terminal response (or follow disabled / no Location) */
        }
        if (hop >= max_redirects) {
            free(loc);
            free(u.host);
            free(cur);
            free(curm);
            for (size_t k = 0; k < nchain; k++) {
                free(chain[k]);
            }
            free(chain);
            free(resp_headers);
            free(resp_body);
            zap_panic_typed(ZAP_EXC_VALUE_ERROR,
                            zap_str_new("http: too many redirects",
                                        (int64_t)sizeof("http: too many redirects") - 1));
            return NULL;
        }

        ZapStr *baseZ = (ZapStr *)zap_str_new(cur, (int64_t)strlen(cur));
        ZapStr *refZ = (ZapStr *)zap_str_new(loc, (int64_t)strlen(loc));
        ZapStr *joined = (ZapStr *)zap_url_join(baseZ, refZ);
        char *newurl = zap_net_dupz(joined);
        free(loc);
        free(u.host);
        if (!newurl) {
            free(cur);
            free(curm);
            for (size_t k = 0; k < nchain; k++) {
                free(chain[k]);
            }
            free(chain);
            free(resp_headers);
            free(resp_body);
            zap_net_panic("http: out of memory");
            return NULL;
        }
        char **grown = (char **)realloc(chain, (nchain + 1) * sizeof(char *));
        if (!grown) {
            free(newurl);
            free(cur);
            free(curm);
            for (size_t k = 0; k < nchain; k++) {
                free(chain[k]);
            }
            free(chain);
            free(resp_headers);
            free(resp_body);
            zap_net_panic("http: out of memory");
            return NULL;
        }
        chain = grown;
        chain[nchain] = zap_http_cstr_dup(newurl);
        nchain++;

        if (status == 303) {
            if (!zap_http_method_is(curm, "HEAD")) {
                free(curm);
                curm = zap_http_cstr_dup("GET");
                drop_body = 1;
            }
        } else if (status == 301 || status == 302) {
            if (zap_http_method_is(curm, "POST")) {
                free(curm);
                curm = zap_http_cstr_dup("GET");
                drop_body = 1;
            }
        }
        suppress_ctype = drop_body;
        free(cur);
        cur = newurl;
    }

    /* Terminal response: build the GC results now (no further park). */
    ZapHttpResp *resp = (ZapHttpResp *)zap_obj(&zap_desc_httpresp);
    resp->status = (int64_t)status;
    resp->headers = (const ZapStr *)zap_str_new(resp_headers ? resp_headers : "",
                                                (int64_t)resp_headers_len);
    resp->body =
        (const ZapStr *)zap_str_new(resp_body ? resp_body : "", (int64_t)resp_body_len);
    resp->final_url = (const ZapStr *)zap_str_new(cur, (int64_t)strlen(cur));
    void *chainlist = zap_list_new();
    for (size_t k = 0; k < nchain; k++) {
        zap_list_push_ptr(chainlist, zap_str_new(chain[k], (int64_t)strlen(chain[k])));
        free(chain[k]);
    }
    free(chain);
    free(cur);
    free(curm);
    free(resp_headers);
    free(resp_body);
    resp->chain = chainlist;
    return resp;
}

int64_t zap_http_status(void *resp_) { return ((ZapHttpResp *)resp_)->status; }
void *zap_http_body(void *resp_) { return (void *)((ZapHttpResp *)resp_)->body; }

void *zap_http_header(void *resp_, void *name_) {
    ZapHttpResp *resp = (ZapHttpResp *)resp_;
    char *nz = zap_net_dupz((const ZapStr *)name_);
    if (!nz) {
        return zap_str_new("", 0);
    }
    void *v = zap_http_header_value(resp->headers->data, (size_t)resp->headers->len, nz);
    free(nz);
    return v;
}

/* http.url(host, port, path) -> "http://host:port/path". A convenience for
 * dialing a server whose (ephemeral) port is only known at runtime; `path` gets
 * a leading '/' if it lacks one. No park, so the GC result is built directly. */
void *zap_http_url(void *host_, int64_t port, void *path_) {
    const ZapStr *host = (const ZapStr *)host_;
    const ZapStr *path = (const ZapStr *)path_;
    size_t hlen = (size_t)(host->len < 0 ? 0 : host->len);
    size_t plen = (size_t)(path->len < 0 ? 0 : path->len);
    size_t cap = hlen + plen + 48;
    char *out = (char *)malloc(cap);
    if (!out) {
        zap_net_panic("http: out of memory");
        return NULL;
    }
    int n = snprintf(out, cap, "http://%.*s:%ld", (int)hlen, host->data, (long)port);
    if (plen == 0 || path->data[0] != '/') {
        n += snprintf(out + n, cap - (size_t)n, "/");
    }
    if (plen > 0) {
        memcpy(out + n, path->data, plen);
        n += (int)plen;
    }
    void *s = zap_str_new(out, (int64_t)n);
    free(out);
    return s;
}

/* http.read_request(conn) -> request handle. Reads headers up to the blank line,
 * then Content-Length body bytes (default 0). Parses the request line
 * "METHOD PATH HTTP/1.1". Keep-alive is not assumed; a client sending
 * Connection: close is handled the same way. Transport-agnostic (I#556): `c_` is
 * either a plaintext `net` conn or a TLS `tls.accept` conn, and every byte read
 * goes through the `zap_http_srv_fill` seam, so this single parser serves both.
 * The read deadline (I#485) is carried by the seam and fires over TLS too. The
 * whole-operation deadline (net.set_read_deadline, I#567) is computed ONCE here so
 * it bounds the ENTIRE request (headers + body) regardless of byte pacing — a
 * slow-dribble peer that keeps resetting the per-park clock is still cut off. */
void *zap_http_read_request(void *c_) {
    ZapDynBuf b = {NULL, 0, 0};
    size_t hlen = 0, body_off = 0;
    int have_headers = 0;
    int64_t rdl_ms = zap_conn_read_deadline_ms(c_);
    int64_t whole_deadline_ns = (rdl_ms > 0) ? zap_deadline_after_ms(rdl_ms) : 0;
    for (;;) {
        if (zap_http_split(&b, &hlen, &body_off) == 0) {
            have_headers = 1;
            break;
        }
        ssize_t r = zap_http_srv_fill(c_, &b, whole_deadline_ns);
        if (r == -2 || r == -3) {
            zap_dyn_free(&b);
            zap_io_panic((int)r, "http: read failed");
            return NULL;
        }
        if (r <= 0) {
            break;
        }
    }
    if (!have_headers) {
        zap_dyn_free(&b);
        zap_io_panic(-1, "http: incomplete request");
        return NULL;
    }

    /* request line */
    size_t i = 0;
    size_t mstart = 0;
    while (i < hlen && b.buf[i] != ' ') {
        i++;
    }
    size_t mend = i;
    while (i < hlen && b.buf[i] == ' ') {
        i++;
    }
    size_t pstart = i;
    while (i < hlen && b.buf[i] != ' ' && b.buf[i] != '\r' && b.buf[i] != '\n') {
        i++;
    }
    size_t pend = i;
    /* HTTP version token: skip the single space after the path, then read up to
     * CR/LF/space. Absent (HTTP/0.9-style request line) leaves an empty token. */
    size_t vcur = pend;
    while (vcur < hlen && b.buf[vcur] == ' ') {
        vcur++;
    }
    size_t vstart = vcur;
    while (vcur < hlen && b.buf[vcur] != ' ' && b.buf[vcur] != '\r' && b.buf[vcur] != '\n') {
        vcur++;
    }
    size_t vend = vcur;
    /* header block begins after the request line */
    size_t hstart = pend;
    while (hstart < hlen && b.buf[hstart] != '\n') {
        hstart++;
    }
    if (hstart < hlen) {
        hstart++;
    }

    /* read the body per Content-Length (relative to the header block) */
    long clen = zap_http_content_length(b.buf + hstart, hlen > hstart ? hlen - hstart : 0);
    if (clen < 0) {
        clen = 0;
    }
    while ((size_t)(b.len - body_off) < (size_t)clen) {
        ssize_t r = zap_http_srv_fill(c_, &b, whole_deadline_ns);
        if (r == -2 || r == -3) {
            zap_dyn_free(&b);
            zap_io_panic((int)r, "http: read failed");
            return NULL;
        }
        if (r <= 0) {
            break;
        }
    }
    size_t bodylen = b.len - body_off;
    if (bodylen > (size_t)clen) {
        bodylen = (size_t)clen;
    }

    void *method = zap_str_new(b.buf + mstart, (int64_t)(mend - mstart));
    void *path = zap_str_new(b.buf + pstart, (int64_t)(pend - pstart));
    void *headers = zap_str_new(b.buf + hstart, (int64_t)(hlen > hstart ? hlen - hstart : 0));
    void *bodyobj = zap_str_new(b.buf + body_off, (int64_t)bodylen);
    void *version = zap_str_new(b.buf + vstart, (int64_t)(vend - vstart));
    zap_dyn_free(&b);
    zap_check_cancel();

    ZapHttpReq *req = (ZapHttpReq *)zap_obj(&zap_desc_httpreq);
    req->method = (const ZapStr *)method;
    req->path = (const ZapStr *)path;
    req->headers = (const ZapStr *)headers;
    req->body = (const ZapStr *)bodyobj;
    req->version = (const ZapStr *)version;
    return req;
}

void *zap_http_req_method(void *req_) { return (void *)((ZapHttpReq *)req_)->method; }
void *zap_http_req_path(void *req_) { return (void *)((ZapHttpReq *)req_)->path; }
void *zap_http_req_body(void *req_) { return (void *)((ZapHttpReq *)req_)->body; }
void *zap_http_req_version(void *req_) { return (void *)((ZapHttpReq *)req_)->version; }

void *zap_http_req_header(void *req_, void *name_) {
    ZapHttpReq *req = (ZapHttpReq *)req_;
    char *nz = zap_net_dupz((const ZapStr *)name_);
    if (!nz) {
        return zap_str_new("", 0);
    }
    void *v = zap_http_header_value(req->headers->data, (size_t)req->headers->len, nz);
    free(nz);
    return v;
}

/* http.req_headers(req) enumeration backing (#960, D-0197). The raw request
 * header block is a GC `ZapStr` of the bytes between the request line and the
 * blank line; each header is one CR?LF-terminated `Name: value` line. These
 * three infallible accessors drive the `list[(str, str)]` the lowering
 * assembles: the count of header lines, and the name / value of the idx-th one
 * (0-based, in received order, duplicate names preserved as separate pairs).
 *
 * A header line is a non-empty line whose first `:` is preceded by at least one
 * byte (a non-empty field name); a blank or colon-less line is skipped. The
 * value is the bytes after the first `:` with leading spaces/tabs trimmed —
 * exactly the trimming `zap_http_header_value` applies to the by-name lookup.
 * Names are returned verbatim (original case); case-insensitive discovery is the
 * separate by-name `zap_http_req_header`.
 *
 * GC note: like `zap_http_header_value`, each call does all of its pointer
 * arithmetic over `req->headers->data` BEFORE its single `zap_str_new`, and the
 * caller re-derives the block pointer on every call, so no raw interior pointer
 * is ever held across an allocation (safe under a moving collector). */
static int zap_http_next_header_line(const char *hbuf, size_t hlen, size_t *i,
                                     size_t *line, size_t *colon, size_t *end) {
    if (*i >= hlen) {
        return 0;
    }
    *line = *i;
    while (*i < hlen && hbuf[*i] != '\n') {
        (*i)++;
    }
    size_t e = *i;
    if (e > *line && hbuf[e - 1] == '\r') {
        e--;
    }
    *end = e;
    size_t c = *line;
    while (c < e && hbuf[c] != ':') {
        c++;
    }
    *colon = c;
    if (*i < hlen) {
        (*i)++; /* step over the '\n' */
    }
    return 1;
}

/* A field line is one with a non-empty name (colon strictly after the start)
 * and a colon within the line. */
static int zap_http_is_field_line(size_t line, size_t colon, size_t end) {
    return colon < end && colon > line;
}

int64_t zap_http_req_header_count(void *req_) {
    const ZapHttpReq *req = (const ZapHttpReq *)req_;
    const char *hbuf = req->headers->data;
    size_t hlen = (size_t)req->headers->len;
    int64_t n = 0;
    size_t i = 0, line, colon, end;
    while (zap_http_next_header_line(hbuf, hlen, &i, &line, &colon, &end)) {
        if (zap_http_is_field_line(line, colon, end)) {
            n++;
        }
    }
    return n;
}

void *zap_http_req_header_name(void *req_, int64_t idx) {
    const ZapHttpReq *req = (const ZapHttpReq *)req_;
    const char *hbuf = req->headers->data;
    size_t hlen = (size_t)req->headers->len;
    int64_t k = 0;
    size_t i = 0, line, colon, end;
    while (zap_http_next_header_line(hbuf, hlen, &i, &line, &colon, &end)) {
        if (!zap_http_is_field_line(line, colon, end)) {
            continue;
        }
        if (k == idx) {
            return zap_str_new(hbuf + line, (int64_t)(colon - line));
        }
        k++;
    }
    return zap_str_new("", 0);
}

void *zap_http_req_header_val(void *req_, int64_t idx) {
    const ZapHttpReq *req = (const ZapHttpReq *)req_;
    const char *hbuf = req->headers->data;
    size_t hlen = (size_t)req->headers->len;
    int64_t k = 0;
    size_t i = 0, line, colon, end;
    while (zap_http_next_header_line(hbuf, hlen, &i, &line, &colon, &end)) {
        if (!zap_http_is_field_line(line, colon, end)) {
            continue;
        }
        if (k == idx) {
            size_t v = colon + 1;
            while (v < end && (hbuf[v] == ' ' || hbuf[v] == '\t')) {
                v++;
            }
            return zap_str_new(hbuf + v, (int64_t)(end - v));
        }
        k++;
    }
    return zap_str_new("", 0);
}

/* ---- client response header enumeration (#989) --------------------------- *
 * The response twin of `zap_http_req_header_{count,name,val}`: enumerate the
 * response header block (`ZapHttpResp.headers`, first-line-stripped) as it
 * arrived, duplicates preserved — so multiple `Set-Cookie` lines are each
 * visible. Backs `http.headers(resp)`; the list[(str,str)] is assembled in
 * lowering (the `req_headers` pattern). */
int64_t zap_http_resp_header_count(void *resp_) {
    const ZapHttpResp *r = (const ZapHttpResp *)resp_;
    const char *hbuf = r->headers->data;
    size_t hlen = (size_t)r->headers->len;
    int64_t n = 0;
    size_t i = 0, line, colon, end;
    while (zap_http_next_header_line(hbuf, hlen, &i, &line, &colon, &end)) {
        if (zap_http_is_field_line(line, colon, end)) {
            n++;
        }
    }
    return n;
}

void *zap_http_resp_header_name(void *resp_, int64_t idx) {
    const ZapHttpResp *r = (const ZapHttpResp *)resp_;
    const char *hbuf = r->headers->data;
    size_t hlen = (size_t)r->headers->len;
    int64_t k = 0;
    size_t i = 0, line, colon, end;
    while (zap_http_next_header_line(hbuf, hlen, &i, &line, &colon, &end)) {
        if (!zap_http_is_field_line(line, colon, end)) {
            continue;
        }
        if (k == idx) {
            return zap_str_new(hbuf + line, (int64_t)(colon - line));
        }
        k++;
    }
    return zap_str_new("", 0);
}

void *zap_http_resp_header_val(void *resp_, int64_t idx) {
    const ZapHttpResp *r = (const ZapHttpResp *)resp_;
    const char *hbuf = r->headers->data;
    size_t hlen = (size_t)r->headers->len;
    int64_t k = 0;
    size_t i = 0, line, colon, end;
    while (zap_http_next_header_line(hbuf, hlen, &i, &line, &colon, &end)) {
        if (!zap_http_is_field_line(line, colon, end)) {
            continue;
        }
        if (k == idx) {
            size_t v = colon + 1;
            while (v < end && (hbuf[v] == ' ' || hbuf[v] == '\t')) {
                v++;
            }
            return zap_str_new(hbuf + v, (int64_t)(end - v));
        }
        k++;
    }
    return zap_str_new("", 0);
}

/* ---- Set-Cookie parsing (#989) ------------------------------------------- *
 * Parse ONE `Set-Cookie` header value into its components (the value as read
 * from `http.header(resp, "Set-Cookie")` or one entry of `http.headers(resp)`).
 * The first `name=value` pair is the cookie; the remaining `;`-separated
 * attributes set Domain / Path / Expires / Max-Age / SameSite (values) and
 * Secure / HttpOnly (flags). Everything indexes into the input (no allocation
 * until the per-field getters build the GC strings). Backs the prelude
 * `http.parse_set_cookie(value) -> Cookie`; lowering builds the struct with one
 * getter per field (the `url.split` / `UrlParts` pattern). */
typedef struct {
    const char *name;
    size_t name_len;
    const char *value;
    size_t value_len;
    const char *domain;
    size_t domain_len;
    const char *path;
    size_t path_len;
    const char *expires;
    size_t expires_len;
    const char *same_site;
    size_t same_site_len;
    long max_age; /* -1 = absent */
    int secure;
    int http_only;
} ZapCookieFields;

/* Trim leading/trailing spaces+tabs of [*p, *p+*n). */
static void zap_cookie_trim(const char **p, size_t *n) {
    const char *s = *p;
    size_t len = *n;
    while (len > 0 && (s[0] == ' ' || s[0] == '\t')) {
        s++;
        len--;
    }
    while (len > 0 && (s[len - 1] == ' ' || s[len - 1] == '\t')) {
        len--;
    }
    *p = s;
    *n = len;
}

/* Case-insensitive compare of [p,p+n) against a NUL-terminated ASCII literal. */
static int zap_cookie_ci_eq(const char *p, size_t n, const char *lit) {
    if (n != strlen(lit)) {
        return 0;
    }
    for (size_t k = 0; k < n; k++) {
        char a = p[k];
        char b = lit[k];
        if (a >= 'A' && a <= 'Z') {
            a = (char)(a - 'A' + 'a');
        }
        if (b >= 'A' && b <= 'Z') {
            b = (char)(b - 'A' + 'a');
        }
        if (a != b) {
            return 0;
        }
    }
    return 1;
}

static void zap_http_parse_cookie_fields(const ZapStr *s, ZapCookieFields *f) {
    memset(f, 0, sizeof *f);
    f->name = "";
    f->value = "";
    f->domain = "";
    f->path = "";
    f->expires = "";
    f->same_site = "";
    f->max_age = -1;
    if (!s || s->len <= 0) {
        return;
    }
    const char *data = s->data;
    size_t len = (size_t)s->len;
    size_t pos = 0;
    int first = 1;
    while (pos < len) {
        size_t semi = pos;
        while (semi < len && data[semi] != ';') {
            semi++;
        }
        const char *seg = data + pos;
        size_t seglen = semi - pos;
        zap_cookie_trim(&seg, &seglen);
        /* split on the first '=' */
        size_t eq = 0;
        while (eq < seglen && seg[eq] != '=') {
            eq++;
        }
        const char *key = seg;
        size_t keylen = eq;
        const char *val = (eq < seglen) ? seg + eq + 1 : seg + seglen;
        size_t vallen = (eq < seglen) ? seglen - eq - 1 : 0;
        zap_cookie_trim(&key, &keylen);
        zap_cookie_trim(&val, &vallen);
        if (first) {
            f->name = key;
            f->name_len = keylen;
            f->value = val;
            f->value_len = vallen;
            first = 0;
        } else if (zap_cookie_ci_eq(key, keylen, "domain")) {
            f->domain = val;
            f->domain_len = vallen;
        } else if (zap_cookie_ci_eq(key, keylen, "path")) {
            f->path = val;
            f->path_len = vallen;
        } else if (zap_cookie_ci_eq(key, keylen, "expires")) {
            f->expires = val;
            f->expires_len = vallen;
        } else if (zap_cookie_ci_eq(key, keylen, "samesite")) {
            f->same_site = val;
            f->same_site_len = vallen;
        } else if (zap_cookie_ci_eq(key, keylen, "max-age")) {
            long m = 0;
            int any = 0, neg = 0;
            size_t k = 0;
            if (vallen > 0 && val[0] == '-') {
                neg = 1;
                k = 1;
            }
            for (; k < vallen; k++) {
                if (val[k] < '0' || val[k] > '9') {
                    any = 0;
                    break;
                }
                m = m * 10 + (val[k] - '0');
                any = 1;
            }
            f->max_age = any ? (neg ? -m : m) : -1;
        } else if (zap_cookie_ci_eq(key, keylen, "secure")) {
            f->secure = 1;
        } else if (zap_cookie_ci_eq(key, keylen, "httponly")) {
            f->http_only = 1;
        }
        pos = (semi < len) ? semi + 1 : len;
    }
}

void *zap_http_cookie_name(void *v) {
    ZapCookieFields f;
    zap_http_parse_cookie_fields((const ZapStr *)v, &f);
    return zap_str_new(f.name, (int64_t)f.name_len);
}
void *zap_http_cookie_value(void *v) {
    ZapCookieFields f;
    zap_http_parse_cookie_fields((const ZapStr *)v, &f);
    return zap_str_new(f.value, (int64_t)f.value_len);
}
void *zap_http_cookie_domain(void *v) {
    ZapCookieFields f;
    zap_http_parse_cookie_fields((const ZapStr *)v, &f);
    return zap_str_new(f.domain, (int64_t)f.domain_len);
}
void *zap_http_cookie_path(void *v) {
    ZapCookieFields f;
    zap_http_parse_cookie_fields((const ZapStr *)v, &f);
    return zap_str_new(f.path, (int64_t)f.path_len);
}
void *zap_http_cookie_expires(void *v) {
    ZapCookieFields f;
    zap_http_parse_cookie_fields((const ZapStr *)v, &f);
    return zap_str_new(f.expires, (int64_t)f.expires_len);
}
void *zap_http_cookie_same_site(void *v) {
    ZapCookieFields f;
    zap_http_parse_cookie_fields((const ZapStr *)v, &f);
    return zap_str_new(f.same_site, (int64_t)f.same_site_len);
}
int64_t zap_http_cookie_max_age(void *v) {
    ZapCookieFields f;
    zap_http_parse_cookie_fields((const ZapStr *)v, &f);
    return (int64_t)f.max_age;
}
int64_t zap_http_cookie_secure(void *v) {
    ZapCookieFields f;
    zap_http_parse_cookie_fields((const ZapStr *)v, &f);
    return (int64_t)f.secure;
}
int64_t zap_http_cookie_http_only(void *v) {
    ZapCookieFields f;
    zap_http_parse_cookie_fields((const ZapStr *)v, &f);
    return (int64_t)f.http_only;
}

/* Reason phrase for a status code (a small common set; "OK" otherwise). */
static const char *zap_http_reason(int64_t status) {
    switch (status) {
    case 200: return "OK";
    case 201: return "Created";
    case 202: return "Accepted";
    case 204: return "No Content";
    case 301: return "Moved Permanently";
    case 302: return "Found";
    case 304: return "Not Modified";
    case 400: return "Bad Request";
    case 401: return "Unauthorized";
    case 403: return "Forbidden";
    case 404: return "Not Found";
    case 405: return "Method Not Allowed";
    case 409: return "Conflict";
    case 500: return "Internal Server Error";
    case 501: return "Not Implemented";
    case 503: return "Service Unavailable";
    default: return "OK";
    }
}

/* Response headers always computed by the server and never taken from the
 * caller's map, so exactly one of each appears on the wire. Content-Type is
 * NOT reserved: a caller entry overrides the text/plain default (mirrors the
 * client's zap_http_hdr_managed, minus the request-only Host). */
static int zap_http_resp_hdr_managed(const ZapStr *name) {
    return zap_http_name_eq(name, "Content-Length") || zap_http_name_eq(name, "Connection");
}

/* Core of http.respond. Writes an HTTP/1.1 response: the status line, a
 * server-managed Content-Length and Connection: close, a default
 * Content-Type: text/plain unless the caller overrides it, then every live
 * caller header (reserved names dropped). `headers_` is a ZapDict* (#279) or
 * NULL for the bare three-arg form. Header names/values are copied out of the
 * GC dict into the malloc buffer before any send, so the dict is only touched
 * while it is still rooted by the caller's frame. */
static void zap_http_respond_do(void *c_, int64_t status, void *body_, void *headers_) {
    const ZapStr *body = (const ZapStr *)body_;
    const ZapDict *hdrs = (const ZapDict *)headers_;
    size_t blen = (size_t)(body->len < 0 ? 0 : body->len);

    /* Size the buffer: status/Content-Length/Content-Type/Connection lines +
     * body + the caller's header map (name + ": " + value + CRLF per entry). */
    size_t hdrbytes = 0;
    int caller_content_type = 0;
    if (hdrs) {
        for (int64_t e = 0; e < hdrs->cap; e++) {
            if (zap_d_tags(hdrs)[e] < 2) { /* skip empty / tombstone slots (#840) */
                continue;
            }
            const ZapStr *name = (const ZapStr *)(intptr_t)zap_d_keys(hdrs)[e];
            const ZapStr *val = (const ZapStr *)(intptr_t)zap_d_vals(hdrs)[e];
            if (zap_http_resp_hdr_managed(name)) {
                continue;
            }
            if (zap_http_name_eq(name, "Content-Type")) {
                caller_content_type = 1;
            }
            size_t nl = (size_t)(name->len < 0 ? 0 : name->len);
            size_t vl = (size_t)(val->len < 0 ? 0 : val->len);
            hdrbytes += nl + vl + 4; /* ": " + "\r\n" */
        }
    }
    size_t cap = blen + hdrbytes + 256;
    char *out = (char *)malloc(cap);
    if (!out) {
        zap_net_panic("http: out of memory");
        return;
    }
    int hdr = snprintf(out, cap, "HTTP/1.1 %ld %s\r\nContent-Length: %zu\r\n", (long)status,
                       zap_http_reason(status), blen);
    if (!caller_content_type) {
        hdr += snprintf(out + hdr, cap - (size_t)hdr,
                        "Content-Type: text/plain; charset=utf-8\r\n");
    }
    hdr += snprintf(out + hdr, cap - (size_t)hdr, "Connection: close\r\n");
    /* Caller-supplied headers, copied out of the GC dict into the buffer. */
    if (hdrs) {
        for (int64_t e = 0; e < hdrs->cap; e++) {
            if (zap_d_tags(hdrs)[e] < 2) { /* skip empty / tombstone slots (#840) */
                continue;
            }
            const ZapStr *name = (const ZapStr *)(intptr_t)zap_d_keys(hdrs)[e];
            const ZapStr *val = (const ZapStr *)(intptr_t)zap_d_vals(hdrs)[e];
            if (zap_http_resp_hdr_managed(name)) {
                continue;
            }
            size_t nl = (size_t)(name->len < 0 ? 0 : name->len);
            size_t vl = (size_t)(val->len < 0 ? 0 : val->len);
            hdr += snprintf(out + hdr, cap - (size_t)hdr, "%.*s: %.*s\r\n", (int)nl, name->data,
                            (int)vl, val->data);
        }
    }
    hdr += snprintf(out + hdr, cap - (size_t)hdr, "\r\n");
    if (blen > 0) {
        memcpy(out + hdr, body->data, blen);
    }
    /* I#556: dispatch the write over the transport seam so a response is emitted
     * over a TLS conn (SSL_write) just as over a plaintext one (send). */
    int w = zap_http_srv_send_all(c_, out, (size_t)hdr + blen);
    free(out);
    if (w < 0) {
        zap_io_panic(w, "http: respond failed");
    }
}

/* http.respond(conn, status, body). Writes a minimal HTTP/1.1 response with
 * Content-Length, a text/plain content type, and Connection: close. */
void zap_http_respond(void *c_, int64_t status, void *body_) {
    zap_http_respond_do(c_, status, body_, NULL);
}

/* http.respond(conn, status, body, headers) — the headers-bearing form (#279).
 * `headers` is a dict[str,str] merged with the server-managed set: the caller
 * may override Content-Type, Content-Length and Connection stay server-owned. */
void zap_http_respond_h(void *c_, int64_t status, void *body_, void *headers_) {
    zap_http_respond_do(c_, status, body_, headers_);
}
#endif /* !ZAP_NO_OPENSSL — end TLS transport + http client/server (#853) */

