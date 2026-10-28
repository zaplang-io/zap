# Golden fixture: raw-bytes HMAC-SHA256 (ticket #153).
#
# `crypto.hmac_sha256_raw(key, msg)` returns the 32-byte digest as `bytes`,
# rather than the lowercase hex `crypto.hmac_sha256` returns. Proven against
# published HMAC-SHA256 test vectors. There is no bytes->hex builtin, so each
# raw digest is fingerprinted with `crypto.sha256(raw)` (a deterministic,
# byte-for-byte-sensitive check) alongside its length.
#
# RFC 4231 Test Case 2: key="Jefe", msg="what do ya want for nothing?".
#   Published raw digest:  5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843
#   so crypto.sha256(raw) = 86ea816be859ea16764f6371c1b0e0b5577efb5e6e72b20ed5f683c503f8e80f
#
# Widely-published vector: key="key", msg="The quick brown fox jumps over the lazy dog".
#   Published raw digest:  f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8
#   (identical to crypto.hmac_sha256's hex output — the raw and hex paths agree)
#   so crypto.sha256(raw) = 2d1b70fdbe38872bff17d3d326fe1611d31b0dfb4b735bbcef279abc415faf75
def main():
    auto r1 = crypto.hmac_sha256_raw("Jefe", "what do ya want for nothing?")
    print(len(r1))
    print(crypto.sha256(r1))
    auto r2 = crypto.hmac_sha256_raw(
        "key",
        "The quick brown fox jumps over the lazy dog",
    )
    print(len(r2))
    print(crypto.sha256(r2))
