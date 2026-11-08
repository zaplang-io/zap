# Golden fixture: general bytes<->text encoders (ticket #771, D-0133).
#
# Convenience aliases on `crypto` over the existing hex/base64 runtime, so a
# caller holding a digest or key as `bytes` can render it without a round-trip
# hack (the gap flagged by the raw-HMAC work #153):
#   crypto.hex(data) -> str            # lowercase, like Python bytes.hex()
#   crypto.unhex(s) -> bytes
#   crypto.b64url_encode(data) -> str  # unpadded, JOSE-style -_ alphabet
#   crypto.b64url_decode(s) -> bytes   # padding optional
# Round-trips arbitrary bytes (incl. 0x00 / 0xff), rejects malformed hex, and
# produces/accepts unpadded URL-safe base64. Intent documentation.

def main():
    auto d = bytes([0, 1, 2, 255, 254, 0, 128])

    # --- hex: lowercase, round-trips through unhex, handles 0x00 / 0xff ---
    print(crypto.hex(d))
    assert crypto.unhex(crypto.hex(d)) == d
    assert crypto.hex(b"") == ""
    assert crypto.unhex("") == b""
    # hex accepts str input too (same byte-data ABI).
    assert crypto.hex("abc") == "616263"

    # --- base64url: unpadded, uses the -_ alphabet, round-trips ---
    print(crypto.b64url_encode(d))
    assert crypto.b64url_decode(crypto.b64url_encode(d)) == d
    # Bytes that exercise both URL-safe substitutions (- for +, _ for /).
    print(crypto.b64url_encode(hex.decode("fbffbf")))
    # No padding is ever emitted.
    assert not "=" in crypto.b64url_encode(d)
    assert not "=" in crypto.b64url_encode(b"hello, world")
    # Empty input is valid both ways.
    assert crypto.b64url_encode(b"") == ""
    assert crypto.b64url_decode("") == b""

    # --- decode accepts input with OR without padding ---
    assert crypto.b64url_decode("AAEC__4AgA") == d
    assert crypto.b64url_decode("AAEC__4AgA==") == d

    # --- unhex rejects odd length and non-hex characters (catchable) ---
    auto raised = False
    try:
        crypto.unhex("abc")
    except Exception:
        raised = True
    assert raised
    raised = False
    try:
        crypto.unhex("zz")
    except Exception:
        raised = True
    assert raised

    print("crypto.encoding: PASS")
