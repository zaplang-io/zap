//! `crypto` module — sema facet (ticket #105, D-0056).
//!
//! Hashing (`sha256`/`sha512`/`sha1`/`md5`), HMAC and secure random / secrets.
//! TLS is explicitly deferred (late-v1). Surface:
//!
//! One-shot hashing over `str`/`bytes`, returning the lowercase hex digest:
//!   `crypto.sha256(data) -> str`, and `sha512`/`sha1`/`md5` likewise.
//!
//! Streaming hashing — a `_new` returns an opaque `bytes` handle whose buffer
//! holds the hash context; feed it incrementally and finalize (non-destructive,
//! so a handle may be digested more than once):
//!   `crypto.sha256_new() -> bytes` (and `sha512`/`sha1`/`md5`)
//!   `crypto.update(h, data)` ; `crypto.hexdigest(h) -> str` ; `crypto.digest(h) -> bytes`
//!
//! HMAC over (key, message), returning the lowercase hex digest:
//!   `crypto.hmac_sha256(key, msg) -> str` ; `crypto.hmac_sha512(key, msg) -> str`
//! The raw-bytes HMAC-SHA256 digest (for chaining, e.g. AWS SigV4, #153):
//!   `crypto.hmac_sha256_raw(key, msg) -> bytes`
//!
//! Constant-time byte comparison (#729, D-0131) — timing independent of the
//! byte *contents* for equal-length inputs; a length difference is observable:
//!   `crypto.constant_time_eq(a, b) -> bool`
//!
//! RSA PKCS#1 v1.5 signing over a PEM-encoded private key (PKCS#1 or PKCS#8),
//! returning the RAW signature bytes (for CloudFront signed URLs, #628):
//!   `crypto.rsa_sha1_sign(pem_key, data) -> bytes`
//!   `crypto.rsa_sha256_sign(pem_key, data) -> bytes`
//! Verification against a PEM-encoded public key:
//!   `crypto.rsa_sha256_verify(pem_pubkey, data, sig) -> bool`
//!
//! Secure random from the OS CSPRNG (getentropy / /dev/urandom):
//!   `crypto.random_bytes(n) -> bytes` ; `crypto.token_hex(n) -> str`
//!   `crypto.randbelow(n) -> int` (unbiased `[0, n)`) ;
//!   `crypto.choice(seq: list[T]) -> T` (element-generic uniform pick; an empty
//!     list raises `ValueError`)
//!
//! Validated asymmetric keys — metadata, JWK-component import, and P-256 ECDSA
//! (ES256) / Ed25519 sign+verify (#643, D-0126). All free functions over PEM /
//! raw components; no raw OpenSSL state is exposed:
//!   `crypto.key_info(pem) -> KeyInfo` — the built-in `KeyInfo` value struct
//!     `{ str kind; int64 bits; str curve; bool is_private }`. `kind` is
//!     `"RSA"`/`"EC"`/`"Ed25519"`; `bits` is the RSA modulus size (or 256 for
//!     P-256/Ed25519); `curve` is `"P-256"` for EC (else `""`). Raises
//!     `ValueError` on an unparseable / unsupported / encrypted key.
//!   `crypto.rsa_public_key_pem(n, e) -> bytes`,
//!   `crypto.p256_public_key_pem(x, y) -> bytes`,
//!   `crypto.ed25519_public_key_pem(x) -> bytes` — validated SubjectPublicKeyInfo
//!     PEM from JWK integer/octet components (P-256 coords are 32 bytes and must
//!     lie on the curve; Ed25519 keys are 32 bytes).
//!   `crypto.p256_sha256_sign(private_pem, msg) -> bytes` /
//!   `crypto.p256_sha256_verify(public_pem, msg, sig) -> bool` — ES256; the
//!     signature is the JOSE fixed-width 64-byte `r||s` (DER is never exposed).
//!   `crypto.ed25519_sign(private_pem, msg) -> bytes` /
//!   `crypto.ed25519_verify(public_pem, msg, sig) -> bool` — pure Ed25519 over
//!     the original message (RFC 8032); 64-byte signatures.
//! Signing requires a private key of the exact family (a public key / wrong
//! family / wrong curve raises `ValueError`); verification returns `false` on any
//! mismatch or tamper, never a panic.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{assignable, expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "crypto",
    check,
};

/// A `str`/`bytes` value is required for data/key/message arguments (both share
/// the `ZapStr` runtime layout). Infers the argument so it is still type-checked.
fn expect_data(checker: &mut Checker, func: &str, arg: &Arg) {
    let t = checker.infer(&arg.value);
    if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
        checker.error(
            ErrorCode::ZAP4024,
            format!(
                "{func}() expects a str or bytes argument, found {}",
                t.display()
            ),
            expr_span(&arg.value),
        );
    }
}

/// An integer-valued argument (counts: iterations / dklen / length / nbytes).
/// Infers the argument so it is still type-checked, and lowering coerces to i64.
fn expect_int(checker: &mut Checker, func: &str, arg: &Arg) {
    let t = checker.infer(&arg.value);
    if !matches!(t, Ty::Int32 | Ty::Int64 | Ty::UntypedInt | Ty::Unknown) {
        checker.error(
            ErrorCode::ZAP4024,
            format!("{func}() expects an int argument, found {}", t.display()),
            expr_span(&arg.value),
        );
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // One-shot hashing: crypto.<algo>(data: str|bytes) -> str (hex digest).
        // sha384/sha224 (#765) complete the SHA-2 family (truncated SHA-512/256);
        // sha3_256/sha3_512/blake2b/blake2s (#770) are the OpenSSL-backed SHA-3
        // and BLAKE2 families. crypto.hex(data) (#771) is the general lowercase
        // hex encoder (like Python bytes.hex()), the same str result shape.
        "sha256" | "sha512" | "sha1" | "md5" | "sha384" | "sha224" | "sha3_256" | "sha3_512"
        | "blake2b" | "blake2s" | "hex" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(data) takes 1 argument, found {}",
                        args.len()
                    ),
                    span,
                );
            } else {
                expect_data(checker, method, &args[0]);
            }
            Some(Ty::Str)
        }
        // Streaming: crypto.<algo>_new() -> bytes (an opaque context handle).
        "sha256_new" | "sha512_new" | "sha1_new" | "md5_new" | "sha384_new" | "sha224_new" => {
            expect_argc(checker, "crypto", method, args, 0, span);
            Some(Ty::Bytes)
        }
        // crypto.update(handle, data): feed more input into a streaming handle.
        "update" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.update(h, data) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                checker.infer(&args[0].value);
                expect_data(checker, "update", &args[1]);
            }
            Some(Ty::Unit)
        }
        // crypto.hexdigest(handle) -> str: finalize to a lowercase hex digest.
        "hexdigest" => {
            expect_argc(checker, "crypto", "hexdigest", args, 1, span);
            Some(Ty::Str)
        }
        // crypto.digest(handle) -> bytes: finalize to the raw digest bytes.
        "digest" => {
            expect_argc(checker, "crypto", "digest", args, 1, span);
            Some(Ty::Bytes)
        }
        // HMAC: crypto.hmac_<algo>(key, msg) -> str (hex digest).
        "hmac_sha256" | "hmac_sha512" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(key, msg) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(Ty::Str)
        }
        // Raw-bytes HMAC-SHA256: crypto.hmac_sha256_raw(key, msg) -> bytes (the
        // 32-byte digest). Lets a digest be fed as the next step's key without a
        // lossy hex round-trip (AWS SigV4 signing-key derivation, #153).
        "hmac_sha256_raw" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(key, msg) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(Ty::Bytes)
        }
        // crypto.constant_time_eq(a, b) -> bool (#729, D-0131): true iff the two
        // byte sequences are equal, compared in time independent of their byte
        // contents (OpenSSL CRYPTO_memcmp) for equal-length inputs. A length
        // difference returns false and is observable; no branch is taken on byte
        // contents. Both args are str|bytes; empty inputs are valid.
        "constant_time_eq" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(a, b) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(Ty::Bool)
        }
        // RSA PKCS#1 v1.5 signing: crypto.rsa_<hash>_sign(pem_key, data) -> bytes
        // (the raw signature over the hash of `data`). The PEM private key may be
        // PKCS#1 (`BEGIN RSA PRIVATE KEY`) or PKCS#8 (`BEGIN PRIVATE KEY`). The
        // CloudFront base64 variant is applied by the caller, not here (#628).
        // Added (#768): RSA PKCS#1 v1.5 SHA-384/512 (RS384/RS512) and RSA-PSS
        // (PS256/384/512) signing — all (pem_key, data) -> bytes.
        "rsa_sha1_sign"
        | "rsa_sha256_sign"
        | "rsa_sha384_sign"
        | "rsa_sha512_sign"
        | "rsa_pss_sha256_sign"
        | "rsa_pss_sha384_sign"
        | "rsa_pss_sha512_sign" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(pem_key, data) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(Ty::Bytes)
        }
        // crypto.rsa_sha256_verify(pem_pubkey, data, sig) -> bool: true iff `sig`
        // is a valid RSA-SHA256 PKCS#1 v1.5 signature over `data` (#628).
        // Added (#768): matching RSA SHA-384/512 and RSA-PSS verifiers —
        // (pem_pubkey, data, sig) -> bool.
        "rsa_sha256_verify"
        | "rsa_sha384_verify"
        | "rsa_sha512_verify"
        | "rsa_pss_sha256_verify"
        | "rsa_pss_sha384_verify"
        | "rsa_pss_sha512_verify" => {
            if args.len() != 3 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(pem_pubkey, data, sig) takes 3 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
                expect_data(checker, method, &args[2]);
            }
            Some(Ty::Bool)
        }
        // crypto.key_info(pem) -> KeyInfo (#643, D-0126): validated metadata for a
        // supported PEM/DER asymmetric key. Raises ValueError on an unparseable,
        // unsupported, or encrypted key.
        "key_info" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.key_info(pem) takes 1 argument, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
            }
            Some(Ty::Named("KeyInfo".to_string()))
        }
        // crypto.rsa_public_key_pem(n_bytes, e_bytes) -> bytes (#643): build a
        // validated SPKI PEM public key from the RSA JWK integer components.
        // crypto.p256_public_key_pem(x_bytes, y_bytes) -> bytes: P-256 affine
        // coordinates (32 bytes each), validated to lie on the curve.
        "rsa_public_key_pem"
        | "p256_public_key_pem"
        | "p384_public_key_pem"
        | "p521_public_key_pem" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(a, b) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(Ty::Bytes)
        }
        // crypto.ed25519_public_key_pem(x_bytes) -> bytes (#643): build a
        // validated SPKI PEM public key from the 32-byte Ed25519 public key.
        "ed25519_public_key_pem" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(x_bytes) takes 1 argument, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
            }
            Some(Ty::Bytes)
        }
        // crypto.p256_sha256_sign(private_pem, msg) -> bytes (#643, ES256): the
        // 64-byte JOSE r||s signature. crypto.ed25519_sign(private_pem, msg) ->
        // bytes: the 64-byte pure-Ed25519 signature over the original message.
        "p256_sha256_sign" | "ed25519_sign" | "p384_sha384_sign" | "p521_sha512_sign" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(private_pem, msg) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(Ty::Bytes)
        }
        // crypto.p256_sha256_verify(public_pem, msg, sig) -> bool (#643): true iff
        // `sig` is a valid 64-byte JOSE ES256 signature. crypto.ed25519_verify
        // (public_pem, msg, sig) -> bool: pure-Ed25519 verification. A bad key or
        // any verification failure is a plain `false`, never a panic.
        "p256_sha256_verify" | "ed25519_verify" | "p384_sha384_verify" | "p521_sha512_verify" => {
            if args.len() != 3 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(public_pem, msg, sig) takes 3 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
                expect_data(checker, method, &args[2]);
            }
            Some(Ty::Bool)
        }
        // crypto.pbkdf2_hmac(hash_name, password, salt, iterations, dklen=0)
        // -> bytes (#762): PBKDF2-HMAC password-based key derivation, matching
        // Python hashlib.pbkdf2_hmac. `dklen` is optional (0 ⇒ native digest
        // length). hash_name is a str; password/salt are str|bytes; iterations
        // and dklen are ints. The runtime raises ValueError on bad arguments or
        // an unknown hash_name.
        "pbkdf2_hmac" => {
            if args.len() != 4 && args.len() != 5 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(hash_name, password, salt, iterations, dklen=0) takes 4 or 5 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
                expect_data(checker, method, &args[2]);
                expect_int(checker, method, &args[3]);
                if let Some(dklen) = args.get(4) {
                    expect_int(checker, method, dklen);
                }
            }
            Some(Ty::Bytes)
        }
        // crypto.scrypt(password, salt, n, r, p, dklen=64) -> bytes (#775,
        // D-0134): scrypt password-based KDF (RFC 7914), mirroring Python
        // hashlib.scrypt. password/salt are str|bytes; n/r/p/dklen are ints
        // (dklen optional, defaults to 64). The runtime raises ValueError on a
        // bad parameter (n not a power of two > 1, non-positive r/p/dklen) or a
        // derivation failure.
        "scrypt" => {
            if args.len() != 5 && args.len() != 6 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(password, salt, n, r, p, dklen=64) takes 5 or 6 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
                expect_int(checker, method, &args[2]);
                expect_int(checker, method, &args[3]);
                expect_int(checker, method, &args[4]);
                if let Some(dklen) = args.get(5) {
                    expect_int(checker, method, dklen);
                }
            }
            Some(Ty::Bytes)
        }
        // AEAD symmetric encryption (#772, D-0135): AES-256-GCM and
        // ChaCha20-Poly1305. crypto.<alg>_encrypt(key, nonce, plaintext, aad="")
        // -> bytes (ciphertext||tag); crypto.<alg>_decrypt(key, nonce,
        // ciphertext, aad="") -> bytes (raises ValueError on a bad key/nonce
        // length or an authentication failure). Key is 32 bytes, nonce 12, tag
        // 16; all data args are str|bytes, aad optional.
        "aes256gcm_encrypt"
        | "aes256gcm_decrypt"
        | "chacha20poly1305_encrypt"
        | "chacha20poly1305_decrypt" => {
            if args.len() != 3 && args.len() != 4 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(key, nonce, data, aad=\"\") takes 3 or 4 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
                expect_data(checker, method, &args[2]);
                if let Some(aad) = args.get(3) {
                    expect_data(checker, method, aad);
                }
            }
            Some(Ty::Bytes)
        }
        // crypto.hkdf(hash_name, ikm, length, salt="", info="") -> bytes (#763,
        // RFC 5869 extract-and-expand). hash_name is a str; ikm/salt/info are
        // str|bytes (salt and info optional); length is an int. The runtime
        // raises ValueError on length <= 0, length > 255*HashLen, or an unknown
        // hash_name.
        "hkdf" => {
            if !(3..=5).contains(&args.len()) {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(hash_name, ikm, length, salt=\"\", info=\"\") takes 3 to 5 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
                expect_int(checker, method, &args[2]);
                if let Some(salt) = args.get(3) {
                    expect_data(checker, method, salt);
                }
                if let Some(info) = args.get(4) {
                    expect_data(checker, method, info);
                }
            }
            Some(Ty::Bytes)
        }
        // crypto.token_urlsafe(nbytes) -> str (#764): nbytes of CSPRNG randomness,
        // base64url-encoded without padding (mirrors Python secrets.token_urlsafe).
        // The runtime raises ValueError on a negative nbytes.
        "token_urlsafe" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.token_urlsafe(nbytes) takes 1 argument, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_int(checker, method, &args[0]);
            }
            Some(Ty::Str)
        }
        // Encoding helpers (#771): crypto.unhex(s) and crypto.b64url_decode(s)
        // decode text to bytes; crypto.b64url_encode(data) encodes bytes to an
        // unpadded URL-safe base64 str. (crypto.hex is folded into the one-shot
        // str-returning arm above.) All take a single str|bytes argument.
        "unhex" | "b64url_decode" => {
            expect_argc(checker, "crypto", method, args, 1, span);
            if let Some(a) = args.first() {
                expect_data(checker, method, a);
            }
            Some(Ty::Bytes)
        }
        "b64url_encode" => {
            expect_argc(checker, "crypto", method, args, 1, span);
            if let Some(a) = args.first() {
                expect_data(checker, method, a);
            }
            Some(Ty::Str)
        }
        // Streaming HMAC (#766), mirroring the streaming-hash handle surface.
        // crypto.hmac_new(hash_name, key) -> bytes (an opaque context handle).
        "hmac_new" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.hmac_new(hash_name, key) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(Ty::Bytes)
        }
        // crypto.hmac_update(handle, data): feed more input into a streaming HMAC.
        "hmac_update" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.hmac_update(h, data) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                checker.infer(&args[0].value);
                expect_data(checker, "hmac_update", &args[1]);
            }
            Some(Ty::Unit)
        }
        // crypto.hmac_hexdigest(handle) -> str / crypto.hmac_digest(handle) -> bytes.
        "hmac_hexdigest" => {
            expect_argc(checker, "crypto", "hmac_hexdigest", args, 1, span);
            Some(Ty::Str)
        }
        "hmac_digest" => {
            expect_argc(checker, "crypto", "hmac_digest", args, 1, span);
            Some(Ty::Bytes)
        }
        // crypto.randbelow(n) -> int (#767): uniform in [0, n); n must be int.
        "randbelow" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!("crypto.randbelow(n) takes 1 argument, found {}", args.len()),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_int(checker, method, &args[0]);
            }
            Some(Ty::Int64)
        }
        // crypto.choice(seq) -> a uniformly random element of the list (#767),
        // drawn from the OS CSPRNG (mirrors Python secrets.choice, and the
        // surface of random.choice). Element-generic: yields the list's element
        // type `T`. An empty list raises a catchable ValueError at runtime.
        "choice" => {
            let ty = require_list(checker, "choice", args, span);
            Some(ty)
        }
        // crypto.random_bytes(n) -> bytes: n cryptographically secure random bytes.
        "random_bytes" => {
            expect_argc(checker, "crypto", "random_bytes", args, 1, span);
            Some(Ty::Bytes)
        }
        // crypto.token_hex(n) -> str: n secure random bytes as 2n lowercase hex chars.
        "token_hex" => {
            expect_argc(checker, "crypto", "token_hex", args, 1, span);
            Some(Ty::Str)
        }
        _ => None,
    }
}

/// Require a single `list[T]` argument, returning its element type `T` (or
/// `Unknown` on error). Backs `crypto.choice`, which yields `T` (an element
/// drawn with the CSPRNG). Mirrors the `random.choice` checker.
fn require_list(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Ty {
    if args.len() != 1 {
        checker.error(
            ErrorCode::ZAP4025,
            format!(
                "crypto.{method}(seq) takes 1 argument, found {}",
                args.len()
            ),
            span,
        );
        for a in args {
            checker.infer(&a.value);
        }
        return Ty::Unknown;
    }
    match checker.infer(&args[0].value) {
        Ty::List(elem) => *elem,
        Ty::Unknown => Ty::Unknown,
        other => {
            checker.error(
                ErrorCode::ZAP4024,
                format!("crypto.{method} expects a list, found {}", other.display()),
                expr_span(&args[0].value),
            );
            Ty::Unknown
        }
    }
}
