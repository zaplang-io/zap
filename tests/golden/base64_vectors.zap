# Golden fixture: base64 encode/decode, standard and URL-safe, with and without
# padding (ticket #337). The ASCII inputs are the RFC 4648 §10 test vectors; the
# binary input [251, 255] exercises the two alphabet-specific characters.
def main():
    print(base64.encode(b""))  # (empty)
    print(base64.encode(b"f"))  # Zg==
    print(base64.encode(b"fo"))  # Zm8=
    print(base64.encode(b"foo"))  # Zm9v
    print(base64.encode(b"foob"))  # Zm9vYg==
    print(base64.encode(b"fooba"))  # Zm9vYmE=
    print(base64.encode(b"foobar"))  # Zm9vYmFy
    print(base64.encode_nopad(b"f"))  # Zg
    print(base64.encode_nopad(b"fooba"))  # Zm9vYmE
    print(base64.encode(bytes([251, 255])))  # +/8=
    print(base64.urlsafe_encode(bytes([251, 255])))  # -_8=
    print(base64.urlsafe_encode_nopad(bytes([251, 255])))  # -_8
    print(str(base64.decode("Zm9vYmFy")))  # foobar
    print(str(base64.decode("Zg==")))  # f
    print(str(base64.decode("Zg")))  # f  (missing padding tolerated)
    print(hex.encode(base64.urlsafe_decode("-_8=")))  # fbff
