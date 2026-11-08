# Golden fixture: SHA-384 and SHA-224 — completing the SHA-2 family (ticket
# #765, D-0132). SHA-384 is SHA-512 run from a different IV and truncated to 48
# bytes; SHA-224 is SHA-256 from a different IV truncated to 28 bytes.
#
# Known-answer vectors (FIPS 180-4 / NIST CAVP examples): the empty string,
# "abc", and the 896-bit two-block message, plus the 448-bit message for
# SHA-224. One-shot `crypto.sha384/sha224(data)` returns the lowercase hex
# digest. The streaming path (`_new`/`update`/`hexdigest`/`digest`) must agree
# with the one-shot across chunk boundaries, be non-destructive, and accept
# str and bytes identically. This fixture is intent documentation: later PRs
# must keep every case holding.

def main():
    # --- SHA-384 one-shot known-answer vectors (hex digest printed) ---
    print(crypto.sha384(""))
    print(crypto.sha384("abc"))
    print(
        crypto.sha384(
            "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
        ),
    )

    # --- SHA-224 one-shot known-answer vectors ---
    print(crypto.sha224(""))
    print(crypto.sha224("abc"))
    print(crypto.sha224("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"))
    print(
        crypto.sha224(
            "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
        ),
    )

    # --- str and bytes inputs are identical ---
    assert crypto.sha384("abc") == crypto.sha384(b"abc")
    assert crypto.sha224("abc") == crypto.sha224(b"abc")

    # --- Digest length: the FIPS truncation (48 / 28 raw bytes) ---
    auto d384 = crypto.sha384_new()
    crypto.update(d384, b"abc")
    assert len(crypto.digest(d384)) == 48
    auto d224 = crypto.sha224_new()
    crypto.update(d224, b"abc")
    assert len(crypto.digest(d224)) == 28

    # --- Raw digest, hex-encoded, equals the one-shot hex ---
    assert hex.encode(crypto.digest(d384)) == crypto.sha384("abc")
    assert hex.encode(crypto.digest(d224)) == crypto.sha224("abc")

    # --- Streaming == one-shot across a chunk boundary (SHA-384) ---
    auto s384 = crypto.sha384_new()
    crypto.update(s384, b"ab")
    crypto.update(s384, b"c")
    assert crypto.hexdigest(s384) == crypto.sha384("abc")

    # --- Non-destructive finalize: digest twice, then keep updating (SHA-384) ---
    assert crypto.hexdigest(s384) == crypto.sha384("abc")
    crypto.update(s384, b"d")
    assert crypto.hexdigest(s384) == crypto.sha384("abcd")

    # --- Streaming == one-shot across a chunk boundary (SHA-224) ---
    auto s224 = crypto.sha224_new()
    crypto.update(s224, b"ab")
    crypto.update(s224, b"c")
    assert crypto.hexdigest(s224) == crypto.sha224("abc")
    assert crypto.hexdigest(s224) == crypto.sha224("abc")
    crypto.update(s224, b"d")
    assert crypto.hexdigest(s224) == crypto.sha224("abcd")

    # --- Split a multi-block (112-byte / 896-bit) message at an awkward offset
    #     that crosses the internal 64-byte block boundary, fed as three uneven
    #     chunks; the streamed digest must equal the one-shot for both algos. ---
    auto msg = "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"
    auto m384 = crypto.sha384_new()
    auto m224 = crypto.sha224_new()
    crypto.update(m384, msg[0:1])
    crypto.update(m384, msg[1:64])
    crypto.update(m384, msg[64:112])
    crypto.update(m224, msg[0:1])
    crypto.update(m224, msg[1:64])
    crypto.update(m224, msg[64:112])
    assert crypto.hexdigest(m384) == crypto.sha384(msg)
    assert crypto.hexdigest(m224) == crypto.sha224(msg)

    print("crypto.sha384/sha224 streaming: PASS")
