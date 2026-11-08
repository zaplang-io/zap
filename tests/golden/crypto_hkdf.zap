# Golden fixture: HKDF extract-and-expand key derivation (ticket #763, D-0132,
# RFC 5869). `crypto.hkdf(hash_name, ikm, length, salt="", info="")` performs a
# one-shot HKDF-Extract then HKDF-Expand; an empty salt is the RFC-mandated
# HashLen zero string. The output keying material is `bytes`, printed via
# hex.encode.
#
# Known-answer vectors: all of RFC 5869 Appendix A — A.1/A.2/A.3 (SHA-256,
# including A.3's zero-length salt and info) and A.4/A.5/A.6/A.7 (SHA-1) — plus
# a SHA-512 smoke vector, asserting the exact OKM. Behavioral invariants
# (str/bytes equivalence, empty vs non-empty salt differ, empty info valid, the
# length bounds) are asserted. Intent documentation: keep every case holding.

def main():
    auto ikm1 = hex.decode("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b")
    auto salt1 = hex.decode("000102030405060708090a0b0c")
    auto info1 = hex.decode("f0f1f2f3f4f5f6f7f8f9")
    auto ikm2 = hex.decode(
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f",
    )
    auto salt2 = hex.decode(
        "606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeaf",
    )
    auto info2 = hex.decode(
        "b0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff",
    )
    auto ikm_c = hex.decode("0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c")

    # A.1 — SHA-256, basic
    print(hex.encode(crypto.hkdf("sha256", ikm1, 42, salt1, info1)))
    # A.2 — SHA-256, longer inputs and output (82 bytes)
    print(hex.encode(crypto.hkdf("sha256", ikm2, 82, salt2, info2)))
    # A.3 — SHA-256, zero-length salt and info (omitted ⇒ defaults)
    print(hex.encode(crypto.hkdf("sha256", ikm1, 42)))
    # A.4 — SHA-1, basic
    print(
        hex.encode(
            crypto.hkdf("sha1", hex.decode("0b0b0b0b0b0b0b0b0b0b0b"), 42, salt1, info1),
        ),
    )
    # A.5 — SHA-1, longer inputs and output
    print(hex.encode(crypto.hkdf("sha1", ikm2, 82, salt2, info2)))
    # A.6 — SHA-1, zero-length salt and info, IKM 22x0b
    print(hex.encode(crypto.hkdf("sha1", ikm1, 42)))
    # A.7 — SHA-1, zero-length salt (default), IKM 22x0c
    print(hex.encode(crypto.hkdf("sha1", ikm_c, 42)))
    # SHA-512 smoke vector (cross-checked against the RFC 5869 reference)
    print(hex.encode(crypto.hkdf("sha512", ikm1, 42, salt1, info1)))

    # --- str and bytes inputs are equivalent ---
    assert crypto.hkdf("sha256", "secret", 32, "salt", "info") == crypto.hkdf("sha256", b"secret", 32, b"salt", b"info")

    # --- A non-empty salt differs from the empty (all-zero) salt ---
    assert crypto.hkdf("sha256", ikm1, 42, salt1, info1) != crypto.hkdf("sha256", ikm1, 42, b"", info1)
    # Omitting both salt and info equals passing explicit empty values.
    assert crypto.hkdf("sha256", ikm1, 42) == crypto.hkdf("sha256", ikm1, 42, b"", b"")

    # --- Empty info is valid (omitted ⇒ empty) and differs from non-empty info ---
    assert len(crypto.hkdf("sha256", ikm1, 42, salt1)) == 42
    assert crypto.hkdf("sha256", ikm1, 42, salt1) != crypto.hkdf("sha256", ikm1, 42, salt1, info1)

    # --- length controls the output size exactly ---
    assert len(crypto.hkdf("sha256", ikm1, 1)) == 1
    assert len(crypto.hkdf("sha256", ikm1, 32)) == 32

    # --- length up to 255*HashLen is allowed; beyond it raises ValueError ---
    assert len(crypto.hkdf("sha256", ikm1, 8160)) == 8160
    auto raised = False
    try:
        crypto.hkdf("sha256", ikm1, 8161)
    except ValueError:
        raised = True
    assert raised

    # --- length <= 0 raises (zero and negative) ---
    raised = False
    try:
        crypto.hkdf("sha256", ikm1, 0)
    except ValueError:
        raised = True
    assert raised
    raised = False
    try:
        crypto.hkdf("sha256", ikm1, -1)
    except ValueError:
        raised = True
    assert raised

    # --- Unknown hash_name raises ValueError ---
    raised = False
    try:
        crypto.hkdf("sha3", ikm1, 42)
    except ValueError:
        raised = True
    assert raised

    print("crypto.hkdf: PASS")
