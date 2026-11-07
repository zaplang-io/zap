# Golden fixture: constant-time byte comparison (ticket #729, D-0131).
#
# `crypto.constant_time_eq(a, b) -> bool` returns whether two byte sequences are
# equal, comparing them (via OpenSSL CRYPTO_memcmp) in time independent of their
# byte CONTENTS for equal-length inputs. A length difference is reported as a
# mismatch and is observable; there is no branch on byte contents.
#
# Timing itself cannot be asserted deterministically in a golden, so this fixture
# nails down the full FUNCTIONAL contract instead — the equality semantics across
# every edge case the downstream HMAC-comparison use (zaplang-io/jwt #11 item 8)
# depends on. It is intent documentation: later PRs must keep every case holding.
#
# Both arguments accept `str` or `bytes` (they share the byte-data ABI); empty
# inputs are valid; embedded NUL bytes are ordinary content, never terminators.

def main():
    # --- Equal, non-empty: bytes==bytes and str==str ---
    assert crypto.constant_time_eq(b"message", b"message")
    assert crypto.constant_time_eq("message", "message")

    # --- Unequal, SAME length, differing in the FIRST byte ---
    assert not crypto.constant_time_eq(b"Xessage", b"message")
    assert not crypto.constant_time_eq("Xessage", "message")

    # --- Unequal, SAME length, differing in the LAST byte ---
    assert not crypto.constant_time_eq(b"messagE", b"message")
    assert not crypto.constant_time_eq("messagE", "message")

    # --- Unequal, SAME length, differing in a MIDDLE byte ---
    assert not crypto.constant_time_eq(b"mesXage", b"message")
    assert not crypto.constant_time_eq("mesXage", "message")

    # --- Unequal LENGTHS (a longer than b, and b longer than a). A length
    #     difference is a mismatch even when one is a strict prefix of the other. ---
    assert not crypto.constant_time_eq(b"message!", b"message")
    assert not crypto.constant_time_eq(b"message", b"message!")
    assert not crypto.constant_time_eq(b"msg", b"message")
    assert not crypto.constant_time_eq(b"message", b"msg")
    # Prefix relationship must not read as equal.
    assert not crypto.constant_time_eq(b"mess", b"message")

    # --- Both empty: two empty inputs are equal (bytes and str, mixed) ---
    assert crypto.constant_time_eq(b"", b"")
    assert crypto.constant_time_eq("", "")
    assert crypto.constant_time_eq("", b"")
    assert crypto.constant_time_eq(b"", "")

    # --- One empty, one not (both orders) ---
    assert not crypto.constant_time_eq(b"", b"x")
    assert not crypto.constant_time_eq(b"x", b"")
    assert not crypto.constant_time_eq("", "x")
    assert not crypto.constant_time_eq("x", "")

    # --- Single-byte inputs: equal and differing ---
    assert crypto.constant_time_eq(b"a", b"a")
    assert not crypto.constant_time_eq(b"a", b"b")

    # --- str vs bytes equivalence: identical ASCII content compares equal
    #     regardless of which argument carries which representation ---
    assert crypto.constant_time_eq("hello", b"hello")
    assert crypto.constant_time_eq(b"hello", "hello")
    # str vs bytes, same length, differing last byte.
    assert not crypto.constant_time_eq("hello", b"hellp")
    assert not crypto.constant_time_eq(b"hellp", "hello")
    # str vs bytes, differing length.
    assert not crypto.constant_time_eq("hello", b"hell")
    assert not crypto.constant_time_eq(b"hell", "hello")

    # --- Symmetry: the result never depends on argument order ---
    assert crypto.constant_time_eq(b"abc", b"abc") == crypto.constant_time_eq(b"abc", b"abc")
    assert crypto.constant_time_eq(b"abc", b"abd") == crypto.constant_time_eq(b"abd", b"abc")
    assert crypto.constant_time_eq(b"abc", b"ab") == crypto.constant_time_eq(b"ab", b"abc")

    # --- Non-ASCII / high bytes: byte content, not text, is what matters.
    #     "é" is C3 A9 in UTF-8; raw bytes built two ways compare equal. ---
    assert crypto.constant_time_eq(bytes([0xC3, 0xA9]), hex.decode("c3a9"))
    assert crypto.constant_time_eq("é", bytes([0xC3, 0xA9]))
    assert not crypto.constant_time_eq(bytes([0xC3, 0xA9]), bytes([0xC3, 0xAA]))
    # Full high-byte range, equal and single-bit differences at the edges.
    assert crypto.constant_time_eq(bytes([0, 127, 128, 255]), bytes([0, 127, 128, 255]))
    assert not crypto.constant_time_eq(bytes([0, 127, 128, 255]), bytes([1, 127, 128, 255]))
    assert not crypto.constant_time_eq(bytes([0, 127, 128, 255]), bytes([0, 127, 128, 254]))

    # --- Embedded NUL bytes are ordinary content, NOT string terminators.
    #     Equal when every byte matches; a NUL in the middle does not truncate. ---
    assert crypto.constant_time_eq(bytes([0, 1, 0, 2]), bytes([0, 1, 0, 2]))
    assert crypto.constant_time_eq(bytes([0, 1, 0, 2]), hex.decode("00010002"))
    # Differ only AFTER an embedded NUL — must still be detected.
    assert not crypto.constant_time_eq(bytes([0, 1, 0, 2]), bytes([0, 1, 0, 3]))
    # Differ in the leading NUL region (first byte).
    assert not crypto.constant_time_eq(bytes([0, 1, 0, 2]), bytes([9, 1, 0, 2]))
    # A trailing NUL makes the inputs DIFFERENT lengths (NUL is a real byte),
    # so they are unequal — a C-string comparison would wrongly call them equal.
    assert not crypto.constant_time_eq(bytes([0, 1, 0, 2]), bytes([0, 1, 0, 2, 0]))
    # All-NUL buffers of equal vs differing length.
    assert crypto.constant_time_eq(bytes([0, 0, 0]), bytes([0, 0, 0]))
    assert not crypto.constant_time_eq(bytes([0, 0, 0]), bytes([0, 0, 0, 0]))

    # --- Longer buffers, difference ONLY at the very last byte (the case a
    #     short-circuiting / early-out compare would be fastest to reject). ---
    auto long_a = crypto.random_bytes(64)
    assert crypto.constant_time_eq(long_a, long_a)
    assert crypto.constant_time_eq(long_a + b"Z", long_a + b"Z")
    assert not crypto.constant_time_eq(long_a + b"Z", long_a + b"Y")

    # --- Realistic MAC-comparison shape: compare a recomputed HMAC digest
    #     against an expected one. Equal digests match; a one-byte-tampered
    #     digest (same length) does not. ---
    auto mac = crypto.hmac_sha256_raw(b"key", b"The quick brown fox")
    auto mac_again = crypto.hmac_sha256_raw(b"key", b"The quick brown fox")
    assert crypto.constant_time_eq(mac, mac_again)
    auto mac_bad = crypto.hmac_sha256_raw(b"key", b"The quick brown foY")
    assert not crypto.constant_time_eq(mac, mac_bad)
    assert len(mac) == len(mac_bad)

    print("crypto.constant_time_eq: PASS")
