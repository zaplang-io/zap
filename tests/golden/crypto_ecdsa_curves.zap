# Golden fixture: additional JOSE ECDSA curves P-384 (ES384) and P-521 (ES512)
# (ticket #769, D-0133). Same validated path and DER<->JOSE fixed-width r||s
# conversion as P-256/ES256 (#643); DER is never surfaced.
#
#   crypto.p384_sha384_sign/verify   (ES384; JOSE 96-byte r||s)
#   crypto.p521_sha512_sign/verify   (ES512; JOSE 132-byte r||s)
#   crypto.p384_public_key_pem / p521_public_key_pem  (JWK x/y import)
#
# ECDSA is randomized, so Zap signing is checked by sign->verify round-trip;
# interoperability is pinned by verifying an OpenSSL-produced signature embedded
# as JOSE r||s hex. key_info reports the curve; wrong-curve keys and off-curve
# coordinates are rejected. Fixed disposable keys (OpenSSL). Message below.

def main():
    auto msg = "jwt.header.payload"
    auto p384_priv = """
-----BEGIN PRIVATE KEY-----
MIG2AgEAMBAGByqGSM49AgEGBSuBBAAiBIGeMIGbAgEBBDAxGsNHDrwCr56hj+DB
JbG9KnFze433mnBPtL+b8H1q7vZMB16pmkEUMjGkyshbw4ahZANiAARzxwYXt98Q
YNbovqHcxSfBjVwZJt3BddeAMTAXieAqT9uSzm76eRhTaXTnwtsbzTTb8/6isxt/
ybECwH+hEOY8NjyO5d8fAPxVYNRCjPsfFyJdmpUceiDH633Hzn2oLYE=
-----END PRIVATE KEY-----
"""
    auto p384_pub = """
-----BEGIN PUBLIC KEY-----
MHYwEAYHKoZIzj0CAQYFK4EEACIDYgAEc8cGF7ffEGDW6L6h3MUnwY1cGSbdwXXX
gDEwF4ngKk/bks5u+nkYU2l058LbG8002/P+orMbf8mxAsB/oRDmPDY8juXfHwD8
VWDUQoz7HxciXZqVHHogx+t9x859qC2B
-----END PUBLIC KEY-----
"""
    auto p521_priv = """
-----BEGIN PRIVATE KEY-----
MIHuAgEAMBAGByqGSM49AgEGBSuBBAAjBIHWMIHTAgEBBEIAM4IIjrfzgxZ8Q3I/
1fo27f0O4uGQXwitMH4hZHqVDVzbZChd8aRdsrguzCfp59+5fR/SJu/6OjXmBfjI
/xywlnyhgYkDgYYABADK4Fl5zgToenInkl8JUd3xrPaGH7zTkigLvGyKsxxeg2G5
DWHqTqUBPH+Db7jpEEru9J416GZqX02Q+qxk7wDrfgFMkA1RLorW1zjoM5V81TVa
3u/zaI95dyBMUhZ8Jjj8DdQjZ7+ai4SC7FHeEd2kVXDnyJmmtMu/KWrh0fqmtSl7
aA==
-----END PRIVATE KEY-----
"""
    auto p521_pub = """
-----BEGIN PUBLIC KEY-----
MIGbMBAGByqGSM49AgEGBSuBBAAjA4GGAAQAyuBZec4E6HpyJ5JfCVHd8az2hh+8
05IoC7xsirMcXoNhuQ1h6k6lATx/g2+46RBK7vSeNehmal9NkPqsZO8A634BTJAN
US6K1tc46DOVfNU1Wt7v82iPeXcgTFIWfCY4/A3UI2e/mouEguxR3hHdpFVw58iZ
prTLvylq4dH6prUpe2g=
-----END PUBLIC KEY-----
"""

    # --- key_info reports the curve ---
    assert crypto.key_info(p384_pub).curve == "P-384"
    assert crypto.key_info(p521_pub).curve == "P-521"

    # --- P-384 / ES384: sign -> verify round-trip and JOSE width ---
    auto s384 = crypto.p384_sha384_sign(p384_priv, msg)
    assert len(s384) == 96
    assert crypto.p384_sha384_verify(p384_pub, msg, s384)
    assert not crypto.p384_sha384_verify(p384_pub, "tampered", s384)

    # --- P-521 / ES512: sign -> verify round-trip and JOSE width ---
    auto s521 = crypto.p521_sha512_sign(p521_priv, msg)
    assert len(s521) == 132
    assert crypto.p521_sha512_verify(p521_pub, msg, s521)
    assert not crypto.p521_sha512_verify(p521_pub, "tampered", s521)

    # --- Interop: verify OpenSSL-produced signatures (external KAT) ---
    assert crypto.p384_sha384_verify(
        p384_pub,
        msg,
        hex.decode(
            "547710388c37796b43607f5fd1b676945036c3b743f2edb3acb6e9d9728801edbaba81a03e456a8eb3b6dc0cfa59f5cfc7b62f9712507347c42d7a39a305854b200094fffe6ce004d1ade66b80f963d406c17111541606c9e3cbd4d406f0f92f",
        ),
    )
    assert crypto.p521_sha512_verify(
        p521_pub,
        msg,
        hex.decode(
            "003f3d30cf4ae64a4dbc9206cf07535542af69f8f93700890a6d5ef4ee2ec688b07d9cb490d0f89ab15aa3f135c6d239006cc0e831379283fab3e4d2563f33ac99a901d89b01c843ec2195d5c028c7c0131b42649117eb257efb25cd00b232938847b36b0ea78e1a3794fc34a209dc021e3ae49853413a33ca0677630459edcb3b5d1bdf",
        ),
    )

    # --- JWK x/y import reconstructs a key that verifies the same signature ---
    auto p384_imported = crypto.p384_public_key_pem(
        hex.decode(
            "73c70617b7df1060d6e8bea1dcc527c18d5c1926ddc175d78031301789e02a4fdb92ce6efa7918536974e7c2db1bcd34",
        ),
        hex.decode(
            "dbf3fea2b31b7fc9b102c07fa110e63c363c8ee5df1f00fc5560d4428cfb1f17225d9a951c7a20c7eb7dc7ce7da82d81",
        ),
    )
    assert crypto.p384_sha384_verify(
        p384_imported,
        msg,
        hex.decode(
            "547710388c37796b43607f5fd1b676945036c3b743f2edb3acb6e9d9728801edbaba81a03e456a8eb3b6dc0cfa59f5cfc7b62f9712507347c42d7a39a305854b200094fffe6ce004d1ade66b80f963d406c17111541606c9e3cbd4d406f0f92f",
        ),
    )
    auto p521_imported = crypto.p521_public_key_pem(
        hex.decode(
            "00cae05979ce04e87a7227925f0951ddf1acf6861fbcd392280bbc6c8ab31c5e8361b90d61ea4ea5013c7f836fb8e9104aeef49e35e8666a5f4d90faac64ef00eb7e",
        ),
        hex.decode(
            "014c900d512e8ad6d738e833957cd5355adeeff3688f7977204c52167c2638fc0dd42367bf9a8b8482ec51de11dda45570e7c899a6b4cbbf296ae1d1faa6b5297b68",
        ),
    )
    assert crypto.p521_sha512_verify(
        p521_imported,
        msg,
        hex.decode(
            "003f3d30cf4ae64a4dbc9206cf07535542af69f8f93700890a6d5ef4ee2ec688b07d9cb490d0f89ab15aa3f135c6d239006cc0e831379283fab3e4d2563f33ac99a901d89b01c843ec2195d5c028c7c0131b42649117eb257efb25cd00b232938847b36b0ea78e1a3794fc34a209dc021e3ae49853413a33ca0677630459edcb3b5d1bdf",
        ),
    )

    # --- Wrong curve: a P-384 key cannot be used on the P-521 path (raises) ---
    auto raised = False
    try:
        crypto.p521_sha512_sign(p384_priv, msg)
    except ValueError:
        raised = True
    assert raised
    # Verify with a wrong-curve public key is a plain False, never a panic.
    assert not crypto.p384_sha384_verify(p521_pub, msg, s384)

    # --- Off-curve / wrong-length coordinates are rejected on import ---
    raised = False
    try:
        crypto.p384_public_key_pem(
            hex.decode(
                "73c70617b7df1060d6e8bea1dcc527c18d5c1926ddc175d78031301789e02a4fdb92ce6efa7918536974e7c2db1bcd00",
            ),
            hex.decode(
                "dbf3fea2b31b7fc9b102c07fa110e63c363c8ee5df1f00fc5560d4428cfb1f17225d9a951c7a20c7eb7dc7ce7da82d81",
            ),
        )
    except ValueError:
        raised = True
    assert raised
    raised = False
    try:
        crypto.p384_public_key_pem(
            hex.decode("00"),
            hex.decode(
                "dbf3fea2b31b7fc9b102c07fa110e63c363c8ee5df1f00fc5560d4428cfb1f17225d9a951c7a20c7eb7dc7ce7da82d81",
            ),
        )
    except ValueError:
        raised = True
    assert raised

    print("crypto.ecdsa_curves: PASS")
