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
    // Secure random / secrets.
    cg.declare_rt("zap_crypto_random_bytes", &[i.into()], Some(p.into()));
    cg.declare_rt("zap_crypto_token_hex", &[i.into()], Some(p.into()));
}
