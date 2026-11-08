# Golden fixture: SHA-3 and BLAKE2 hash families (ticket #770, D-0133).
#
# Unlike the self-contained SHA-1/2 + MD5 core, SHA-3 and BLAKE2 are taken from
# OpenSSL's EVP_Digest (recorded in D-0133). One-shot only in this cut.
#
# Known-answer vectors: NIST SHA-3 KATs for "" and "abc"; BLAKE2b-512 /
# BLAKE2s-256 (the Python hashlib defaults) RFC 7693 / reference vectors.
# `crypto.<algo>(data)` returns the lowercase hex digest. str and bytes inputs
# are identical. Intent documentation: keep every case holding.

def main():
    print(crypto.sha3_256(""))
    print(crypto.sha3_256("abc"))
    print(crypto.sha3_512(""))
    print(crypto.sha3_512("abc"))
    print(crypto.blake2b(""))
    print(crypto.blake2b("abc"))
    print(crypto.blake2s(""))
    print(crypto.blake2s("abc"))

    # str and bytes inputs produce identical digests.
    assert crypto.sha3_256("abc") == crypto.sha3_256(b"abc")
    assert crypto.sha3_512("abc") == crypto.sha3_512(b"abc")
    assert crypto.blake2b("abc") == crypto.blake2b(b"abc")
    assert crypto.blake2s("abc") == crypto.blake2s(b"abc")

    # Digest widths (hex chars): SHA3-256/BLAKE2s = 64, SHA3-512/BLAKE2b = 128.
    assert len(crypto.sha3_256("abc")) == 64
    assert len(crypto.blake2s("abc")) == 64
    assert len(crypto.sha3_512("abc")) == 128
    assert len(crypto.blake2b("abc")) == 128

    print("crypto.sha3/blake2: PASS")
