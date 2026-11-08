# Golden fixture: scrypt password-based KDF (ticket #775, D-0134).
#
# crypto.scrypt(password, salt, n, r, p, dklen=64) -> bytes, OpenSSL
# EVP_PBE_scrypt, mirroring Python hashlib.scrypt. Verified against the RFC 7914
# section 12 test vectors (the N=1048576 vector is omitted on time/memory
# grounds). `dklen` defaults to 64; str and bytes inputs are interchangeable. A
# bad parameter (n not a power of two > 1, non-positive r/p/dklen) raises a
# catchable ValueError. KATs build a walled garden around the KDF.

def main():
    # RFC 7914 section 12, vector 1: P="" S="" N=16 r=1 p=1 dkLen=64.
    auto v1 = crypto.scrypt(b"", b"", 16, 1, 1, 64)
    assert crypto.hex(v1) == "77d6576238657b203b19ca42c18a0497f16b4844e3074ae8dfdffa3fede21442fcd0069ded0948f8326a753a0fc81f17e8d3e0fb2e0d3628cf35e20c38d18906"

    # Vector 2: P="password" S="NaCl" N=1024 r=8 p=16 dkLen=64.
    auto v2 = crypto.scrypt("password", "NaCl", 1024, 8, 16, 64)
    assert crypto.hex(v2) == "fdbabe1c9d3472007856e7190d01e9fe7c6ad7cbc8237830e77376634b3731622eaf30d92e22a3886ff109279d9830dac727afb94a83ee6d8360cbdfa2cc0640"

    # Vector 3: P="pleaseletmein" S="SodiumChloride" N=16384 r=8 p=1 dkLen=64.
    auto v3 = crypto.scrypt("pleaseletmein", "SodiumChloride", 16384, 8, 1, 64)
    assert crypto.hex(v3) == "7023bdcb3afd7348461c06cd81fd38ebfda8fbba904f8e3ea9b543f6545da1f2d5432955613f0fcf62d49705242a9af9e61e85dc0d651e40dfcf017b45575887"

    # dklen defaults to 64 (a 64-byte key is 128 hex chars); a custom dklen is
    # honored (32 bytes -> 64 hex chars).
    assert len(crypto.hex(crypto.scrypt("pw", "salt", 16, 1, 1))) == 128
    assert len(crypto.hex(crypto.scrypt("pw", "salt", 16, 1, 1, 32))) == 64

    # str and bytes inputs agree (shared ZapStr byte ABI).
    assert crypto.scrypt("abc", "xyz", 16, 1, 1, 16) == crypto.scrypt(b"abc", b"xyz", 16, 1, 1, 16)

    # --- bad parameters raise a catchable ValueError ---
    # n not a power of two.
    auto raised = False
    try:
        crypto.scrypt("pw", "salt", 1000, 8, 1, 64)
    except ValueError:
        raised = True
    assert raised

    # n must be greater than 1.
    raised = False
    try:
        crypto.scrypt("pw", "salt", 1, 8, 1, 64)
    except ValueError:
        raised = True
    assert raised

    # r must be positive.
    raised = False
    try:
        crypto.scrypt("pw", "salt", 16, 0, 1, 64)
    except ValueError:
        raised = True
    assert raised

    # dklen must be positive.
    raised = False
    try:
        crypto.scrypt("pw", "salt", 16, 1, 1, 0)
    except ValueError:
        raised = True
    assert raised

    print("crypto.scrypt: PASS")
