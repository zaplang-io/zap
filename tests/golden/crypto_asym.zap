# Golden fixture: validated asymmetric-key metadata/import + P-256 ECDSA (ES256)
# and Ed25519 sign/verify (ticket #643, D-0126).
#
# One coherent supported surface, all free functions over PEM / raw components:
#   crypto.key_info(pem) -> KeyInfo(kind, bits, curve, is_private)
#   crypto.rsa_public_key_pem(n, e) / p256_public_key_pem(x, y) /
#     ed25519_public_key_pem(x) -> SPKI PEM bytes
#   crypto.p256_sha256_sign/verify  (ES256; JOSE fixed-width 64-byte r||s)
#   crypto.ed25519_sign/verify       (pure Ed25519 over the original message)
#
# Fixed disposable test keys generated with OpenSSL 3.5.0:
#   genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256
#   genpkey -algorithm Ed25519
#   genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048
# Components (n/e, x/y, Ed25519 x) are the big-endian octets of that key's JWK
# members, embedded as hex and imported through the supported native path. The
# P-256 external ES256 vector is a real OpenSSL ECDSA-SHA256 signature over the
# message, converted DER -> JOSE r||s, proving bidirectional interop. Ed25519 is
# deterministic (RFC 8032), so its signature is pinned by sha256 fingerprint; the
# embedded Ed25519 vector is the same signature produced by OpenSSL.
#
# Message: "jwt.header.payload".

def main():
    auto msg = "jwt.header.payload"

    auto p256_priv = """
-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgxkJyps3UegfKz6Ix
FnfvD8ni/1AUdoMoOjs35ar4OlqhRANCAATcJP2Ar0duqFLJumaDhQUNf7P8bRuR
lbyC4NWhe9USSjGD6ljEXkXxOB7iTdNL1plD4RF47G6/dA6uNycYeAWU
-----END PRIVATE KEY-----"""
    auto ed_priv = """
-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEILXk8RUFfBKbN8Zlbzzk3jb6dEbZsBfEDu8hOINn+zoK
-----END PRIVATE KEY-----"""

    # Validated public-key import from JWK integer/octet components (#643).
    auto rsa_n = hex.decode(
        "d37ed3bc368062cce8d3847dfd2fecef0d14e60b6a4f9c75e8a05a77a1a3506b71b297c8a2dd593bd54ecd612bf4a027f4cf660fa4470af8b472be11880124fe554e06c42b31ddfd9e905247f21bd7d1ebb35bec8e0c4d62cefef09ff3164d9a59774cc50a8b15c72657643292226be4095e441fc3ac65e2b55ab5e0a6374d1913989e2a953fced07a2d10fa22505fa97f57425e50f5e340ea900c5cb13e53240ccacb59cfc70dda1f6c2bb9ba51fc96fedb0e7ba5d5d6956259fd02ca6a65d0dd477c20af319daa67f3c1793a2b27124592805a5ee7bf0110799e06d1353b7bbd6ab23273c2987173880873d29d3615835b4e2ffb3b02bbed4403500699fdef",
    )
    auto rsa_e = hex.decode("010001")
    auto rsa_pub = crypto.rsa_public_key_pem(rsa_n, rsa_e)

    auto p256_x = hex.decode(
        "dc24fd80af476ea852c9ba668385050d7fb3fc6d1b9195bc82e0d5a17bd5124a",
    )
    auto p256_y = hex.decode(
        "3183ea58c45e45f1381ee24dd34bd69943e11178ec6ebf740eae372718780594",
    )
    auto ec_pub = crypto.p256_public_key_pem(p256_x, p256_y)

    auto ed_x = hex.decode(
        "d00751071f6bbd49b2cba1036d2ce2b37999bdb56916fb23dfade3a32e4d40f4",
    )
    auto ed_pub = crypto.ed25519_public_key_pem(ed_x)

    # Validated metadata (#643). curve is "" for non-EC keys.
    auto ri = crypto.key_info(rsa_pub)
    print(f"{ri.kind} {ri.bits} [{ri.curve}] {ri.is_private}")
    auto ci = crypto.key_info(ec_pub)
    print(f"{ci.kind} {ci.bits} [{ci.curve}] {ci.is_private}")
    auto ei = crypto.key_info(ed_pub)
    print(f"{ei.kind} {ei.bits} [{ei.curve}] {ei.is_private}")
    auto pi = crypto.key_info(p256_priv)
    print(f"{pi.kind} {pi.bits} [{pi.curve}] {pi.is_private}")

    # ES256 (P-256 / SHA-256), JOSE fixed-width 64-byte r||s.
    auto es = crypto.p256_sha256_sign(p256_priv, msg)
    print(len(es))
    print(crypto.p256_sha256_verify(ec_pub, msg, es))
    auto ext = hex.decode(
        "53f5b0d16f8640c80cb2b3269cfadc17adb6e51959cd9d19686ecd9a93686f16b8b4e891b970487a0d9c79669276e4effa05ced7a938b5ca63104fd9d89a3c10",
    )
    print(crypto.p256_sha256_verify(ec_pub, msg, ext))
    print(crypto.p256_sha256_verify(ec_pub, "jwt.header.PAYLOAD", es))
    print(crypto.p256_sha256_verify(ec_pub, msg, b"short"))

    # Pure Ed25519 — deterministic, so the signature is a stable KAT.
    auto ed = crypto.ed25519_sign(ed_priv, msg)
    print(crypto.sha256(ed))
    print(crypto.ed25519_verify(ed_pub, msg, ed))
    auto ed_ext = hex.decode(
        "b2056f12c727c07dc49a0438e00d47c8ec36c7a07047ecb0a9daba2d516a37eb4363c7722a1dd862da9c04d70eaa13de73e0c1d1df3956e240b070c66ac1e708",
    )
    print(crypto.ed25519_verify(ed_pub, msg, ed_ext))
    print(crypto.ed25519_verify(ed_pub, "jwt.header.PAYLOAD", ed))
