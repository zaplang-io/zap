# Golden fixture: PBKDF2-HMAC password-based key derivation (ticket #762,
# D-0132). `crypto.pbkdf2_hmac(hash_name, password, salt, iterations, dklen=0)`
# mirrors Python's hashlib.pbkdf2_hmac; `dklen == 0` means the backing hash's
# native digest length. The derived key is `bytes`, printed here via hex.encode.
#
# Known-answer vectors: RFC 6070 (PBKDF2-HMAC-SHA1) cases 1, 2, 3, 5 and 6 — the
# iterations=16777216 case 4 is omitted on time grounds (RFC 6070 notes it is
# slow; the shorter cases already pin the algorithm) — plus SHA-256 and SHA-512
# vectors cross-checked against CPython hashlib. Behavioral invariants (str/bytes
# equivalence, native vs explicit dklen, truncation vs multi-block expansion, and
# the argument-validation errors) are asserted. Intent documentation: keep every
# case holding.

def main():
    # --- RFC 6070 PBKDF2-HMAC-SHA1 known-answer vectors ---
    print(hex.encode(crypto.pbkdf2_hmac("sha1", "password", "salt", 1, 20)))
    print(hex.encode(crypto.pbkdf2_hmac("sha1", "password", "salt", 2, 20)))
    print(hex.encode(crypto.pbkdf2_hmac("sha1", "password", "salt", 4096, 20)))
    print(
        hex.encode(
            crypto.pbkdf2_hmac(
                "sha1",
                "passwordPASSWORDpassword",
                "saltSALTsaltSALTsaltSALTsaltSALTsalt",
                4096,
                25,
            ),
        ),
    )
    # RFC 6070 case 6: embedded NUL bytes in both password and salt.
    print(hex.encode(crypto.pbkdf2_hmac("sha1", "pass\x00word", "sa\x00lt", 4096, 16)))

    # --- SHA-256 vectors (CPython hashlib.pbkdf2_hmac) ---
    # dklen omitted ⇒ native 32-byte digest length.
    print(hex.encode(crypto.pbkdf2_hmac("sha256", "password", "salt", 1)))
    # dklen 20: shorter than the native length (truncation).
    print(hex.encode(crypto.pbkdf2_hmac("sha256", "password", "salt", 1, 20)))
    print(hex.encode(crypto.pbkdf2_hmac("sha256", "password", "salt", 4096, 32)))
    # dklen 40: longer than the native length (multi-block expansion).
    print(
        hex.encode(
            crypto.pbkdf2_hmac(
                "sha256",
                "passwordPASSWORDpassword",
                "saltSALTsaltSALTsaltSALTsaltSALTsalt",
                4096,
                40,
            ),
        ),
    )

    # --- SHA-512 vector ---
    print(hex.encode(crypto.pbkdf2_hmac("sha512", "password", "salt", 4096, 64)))

    # --- Edge: empty password and empty salt, iterations=1 ---
    print(hex.encode(crypto.pbkdf2_hmac("sha256", "", "", 1, 32)))

    # --- str and bytes inputs produce identical output ---
    assert crypto.pbkdf2_hmac("sha256", "password", "salt", 1, 20) == crypto.pbkdf2_hmac("sha256", b"password", b"salt", 1, 20)

    # --- Native dklen (omitted / 0) equals explicit native length ---
    assert crypto.pbkdf2_hmac("sha256", "password", "salt", 1) == crypto.pbkdf2_hmac("sha256", "password", "salt", 1, 32)
    assert len(crypto.pbkdf2_hmac("sha256", "password", "salt", 1)) == 32
    assert len(crypto.pbkdf2_hmac("sha512", "password", "salt", 1)) == 64
    assert len(crypto.pbkdf2_hmac("sha1", "password", "salt", 1)) == 20

    # --- dklen controls the output length exactly ---
    assert len(crypto.pbkdf2_hmac("sha256", "password", "salt", 1, 20)) == 20
    assert len(crypto.pbkdf2_hmac("sha256", "password", "salt", 1, 40)) == 40

    # --- A shorter dklen is a strict prefix of a longer one (PBKDF2 is a
    #     block-expanding construction; truncation just drops trailing blocks) ---
    assert crypto.pbkdf2_hmac("sha256", "password", "salt", 1, 20) == crypto.pbkdf2_hmac("sha256", "password", "salt", 1, 32)[0:20]

    # --- Unknown hash_name raises a catchable ValueError ---
    auto raised = False
    try:
        crypto.pbkdf2_hmac("sha3", "password", "salt", 1, 16)
    except ValueError:
        raised = True
    assert raised

    # --- iterations <= 0 raises (zero and negative) ---
    raised = False
    try:
        crypto.pbkdf2_hmac("sha256", "password", "salt", 0, 16)
    except ValueError:
        raised = True
    assert raised
    raised = False
    try:
        crypto.pbkdf2_hmac("sha256", "password", "salt", -1, 16)
    except ValueError:
        raised = True
    assert raised

    # --- dklen < 0 raises ---
    raised = False
    try:
        crypto.pbkdf2_hmac("sha256", "password", "salt", 1, -1)
    except ValueError:
        raised = True
    assert raised

    print("crypto.pbkdf2_hmac: PASS")
