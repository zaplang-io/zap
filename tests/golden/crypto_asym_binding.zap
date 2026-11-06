# #643: exhaustive negatives for the asymmetric-key surface (D-0126).
#
# Enforced invariants:
#   - role binding: a PUBLIC key can never sign.
#   - family binding: ES256 signing needs a P-256 key; Ed25519 signing needs an
#     Ed25519 key; a wrong family / wrong curve is rejected, never silently
#     reinterpreted.
#   - key_info rejects unparseable / unsupported / encrypted PEM.
#   - import validation: RSA n/e must be usable public params; P-256 coordinates
#     must be 32 bytes and lie on the curve; Ed25519 keys must be 32 bytes.
#   - verification is a pure boolean: a wrong-family key, a wrong signature
#     length (including a DER-encoded ECDSA signature fed where JOSE r||s is
#     required), or a tampered message/signature yields False, never a panic.
#
# Fixed disposable test keys generated with OpenSSL 3.5.0. The embedded P-256
# external signature's DER form (71 bytes) exercises the JOSE-vs-DER mistake.

def sign_p256_rejects(str pem):
    auto rejected = False
    try:
        crypto.p256_sha256_sign(pem, "m")
    except ValueError:
        rejected = True
    assert rejected

def sign_ed_rejects(str pem):
    auto rejected = False
    try:
        crypto.ed25519_sign(pem, "m")
    except ValueError:
        rejected = True
    assert rejected

def key_info_rejects(str pem):
    auto rejected = False
    try:
        crypto.key_info(pem)
    except ValueError:
        rejected = True
    assert rejected

def rsa_import_rejects(bytes n, bytes e):
    auto rejected = False
    try:
        crypto.rsa_public_key_pem(n, e)
    except ValueError:
        rejected = True
    assert rejected

def p256_import_rejects(bytes x, bytes y):
    auto rejected = False
    try:
        crypto.p256_public_key_pem(x, y)
    except ValueError:
        rejected = True
    assert rejected

def ed_import_rejects(bytes x):
    auto rejected = False
    try:
        crypto.ed25519_public_key_pem(x)
    except ValueError:
        rejected = True
    assert rejected

def main():
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
    auto p384_priv = """
-----BEGIN PRIVATE KEY-----
MIG2AgEAMBAGByqGSM49AgEGBSuBBAAiBIGeMIGbAgEBBDCJh/XDcE6mASXqtTtV
uDZk5HlHhFWBp2O+IMjROLOXVyYxV+Is7FAUT8jR7YtfKg6hZANiAASnyB2g2it4
X1OqX2TAm2gcirAiWn/mrgH/s8rJdwQGzXeb4Umta+ZlCUuab8lnGPZnFa2gKUfK
HVd7xtQxbkpRlCqD5ktGAN3azmAdnXlZcfk8IonIs9SOs4MidIpfGEQ=
-----END PRIVATE KEY-----"""

    auto rsa_n = hex.decode(
        "d37ed3bc368062cce8d3847dfd2fecef0d14e60b6a4f9c75e8a05a77a1a3506b71b297c8a2dd593bd54ecd612bf4a027f4cf660fa4470af8b472be11880124fe554e06c42b31ddfd9e905247f21bd7d1ebb35bec8e0c4d62cefef09ff3164d9a59774cc50a8b15c72657643292226be4095e441fc3ac65e2b55ab5e0a6374d1913989e2a953fced07a2d10fa22505fa97f57425e50f5e340ea900c5cb13e53240ccacb59cfc70dda1f6c2bb9ba51fc96fedb0e7ba5d5d6956259fd02ca6a65d0dd477c20af319daa67f3c1793a2b27124592805a5ee7bf0110799e06d1353b7bbd6ab23273c2987173880873d29d3615835b4e2ffb3b02bbed4403500699fdef",
    )
    auto ec_pub = crypto.p256_public_key_pem(
        hex.decode("dc24fd80af476ea852c9ba668385050d7fb3fc6d1b9195bc82e0d5a17bd5124a"),
        hex.decode("3183ea58c45e45f1381ee24dd34bd69943e11178ec6ebf740eae372718780594"),
    )
    auto ed_pub = crypto.ed25519_public_key_pem(
        hex.decode("d00751071f6bbd49b2cba1036d2ce2b37999bdb56916fb23dfade3a32e4d40f4"),
    )

    # --- role binding: a public key cannot sign ---
    sign_p256_rejects(ec_pub)
    sign_ed_rejects(ed_pub)
    # --- family / curve binding at signing ---
    sign_p256_rejects(ed_priv)  # Ed25519 key offered for ES256
    sign_p256_rejects(p384_priv)  # wrong EC curve (P-384) offered for ES256
    sign_ed_rejects(p256_priv)  # P-256 key offered for Ed25519

    # --- key_info gate ---
    key_info_rejects("not a pem")
    key_info_rejects("")

    # --- import validation ---
    rsa_import_rejects(rsa_n, hex.decode("04"))  # even public exponent
    rsa_import_rejects(rsa_n, hex.decode("01"))  # exponent must be > 1
    rsa_import_rejects(b"", hex.decode("010001"))  # empty modulus
    p256_import_rejects(b"short", b"short")  # coordinates must be 32 bytes
    p256_import_rejects(
        hex.decode("0000000000000000000000000000000000000000000000000000000000000000"),
        hex.decode("0000000000000000000000000000000000000000000000000000000000000000"),
    )  # (0, 0) is not on P-256
    ed_import_rejects(b"short")  # Ed25519 key must be 32 bytes

    # --- verification is a pure boolean (no panic) ---
    auto es = crypto.p256_sha256_sign(p256_priv, "m")
    auto eds = crypto.ed25519_sign(ed_priv, "m")
    assert not crypto.p256_sha256_verify(ed_pub, "m", es)  # wrong family key
    assert not crypto.ed25519_verify(ec_pub, "m", eds)  # wrong family key
    assert not crypto.p256_sha256_verify(ec_pub, "m", b"short")  # bad length
    assert not crypto.ed25519_verify(ed_pub, "m", b"short")  # bad length
    assert not crypto.p256_sha256_verify(ec_pub, "tampered", es)  # tampered message
    # a DER-encoded ECDSA signature (71 bytes) where JOSE r||s is required
    auto der_sig = hex.decode(
        "3045022053f5b0d16f8640c80cb2b3269cfadc17adb6e51959cd9d19686ecd9a93686f16022100b8b4e891b970487a0d9c79669276e4effa05ced7a938b5ca63104fd9d89a3c10",
    )
    assert not crypto.p256_sha256_verify(ec_pub, "m", der_sig)

    print("ok")
