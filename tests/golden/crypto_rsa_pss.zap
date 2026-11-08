# Golden fixture: RSA-PSS (PS256/384/512) and RSA PKCS#1 v1.5 SHA-384/512
# (RS384/RS512) signing & verification (ticket #768, D-0133).
#
# Broadens RSA beyond PKCS#1 v1.5/SHA-256 (#628/D-0102): PS256/384/512 are the
# JOSE-recommended RSASSA-PSS scheme (salt length = digest length, MGF1 with the
# same hash); RS384/RS512 are PKCS#1 v1.5 with the larger SHA-2 digests.
#
# The same fixed 2048-bit key as crypto_rsa.zap. RS384/RS512 are deterministic,
# so each signature is fingerprinted with crypto.sha256(sig). PSS is randomized,
# so PSS signing is checked by sign->verify round-trip; interoperability with an
# external signer is pinned by verifying OpenSSL-produced PSS signatures embedded
# as hex. Role/family binding and tamper-rejection are asserted. Intent docs.
#
# Message: "The quick brown fox jumps over the lazy dog" (43 bytes); 256-byte sig.

def main():
    auto priv = """
-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQDKbtyBPk4EzhGV
l80NlGlDARSNGdX/GicuI9FNbb8kuui3Zq3zx8zS0muZUToT7kI2scUoFJSoVX0K
WtT+MMV3J12z8dQmGKawBY7A7Kl5PsLncDbMPJo/ULW39dDbGEC604x6AZlhugfA
oyJEbabllJmcy/gsEZlv3qT2lkpCv9cAHbx3a3yfykj12SmcW1nckn24GxIsz98X
BW6D5oNtqPpFrFs9V1XFsghdw0ptQj1phlz1XropG6Tkybl7uv5ICywClgXcKCvv
5PMWo6vhWML1U5bK2GUp72Us7foznybcBKOzcfekTo5VaNqGib3h+pWiCR36MVKy
a7I+aGBlAgMBAAECggEALOafKVmxA3bTxqsXk/oFyRNtxAhlg4KBC0Ztj++WVKbS
pI91aRu5rk93B1hRegdAj5IpyYSbHqVrqUbRdJpLymykZ5vtsw+HtYZeDiPQSXDm
2V/1/6I3knCsBeRT0a0U+nue7pb5NwiE3LEykhwCR20JD5LcdqXmsz4euAVI7nnq
v1lyRYFklvSuCTqdjEG2S+NS0qUVGpUNq2F1+56sH4sdcLU0IH7ZNxqP8FKIumAv
UQIxq7oFrNRGOkG6JnUz82kI+4I9QO8qXOFjEY6TScvttbdWU6eASNcsfzmDn/TL
yIIhgUPU+fYQ1dZ1k3dlBBLoz1VDL1JHKZpfy8u8VwKBgQDsOFOKtm3SILARTfEC
dRfAKZga8SPIGq8IEfNYCnP+8RlWdDnKLQxkIvc714Kta4aeqOWfUcsf5W0dFiBt
VZFVaQ2s92E2xk9wYG/hB81Ly6pysSvvaZSXp5mdvB6F8PXIaduEDUJPdjZPuF13
HfFJvu9xKy0wm8mQALG6TTuHtwKBgQDbYkTKQ71bvBNAoVXYk3gkMqVQHnwWm+mp
JQy1x9ka7qPSfXD3i+LVbIcFtXewuUQMRVfPpb+EV2ylt/847rDwXnvxqcU41dvq
PLMWiDe0UdFwMS/r3mAT+1BAixypqDCCo8EYnY9J0ybNtAe+kEg1zcpu9nLICEV2
tJ9uRnAAwwKBgFNk+1FwYEsb0Sz6yEhm0FC+Hn38ffZqPTrrI28Embj6o0hbVf+j
ccl0crZEulTBLlCbLh3WAZEsLR5krmQLHnj50qMq3r44utjgov7eA0NsDqvZErq+
GoCD4f7G6Du/OvZbMO1oxO1KvZfdbdncz0+KwfvBE34iscFODXDJTWf3AoGAVY7q
kxN+CwO7Ym5JQ5Cnvz49OJTbAhpGM2Aup8pMAnPlUhb38qMMkP2lXvWZu0NVpY95
opLasStDRRIQKEz2lpfSsLnsH7FP9fc8ROJVDy8FhkK4lL2CLOjLMJDQWd4tJRXm
Hcp88ma18G1ww8ITV2BtvX7B3v9Sz+u7bc9sqO8CgYBs0R4nshiTZaQdkOo+LCqz
t5iv2T7qc/St594jwpT95hjQ6QOtDHxJZIhXtnjuO8dkDk8POMRg9A+U3DKrouXZ
8eewVQrCrPa1KU4a+ABW1R/wabVW/eqE7V4iWCUx92Z/IpH6CHfughuVa7lp68RY
fSeFwf+I61320pUgZ7z03w==
-----END PRIVATE KEY-----
"""
    auto pub = """
-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAym7cgT5OBM4RlZfNDZRp
QwEUjRnV/xonLiPRTW2/JLrot2at88fM0tJrmVE6E+5CNrHFKBSUqFV9ClrU/jDF
dydds/HUJhimsAWOwOypeT7C53A2zDyaP1C1t/XQ2xhAutOMegGZYboHwKMiRG2m
5ZSZnMv4LBGZb96k9pZKQr/XAB28d2t8n8pI9dkpnFtZ3JJ9uBsSLM/fFwVug+aD
baj6RaxbPVdVxbIIXcNKbUI9aYZc9V66KRuk5Mm5e7r+SAssApYF3Cgr7+TzFqOr
4VjC9VOWythlKe9lLO36M58m3ASjs3H3pE6OVWjahom94fqVogkd+jFSsmuyPmhg
ZQIDAQAB
-----END PUBLIC KEY-----
"""
    auto msg = "The quick brown fox jumps over the lazy dog"

    # --- RS384 / RS512 (PKCS#1 v1.5, deterministic): fingerprint + verify ---
    print(crypto.sha256(crypto.rsa_sha384_sign(priv, msg)))
    print(crypto.sha256(crypto.rsa_sha512_sign(priv, msg)))
    assert crypto.rsa_sha384_verify(pub, msg, crypto.rsa_sha384_sign(priv, msg))
    assert crypto.rsa_sha512_verify(pub, msg, crypto.rsa_sha512_sign(priv, msg))
    assert len(crypto.rsa_sha384_sign(priv, msg)) == 256
    assert len(crypto.rsa_sha512_sign(priv, msg)) == 256
    # A tampered message does not verify.
    assert not crypto.rsa_sha384_verify(pub, "tampered", crypto.rsa_sha384_sign(priv, msg))

    # --- PSS (PS256/384/512): sign -> verify round-trip (randomized salt) ---
    assert crypto.rsa_pss_sha256_verify(pub, msg, crypto.rsa_pss_sha256_sign(priv, msg))
    assert crypto.rsa_pss_sha384_verify(pub, msg, crypto.rsa_pss_sha384_sign(priv, msg))
    assert crypto.rsa_pss_sha512_verify(pub, msg, crypto.rsa_pss_sha512_sign(priv, msg))
    assert len(crypto.rsa_pss_sha256_sign(priv, msg)) == 256
    # Two PSS signatures over the same message differ (random salt) yet both verify.
    auto a = crypto.rsa_pss_sha256_sign(priv, msg)
    auto b = crypto.rsa_pss_sha256_sign(priv, msg)
    assert a != b
    assert crypto.rsa_pss_sha256_verify(pub, msg, a)
    assert crypto.rsa_pss_sha256_verify(pub, msg, b)
    # A tampered message does not verify under PSS.
    assert not crypto.rsa_pss_sha256_verify(pub, "tampered", a)

    # --- Interop: verify OpenSSL-produced PSS signatures (external KAT) ---
    assert crypto.rsa_pss_sha256_verify(
        pub,
        msg,
        hex.decode(
            "2b2c0586a3677283dfca95a61d2d96afadc862dd5513a833296be9b2dca50ef246a4bf9d600dcd9e45686a8a198f2b5d235bb97e0fb9ff1da4005d496edaa47708ff58b0de2b8e0b2c1a5ed051826886f2557458b00ba2bbd5446139a6a42779fdd567aa2eb040f907a9913b134a05233e33b52a89f34ad7b3f53ea4912653b0957eb4ca5c74217760a7d952a4eaf0bc0b01dae22b83a5461897a59d445932036bd1ee380c7db7ba9537e550b8e633c7528aca02625846fbd0e4043d63afe66f9f2f9ce94ebb9822bf5d57db4797c30f052aaf457a89fc34d6700fee59492d94f617b026d30f650dc543e34b3b0f8d59dad7b5aff6e4c3f770ef6ff3dd2493f4",
        ),
    )
    assert crypto.rsa_pss_sha384_verify(
        pub,
        msg,
        hex.decode(
            "097727d4ecd8ce2871d4daa5f0696bff4ad0e7b87b168bce6c4069147d9a62408fe83f5b442b6b42ae9de63d96ec76a65b17253489fbbba9de29544ae19a83a09ac87b94d9ad125af8bd09a13bb7c6c27597e4b5b8165cda2ffe771eac37a5f312e29a6db30500367f2cf8ad1bdc96a36d00cd0a4e7106d37aa962ccdeab036cbe4361d1270d19e172c504d4f17520df156163cf3cf05d5e4a12022fc3348b21fd55cf7e84de71989548192308a7a926c0b0caf7262152d02d0093cfde8fd31aceea11139ed0dbbede9bbb4f5a388f9c670f142b7e4b3824cae657ea1ea9b4f227dd9f43ad1a1c753c0b44dae814a5581d463d5c05dcbe08575e46ee6031333c",
        ),
    )
    assert crypto.rsa_pss_sha512_verify(
        pub,
        msg,
        hex.decode(
            "5c658f0575900765e47321e5a9eaec33ea27dcf6ab97573348326aa05cdf4ba0f1ffd7847adc00e60ad7aa438d6ab5867ec328aa48630534b8a3a204dee34bd54e5699e17853be577108bdfcf3173de715c61871cdfc2e3741e6141cf0a54f2fbc66920168aae0d48d18ccf7b9d8d78a98b09935a201405bf8938038b3210547ee34b142743832f94c497d43456ff0aa8f02be5efb82a4f7c47e9d0a88086fa53997784f26c00218a4181cafa7c77b02db145056154ae9791f5373407d074c4ee4b418c635d6694cf402667767054139f73f88593137646de3101d2f388450503bc85f00fde10378d6bb961bc22052c9d283f934055c9ddc0db1e2cd154fed1b",
        ),
    )
    # A PS256 signature must not verify as PS384 (hash binding).
    assert not crypto.rsa_pss_sha384_verify(pub, msg, hex.decode("2b2c0586a3677283dfca95a61d2d96afadc862dd5513a833296be9b2dca50ef246a4bf9d600dcd9e45686a8a198f2b5d235bb97e0fb9ff1da4005d496edaa47708ff58b0de2b8e0b2c1a5ed051826886f2557458b00ba2bbd5446139a6a42779fdd567aa2eb040f907a9913b134a05233e33b52a89f34ad7b3f53ea4912653b0957eb4ca5c74217760a7d952a4eaf0bc0b01dae22b83a5461897a59d445932036bd1ee380c7db7ba9537e550b8e633c7528aca02625846fbd0e4043d63afe66f9f2f9ce94ebb9822bf5d57db4797c30f052aaf457a89fc34d6700fee59492d94f617b026d30f650dc543e34b3b0f8d59dad7b5aff6e4c3f770ef6ff3dd2493f4"))

    # --- Role binding: a public key cannot sign (raises ValueError) ---
    auto raised = False
    try:
        crypto.rsa_sha384_sign(pub, msg)
    except ValueError:
        raised = True
    assert raised
    raised = False
    try:
        crypto.rsa_pss_sha256_sign(pub, msg)
    except ValueError:
        raised = True
    assert raised

    print("crypto.rsa_pss: PASS")
