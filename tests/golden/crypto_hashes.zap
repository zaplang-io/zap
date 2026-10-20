# Golden fixture: crypto one-shot hashing (ticket #105).
#
# Known test vectors: SHA-256/SHA-512/SHA-1 of "abc" (FIPS examples), MD5 of
# "abc" (RFC 1321 A.5), and SHA-256 of the empty string. `crypto.<algo>(data)`
# returns the lowercase hex digest; `import crypto` is accepted but not required.
def main():
    print(crypto.sha256("abc"))
    print(crypto.sha512("abc"))
    print(crypto.sha1("abc"))
    print(crypto.md5("abc"))
    print(crypto.sha256(""))
