# Golden fixture: URL-safe random token (ticket #764, D-0132).
# `crypto.token_urlsafe(nbytes) -> str` returns nbytes of CSPRNG randomness,
# base64url-encoded WITHOUT padding (mirrors Python secrets.token_urlsafe).
#
# The output is random, so this fixture asserts the SHAPE contract rather than
# an exact value: the unpadded base64url length for several nbytes, a URL-safe
# alphabet with no padding, a clean round-trip through base64.urlsafe_decode
# (which also recovers exactly nbytes bytes), nbytes=0 ⇒ "", a negative nbytes
# raising ValueError, and two successive calls differing. Intent documentation.

def check(int64 nbytes, int64 want_len):
    auto tok = crypto.token_urlsafe(nbytes)
    # Unpadded base64url length: ceil(nbytes*4/3).
    assert len(tok) == want_len
    # URL-safe, unpadded: never standard-alphabet '+'/'/' nor '=' padding.
    assert not "=" in tok
    assert not "+" in tok
    assert not "/" in tok
    # Every character is valid base64url and decodes back to exactly nbytes
    # bytes; re-encoding (no padding) reproduces the token exactly, which proves
    # the alphabet and the absence of padding.
    auto raw = base64.urlsafe_decode(tok)
    assert len(raw) == nbytes
    assert base64.urlsafe_encode_nopad(raw) == tok

def main():
    # Expected unpadded lengths: 0->0, 1->2, 16->22, 32->43, 64->86.
    check(0, 0)
    check(1, 2)
    check(16, 22)
    check(32, 43)
    check(64, 86)

    # nbytes == 0 yields the empty string.
    assert crypto.token_urlsafe(0) == ""

    # Two successive calls differ (entropy smoke check, not a statistical test).
    assert crypto.token_urlsafe(32) != crypto.token_urlsafe(32)

    # A negative nbytes raises a catchable ValueError.
    auto raised = False
    try:
        crypto.token_urlsafe(-1)
    except ValueError:
        raised = True
    assert raised

    print("crypto.token_urlsafe: PASS")
