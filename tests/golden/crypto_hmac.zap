# Golden fixture: crypto HMAC (ticket #105).
#
# RFC 2104 construction, keyed-hash of a message. Known vectors (the widely
# published HMAC example): key="key", message="The quick brown fox jumps over
# the lazy dog". `crypto.hmac_<algo>(key, msg)` returns the lowercase hex digest.
def main():
    print(crypto.hmac_sha256("key", "The quick brown fox jumps over the lazy dog"))
    print(crypto.hmac_sha512("key", "The quick brown fox jumps over the lazy dog"))
