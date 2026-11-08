//! `crypto` module — runtime-symbol declarations (ticket #105, D-0056).
//!
//! Hashing (SHA-256/512, SHA-1, MD5), HMAC and secure random / secrets. The C
//! implementations (self-contained FIPS/RFC reference hashes + an HMAC wrapper +
//! a getentropy/`/dev/urandom` CSPRNG) live in the crypto section of `zaprt.c`.
//! All `str`/`bytes` values are `ZapStr*` pointers:
//!   - one-shot `zap_crypto_<algo>(data) -> str` (hex)
//!   - streaming `zap_crypto_new(algo) -> handle`, `zap_crypto_update(handle,
//!     data)`, `zap_crypto_hexdigest(handle) -> str`, `zap_crypto_digest(handle)
//!     -> bytes`
//!   - `zap_crypto_hmac_sha256/512(key, msg) -> str` (hex)
//!   - `zap_crypto_hmac_sha256_raw(key, msg) -> bytes` (raw digest, #153)
//!   - `zap_crypto_constant_time_eq(a, b) -> i64` (0/1), OpenSSL CRYPTO_memcmp
//!     (#729) — content-independent timing for equal-length inputs
//!   - `zap_crypto_rsa_sha1_sign/sha256_sign(pem_key, data) -> bytes` (raw PKCS#1
//!     v1.5 signature, #628) and `zap_crypto_rsa_sha256_verify(pem_pubkey, data,
//!     sig) -> i64` (0/1) — the only OpenSSL-backed crypto entries
//!   - `zap_crypto_random_bytes(n) -> bytes`, `zap_crypto_token_hex(n) -> str`

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // One-shot hashing: ZapStr* data -> ZapStr* hex digest.
    cg.declare_rt("zap_crypto_sha256", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_sha512", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_sha1", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_md5", &[p.into()], Some(p.into()));
    // SHA-384 / SHA-224 one-shot hex (#765): truncated SHA-512 / SHA-256.
    cg.declare_rt("zap_crypto_sha384", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_sha224", &[p.into()], Some(p.into()));
    // Streaming context handle.
    cg.declare_rt("zap_crypto_new", &[i.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_update", &[p.into(), p.into()], None);
    cg.declare_rt("zap_crypto_hexdigest", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_digest", &[p.into()], Some(p.into()));
    // HMAC: (key, msg) -> hex digest.
    cg.declare_rt(
        "zap_crypto_hmac_sha256",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_hmac_sha512",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    // Raw-bytes HMAC-SHA256: (key, msg) -> bytes (the 32-byte digest, #153).
    cg.declare_rt(
        "zap_crypto_hmac_sha256_raw",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    // Constant-time byte comparison (#729, D-0131): (a, b) -> i64 (0/1). Backed
    // by OpenSSL CRYPTO_memcmp; timing is independent of byte contents for
    // equal-length inputs. A length difference returns 0 and is observable.
    cg.declare_rt(
        "zap_crypto_constant_time_eq",
        &[p.into(), p.into()],
        Some(i.into()),
    );
    // RSA PKCS#1 v1.5 signing (OpenSSL-backed, #628): (pem_key, data) -> bytes.
    cg.declare_rt(
        "zap_crypto_rsa_sha1_sign",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_rsa_sha256_sign",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    // RSA-SHA256 verify: (pem_pubkey, data, sig) -> i64 (0/1).
    cg.declare_rt(
        "zap_crypto_rsa_sha256_verify",
        &[p.into(), p.into(), p.into()],
        Some(i.into()),
    );
    // Asymmetric key metadata / import + P-256 ECDSA + Ed25519 (#643, D-0126),
    // OpenSSL-backed. key_info getters: kind/curve -> ZapStr*, bits/is_private ->
    // i64; all over a PEM key (kind is the validating gate, raising ValueError).
    cg.declare_rt("zap_crypto_key_kind", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_key_bits", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_crypto_key_curve", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_key_is_private", &[p.into()], Some(i.into()));
    // Validated public-key import from JWK components -> SPKI PEM bytes.
    cg.declare_rt(
        "zap_crypto_rsa_public_key_pem",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_p256_public_key_pem",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_ed25519_public_key_pem",
        &[p.into()],
        Some(p.into()),
    );
    // ES256 (P-256 / SHA-256), JOSE 64-byte r||s: (private_pem, msg) -> bytes.
    cg.declare_rt(
        "zap_crypto_p256_sha256_sign",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    // (public_pem, msg, sig) -> i64 (0/1).
    cg.declare_rt(
        "zap_crypto_p256_sha256_verify",
        &[p.into(), p.into(), p.into()],
        Some(i.into()),
    );
    // Pure Ed25519: (private_pem, msg) -> bytes; (public_pem, msg, sig) -> i64.
    cg.declare_rt(
        "zap_crypto_ed25519_sign",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_ed25519_verify",
        &[p.into(), p.into(), p.into()],
        Some(i.into()),
    );
    // Key derivation functions (#762, #763, D-0132), OpenSSL-backed.
    // pbkdf2_hmac(hash_name, password, salt, iterations, dklen) -> bytes.
    cg.declare_rt(
        "zap_crypto_pbkdf2_hmac",
        &[p.into(), p.into(), p.into(), i.into(), i.into()],
        Some(p.into()),
    );
    // hkdf(hash_name, ikm, length, salt, info) -> bytes (RFC 5869).
    cg.declare_rt(
        "zap_crypto_hkdf",
        &[p.into(), p.into(), i.into(), p.into(), p.into()],
        Some(p.into()),
    );
    // scrypt(password, salt, n, r, p, dklen) -> bytes (RFC 7914, #775).
    cg.declare_rt(
        "zap_crypto_scrypt",
        &[p.into(), p.into(), i.into(), i.into(), i.into(), i.into()],
        Some(p.into()),
    );
    // AEAD (#772): <alg>_encrypt/_decrypt(key, nonce, data, aad) -> bytes.
    for sym in [
        "zap_crypto_aes256gcm_encrypt",
        "zap_crypto_aes256gcm_decrypt",
        "zap_crypto_chacha20poly1305_encrypt",
        "zap_crypto_chacha20poly1305_decrypt",
    ] {
        cg.declare_rt(
            sym,
            &[p.into(), p.into(), p.into(), p.into()],
            Some(p.into()),
        );
    }
    // Secure random / secrets.
    cg.declare_rt("zap_crypto_random_bytes", &[i.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_token_hex", &[i.into()], Some(p.into()));
    // token_urlsafe(nbytes) -> str: base64url, unpadded (#764).
    cg.declare_rt("zap_crypto_token_urlsafe", &[i.into()], Some(p.into()));
    // randbelow(n) -> int: unbiased [0, n) (#767).
    cg.declare_rt("zap_crypto_randbelow", &[i.into()], Some(i.into()));
    // crypto.choice (#767): random valid index into a list (ptr) -> i64. The
    // typed element getter (zap_list_get_*) is declared centrally.
    cg.declare_rt("zap_crypto_list_index", &[p.into()], Some(i.into()));
    // Streaming HMAC (#766): new(hash_name, key) -> handle; update(h, data);
    // hexdigest(h) -> str; digest(h) -> bytes.
    cg.declare_rt("zap_crypto_hmac_new", &[p.into(), p.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_hmac_update", &[p.into(), p.into()], None);
    cg.declare_rt("zap_crypto_hmac_hexdigest", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_hmac_digest", &[p.into()], Some(p.into()));
    // SHA-3 / BLAKE2 one-shot hex (#770), OpenSSL-backed: data -> hex str.
    cg.declare_rt("zap_crypto_sha3_256", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_sha3_512", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_blake2b", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_blake2s", &[p.into()], Some(p.into()));
    // RSA PKCS#1 v1.5 SHA-384/512 and RSA-PSS (PS256/384/512) (#768). Sign:
    // (pem, data) -> bytes. Verify: (pem, data, sig) -> i64 (0/1).
    cg.declare_rt(
        "zap_crypto_rsa_sha384_sign",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_rsa_sha512_sign",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_rsa_sha384_verify",
        &[p.into(), p.into(), p.into()],
        Some(i.into()),
    );
    cg.declare_rt(
        "zap_crypto_rsa_sha512_verify",
        &[p.into(), p.into(), p.into()],
        Some(i.into()),
    );
    cg.declare_rt(
        "zap_crypto_rsa_pss_sha256_sign",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_rsa_pss_sha384_sign",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_rsa_pss_sha512_sign",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_rsa_pss_sha256_verify",
        &[p.into(), p.into(), p.into()],
        Some(i.into()),
    );
    cg.declare_rt(
        "zap_crypto_rsa_pss_sha384_verify",
        &[p.into(), p.into(), p.into()],
        Some(i.into()),
    );
    cg.declare_rt(
        "zap_crypto_rsa_pss_sha512_verify",
        &[p.into(), p.into(), p.into()],
        Some(i.into()),
    );
    // Additional JOSE ECDSA curves P-384 (ES384) / P-521 (ES512) (#769).
    cg.declare_rt(
        "zap_crypto_p384_public_key_pem",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_p521_public_key_pem",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_p384_sha384_sign",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_p384_sha384_verify",
        &[p.into(), p.into(), p.into()],
        Some(i.into()),
    );
    cg.declare_rt(
        "zap_crypto_p521_sha512_sign",
        &[p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_crypto_p521_sha512_verify",
        &[p.into(), p.into(), p.into()],
        Some(i.into()),
    );
}
