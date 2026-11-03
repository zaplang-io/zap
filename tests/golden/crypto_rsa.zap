# Golden fixture: RSA PKCS#1 v1.5 signing (ticket #628, D-0102).
#
# `crypto.rsa_sha1_sign` / `rsa_sha256_sign` load a PEM RSA private key and
# return the RAW signature bytes over the message digest (the primitive behind
# CloudFront signed URLs, zaplang-io/aws#25/#440). PKCS#1 v1.5 is deterministic,
# so the signature is a stable vector; there is no bytes->hex builtin, so each
# signature is fingerprinted with `crypto.sha256(sig)` (byte-for-byte sensitive)
# alongside its length. The same fixed key pair is supplied in both PKCS#1
# (`BEGIN RSA PRIVATE KEY`) and PKCS#8 (`BEGIN PRIVATE KEY`) PEM to prove both
# load and sign identically. `rsa_sha256_verify` checks the signature against
# the matching public key (True), and a tampered message fails (False).
#
# Message: "The quick brown fox jumps over the lazy dog" (43 bytes).
# A 2048-bit key yields a 256-byte signature. Expected fingerprints computed
# with OpenSSL (`openssl dgst -sha1/-sha256 -sign` | `openssl dgst -sha256`):
#   sha256(rsa_sha1_sign)   = baf94a175b0719011f60fdd7b151b42ea683f78625f4bc1c80d896b3c57cfd77
#   sha256(rsa_sha256_sign) = 502cd1d80be15b7a5b19a55d2d1c7dabbae531c61e4287cb2d527afee4ef804e
def main():
    auto pkcs8 = """
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
    auto pkcs1 = """
-----BEGIN RSA PRIVATE KEY-----
MIIEogIBAAKCAQEAym7cgT5OBM4RlZfNDZRpQwEUjRnV/xonLiPRTW2/JLrot2at
88fM0tJrmVE6E+5CNrHFKBSUqFV9ClrU/jDFdydds/HUJhimsAWOwOypeT7C53A2
zDyaP1C1t/XQ2xhAutOMegGZYboHwKMiRG2m5ZSZnMv4LBGZb96k9pZKQr/XAB28
d2t8n8pI9dkpnFtZ3JJ9uBsSLM/fFwVug+aDbaj6RaxbPVdVxbIIXcNKbUI9aYZc
9V66KRuk5Mm5e7r+SAssApYF3Cgr7+TzFqOr4VjC9VOWythlKe9lLO36M58m3ASj
s3H3pE6OVWjahom94fqVogkd+jFSsmuyPmhgZQIDAQABAoIBACzmnylZsQN208ar
F5P6BckTbcQIZYOCgQtGbY/vllSm0qSPdWkbua5PdwdYUXoHQI+SKcmEmx6la6lG
0XSaS8pspGeb7bMPh7WGXg4j0Elw5tlf9f+iN5JwrAXkU9GtFPp7nu6W+TcIhNyx
MpIcAkdtCQ+S3Hal5rM+HrgFSO556r9ZckWBZJb0rgk6nYxBtkvjUtKlFRqVDath
dfuerB+LHXC1NCB+2Tcaj/BSiLpgL1ECMau6BazURjpBuiZ1M/NpCPuCPUDvKlzh
YxGOk0nL7bW3VlOngEjXLH85g5/0y8iCIYFD1Pn2ENXWdZN3ZQQS6M9VQy9SRyma
X8vLvFcCgYEA7DhTirZt0iCwEU3xAnUXwCmYGvEjyBqvCBHzWApz/vEZVnQ5yi0M
ZCL3O9eCrWuGnqjln1HLH+VtHRYgbVWRVWkNrPdhNsZPcGBv4QfNS8uqcrEr72mU
l6eZnbwehfD1yGnbhA1CT3Y2T7hddx3xSb7vcSstMJvJkACxuk07h7cCgYEA22JE
ykO9W7wTQKFV2JN4JDKlUB58FpvpqSUMtcfZGu6j0n1w94vi1WyHBbV3sLlEDEVX
z6W/hFdspbf/OO6w8F578anFONXb6jyzFog3tFHRcDEv695gE/tQQIscqagwgqPB
GJ2PSdMmzbQHvpBINc3KbvZyyAhFdrSfbkZwAMMCgYBTZPtRcGBLG9Es+shIZtBQ
vh59/H32aj066yNvBJm4+qNIW1X/o3HJdHK2RLpUwS5Qmy4d1gGRLC0eZK5kCx54
+dKjKt6+OLrY4KL+3gNDbA6r2RK6vhqAg+H+xug7vzr2WzDtaMTtSr2X3W3Z3M9P
isH7wRN+IrHBTg1wyU1n9wKBgFWO6pMTfgsDu2JuSUOQp78+PTiU2wIaRjNgLqfK
TAJz5VIW9/KjDJD9pV71mbtDVaWPeaKS2rErQ0USEChM9paX0rC57B+xT/X3PETi
VQ8vBYZCuJS9gizoyzCQ0FneLSUV5h3KfPJmtfBtcMPCE1dgbb1+wd7/Us/ru23P
bKjvAoGAbNEeJ7IYk2WkHZDqPiwqs7eYr9k+6nP0refeI8KU/eYY0OkDrQx8SWSI
V7Z47jvHZA5PDzjEYPQPlNwyq6Ll2fHnsFUKwqz2tSlOGvgAVtUf8Gm1Vv3qhO1e
IlglMfdmfyKR+gh37oIblWu5aevEWH0nhcH/iOtd9tKVIGe89N8=
-----END RSA PRIVATE KEY-----
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

    # RSA-SHA1: raw signature length, then its byte-for-byte fingerprint from
    # both the PKCS#8 and PKCS#1 encodings of the same key (must agree).
    auto s1_pkcs8 = crypto.rsa_sha1_sign(pkcs8, msg)
    auto s1_pkcs1 = crypto.rsa_sha1_sign(pkcs1, msg)
    print(len(s1_pkcs8))
    print(crypto.sha256(s1_pkcs8))
    print(crypto.sha256(s1_pkcs1))

    # RSA-SHA256: same, over the SHA-256 digest.
    auto s256_pkcs8 = crypto.rsa_sha256_sign(pkcs8, msg)
    auto s256_pkcs1 = crypto.rsa_sha256_sign(pkcs1, msg)
    print(len(s256_pkcs8))
    print(crypto.sha256(s256_pkcs8))
    print(crypto.sha256(s256_pkcs1))

    # Verification against the public key: the genuine signature verifies, a
    # tampered message does not.
    print(crypto.rsa_sha256_verify(pub, msg, s256_pkcs8))
    print(crypto.rsa_sha256_verify(pub, "tampered", s256_pkcs8))
