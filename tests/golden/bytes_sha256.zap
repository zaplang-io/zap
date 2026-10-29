# Golden fixture: SHA-256 over bytes -> hex (ticket #337 / #105). Confirms the
# hashing API consumes the `bytes` type, and that hex.encode of the raw
# streaming digest matches the one-shot hex digest.
def main():
    print(crypto.sha256(b"abc"))  # ba7816bf...20015ad
    auto h = crypto.sha256_new()
    crypto.update(h, b"abc")
    print(hex.encode(crypto.digest(h)))  # identical to the line above
    print(crypto.sha256(bytes("héllo")))  # 3c48591d...6068179
