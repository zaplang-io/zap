# #642: RSA-named APIs enforce ordinary RSA + PKCS#1 v1.5, never generic EVP.
# Fixed disposable test keys generated with OpenSSL 3.5.0:
#   genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256/P-384
#   genpkey -algorithm Ed25519/Ed448/X25519/RSA/RSA-PSS
#   genpkey -genparam -algorithm DSA -pkeyopt dsa_paramgen_bits:2048
#   genpkey -paramfile dsa.params
# Foreign valid signatures: dgst -sha256 -sign KEY; ordinary-RSA PSS uses
# -sigopt rsa_padding_mode:pss -sigopt rsa_pss_saltlen:digest. Each foreign
# signature was independently verified by OpenSSL before embedding below.
# SHA-1/SHA-256 positive fingerprints are sha256(openssl dgst -sha* -sign KEY).
# 2048-bit positive key is shared with crypto_rsa.zap; 3072 tests variable size.

def reject_private(str pem):
    auto sha1_rejected = False
    try:
        crypto.rsa_sha1_sign(pem, "RSA binding regression")
    except ValueError:
        sha1_rejected = True
    assert sha1_rejected
    auto sha256_rejected = False
    try:
        crypto.rsa_sha256_sign(bytes(pem), b"RSA binding regression")
    except ValueError:
        sha256_rejected = True
    assert sha256_rejected

def reject_foreign(str private, str public, bytes valid_signature):
    reject_private(private)
    assert not crypto.rsa_sha256_verify(public, "RSA binding regression", valid_signature)
    assert not crypto.rsa_sha256_verify(bytes(public), b"RSA binding regression", valid_signature)

def valid(
    str private,
    str public,
    bytes message,
    int64 size,
    str sha1_hash,
    str sha256_hash,
):
    auto s1 = crypto.rsa_sha1_sign(private, message)
    auto s256 = crypto.rsa_sha256_sign(bytes(private), message)
    assert len(s1) == size and len(s256) == size
    assert crypto.sha256(s1) == sha1_hash
    assert crypto.sha256(s256) == sha256_hash
    assert crypto.sha256(crypto.rsa_sha1_sign(bytes(private), message)) == sha1_hash
    assert crypto.sha256(crypto.rsa_sha256_sign(private, message)) == sha256_hash
    assert crypto.rsa_sha256_verify(public, message, s256)
    assert crypto.rsa_sha256_verify(bytes(public), message, s256)
    assert not crypto.rsa_sha256_verify(public, message + b"!", s256)
    assert not crypto.rsa_sha256_verify(public, message, s1)
    assert not crypto.rsa_sha256_verify(public, message, b"")
    list[int64] zeros = []
    for i in range(size):
        zeros.append(0)
    assert not crypto.rsa_sha256_verify(public, message, bytes(zeros))

# Mutations use an independent OpenSSL signature, keeping byte-operation
# coverage separate from the sign-result lowering defect tracked in #644.
def reject_damaged(str public, bytes signature):
    assert crypto.rsa_sha256_verify(public, "RSA binding regression", signature)
    assert not crypto.rsa_sha256_verify(public, "RSA binding regression", signature[:-1])
    assert not crypto.rsa_sha256_verify(public, "RSA binding regression", signature + b"x")
    assert not crypto.rsa_sha256_verify(public, "RSA binding regression", bytes([signature[0] ^ 1]) + signature[1:])

def main():
    auto rsa2048 = """
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
    auto rsa_pkcs1 = """
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
    auto rsa_public = """
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
    auto rsa3072 = """
-----BEGIN PRIVATE KEY-----
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
7Ju17wnKFse/TdZuvjb0cm0=
-----END PRIVATE KEY-----
"""
    auto rsa3072_public = """
-----BEGIN PUBLIC KEY-----
MIIBojANBgkqhkiG9w0BAQEFAAOCAY8AMIIBigKCAYEA8eaYydX9ZaSq/FWYt5o8
P8La78w0RcorSbmdlNgVyj4kHWMBiipiKdQmVmEEuqp13iz0M5Vbg8t1tH23XEGC
Mcsrc/AyNVcd1UF/hARTDIYCwhoDNMu69EqgKvjfvLDb7VcUiNV3Jj3poKIeFjMD
wRchzUNtrKTQyS9Wad6uoHFcy/7iieNoVYuQ3UUMfz8KMlrkMeggn6wgpbbn6fc9
JiivRtSaeXzViJzT9t+x4w2/43/rGD7DK2aDUKf33vF4ElZ0UiNYFSBJCGJ1PSOt
4M9khMpWYAg1TzfsZ83eHygIsQHtHIcTVqMENQT8IgDCmhkwb8C5FVC4MmMJDc4T
9A3vWRQ++dmbX+jyfsfjVWH5EJpvublREwVxnIYFsRjtuHuhQPjNK2hR5DPkdFZy
N2uPTwr/BlGr8ogBVUNAXizDzG5NADNbdb5KK0UZi4L8ZVlNV9Ti0bUlrSgLWoBW
LMBTTdJXhC+tgKa2fxSnSUZofAmP8DhJjtinThGLVFm1AgMBAAE=
-----END PUBLIC KEY-----
"""
    auto other_public = """
-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA3Ub7MkAussHLxxST8pTF
T2onnqFURwTCU3xF2WE1R3JQs2s4PWEUyRIxP1uQMKX6RP2KF1h6oLRqm9CG1Ufs
kEmytOQ5xsFkkVqZI54xS+WH+VvDxVsR5NJThgLh58tyqaWF6nsCibPJh9EE7KG7
GirzpA8Yi8FZB6+aMQbWUN8bL+hHNFDxrhXnA/BekJPr95hSB2kaJGU+dtflSEeI
ckvBCpTmOXd5G503Xe5JBFBZaP064mnxR8wBqoHSGQPojiLooI2YQHsuP7257I/M
dj8NOcBdEsrzY9oDG0k4uwy7dg7g4iVnzuJhKZHhciMGcq9ipf3LfiQ9G+iWZGGN
RQIDAQAB
-----END PUBLIC KEY-----
"""
    auto ec256_private = """
-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgeMd1PuqVWtQApVtI
YeLAt3e+e29Qso/nowOhQmhZTgWhRANCAATQ7+9e1ddV4bvS/uMYmCczYBqzUPKD
iV1b4aaMi+/08ZRIaVcSWFy5wg4xcxqkXHDl45aFIuuf8+aosYge9RbE
-----END PRIVATE KEY-----
"""
    auto ec256_public = """
-----BEGIN PUBLIC KEY-----
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE0O/vXtXXVeG70v7jGJgnM2Aas1Dy
g4ldW+GmjIvv9PGUSGlXElhcucIOMXMapFxw5eOWhSLrn/PmqLGIHvUWxA==
-----END PUBLIC KEY-----
"""
    auto ec384_private = """
-----BEGIN PRIVATE KEY-----
MIG2AgEAMBAGByqGSM49AgEGBSuBBAAiBIGeMIGbAgEBBDCWOUK8JaUyRzh7xyFm
XIQHcHKUk5KPdRe3SqwmqU7k0voDMJgI1jcsTQR1RTqmxo+hZANiAASDeWmiHmCD
S+jp6PgycQo20St7osBvEmtMl0dcsnDEk/VSG0+pEVEXZ0Mlb7TnADufzJcQFYZN
OlJ+cAl+jNJ7x7qQhWc+coIXHNo8vg9iW6uLzdbalcl5RikTncUCEcs=
-----END PRIVATE KEY-----
"""
    auto ec384_public = """
-----BEGIN PUBLIC KEY-----
MHYwEAYHKoZIzj0CAQYFK4EEACIDYgAEg3lpoh5gg0vo6ej4MnEKNtEre6LAbxJr
TJdHXLJwxJP1UhtPqRFRF2dDJW+05wA7n8yXEBWGTTpSfnAJfozSe8e6kIVnPnKC
FxzaPL4PYluri83W2pXJeUYpE53FAhHL
-----END PUBLIC KEY-----
"""
    auto ed25519_private = """
-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEIJW5Ec8Pjl2CcCokE/J/5K0+s1LYoq//tEZO3szTBssH
-----END PRIVATE KEY-----
"""
    auto ed25519_public = """
-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAI9mutVlx6TVxOrps8rOwE0LK6hS/jSagilx8or3FmDU=
-----END PUBLIC KEY-----
"""
    auto ed448_private = """
-----BEGIN PRIVATE KEY-----
MEcCAQAwBQYDK2VxBDsEOUW7mb7MXNdVPBUiFuulms+BrmjWzg6pZX+8KyrvRyQU
+WIPtY8lH0TLq60cXSDmnU7iNwMTNpgQ7A==
-----END PRIVATE KEY-----
"""
    auto ed448_public = """
-----BEGIN PUBLIC KEY-----
MEMwBQYDK2VxAzoAKv5UjRoOJbEHyNK/FcjZ9PG6PzGeIAE9TRjuLVQ07Lb8H3nf
Bd8dZpEp37QQCjR0TykIE416P5yA
-----END PUBLIC KEY-----
"""
    auto x25519_private = """
-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VuBCIEIND4EVkfXfrVU05hbliGudsCIEBkJw5e+3lUfJi+jhFf
-----END PRIVATE KEY-----
"""
    auto x25519_public = """
-----BEGIN PUBLIC KEY-----
MCowBQYDK2VuAyEAaBbPq7Reyr+/vpEJ7cD5upfI9+rNx4H9j5TXfQz0Yxg=
-----END PUBLIC KEY-----
"""
    auto pss_private = """
-----BEGIN PRIVATE KEY-----
MIIEuwIBADALBgkqhkiG9w0BAQoEggSnMIIEowIBAAKCAQEA8L78Dm1BcNSH31BJ
lrsGBy7BkmYCYwk1pf0JuLiR7TjJjD9cov5aOTIczMQEjHU6CaIS7CyAREW8ktTo
kBwnJVI4wIfd1TB/m37WySM4qhOlQqlxFHCKBL+Usotjul520h4Rznq7E+6d9Ush
JRb5ThI0jcLdm/BABEyDxlft/Nrhf3IzJLOeMZIxtJrAh2cSIpQSVu0qqqlDjxMw
z3ATuStV2BYqwWi1dewh9TuAleR+RRDm2HvYPKqVvGvacsxkqqEdSqMGSDpRm65Z
d8h6jJXSP3CCJtkvF+X21QrTmRD+VQdjl3a9X301g3zqPYbZVDsxsPvyeVOF+IVp
A4WknQIDAQABAoIBACWXWMHC7xU4ZmfpADicRuZvt1OOAKDosvsSbgZtMYIGXDg9
O5CK4VkteUBC+kfZVSDkcQp2L2gZsIaH3rGD59vkwQ5VGL8ddct4rH6XNJWgAC3O
yeSsZuyzYntXlxCmh+yqu9KPKO+Fkp4W51vEgNSfNJwPGa3r/378WJYrrDJPSJ4z
lQXrgCDOhKmR6BzsxsH5ppxMUlRremhbnCFvLVV1UWd07IotKqZmWaHElExBHDud
fdWgYEz4EueiaIwVhhocnIS3doVcz6MwKz/eflfkx90VfGpYiuOMxLmMTy8ud+dA
dd+useRxF9rALkmcVZxK4MOBV0gg1x+Hj/x3L7kCgYEA+257ADk4IyoR+hD/Mde+
agGPkb1qwgvwhETT2cK05E/BQornahnns/FrBi9nNRHGmw1hVeYDQOS1SJxOx5Lw
q16PTF1OTb/8GkDnkgbImCBZQxtIxkbU+haZA7piUzDbJJoL1QW2tsIyyjqZZoSX
/dB9yjCPVak4iZqDraBWG6UCgYEA9R7NDZH3bG/fSxS5Z1oZU5K9E/+CdJv/6s3X
CLjUqGT2wLiwZPQv01RI+H8fAudUBG8MRbaevj+DlFeZO3bBiHyQoktctSjKYjU2
erCaqZT3cDsBWjPPapgm/733CpLI25hJxio3tyI0JAd2pegJOb+hLy4RPsIH1FNI
VZnUc5kCgYBfWxQtDoZa176/nyiZp5/gpjzj4fmWJF8QWuYZpT1jhVffO7zuQWrd
lMapLtsuhlJTRazPng9H0jVql1VNXU4HQDKKnKxw0knzTwBEImx7PXF9AOEFyaFG
Q8G+ZzaZJGNIK2AnZyZXg6kthzHcbpcSPLsoLmh97VKy5TXhaHifvQKBgBIeELHs
FAmjyzFVes60ziJrOiZ8QDw2O5gzVq2CW9WGgOaFwCP2GILaO/qCCtzAFGmve4y7
jsgpEaSpRN4sTOm1q0sTnuqU9UCFBl3TJ9tY82EMO3FbWv1Z+af1dokFiGsSuoE3
JZ8XnBr9ojtpByWJkHRYR1YIKWP8M+B/2kAhAoGBALUFC1QlaOtM+KKxABvXkSjn
Harivlz6jcR4yVklGKX04tYp18sa0B2GvOp9pUxE/R41dLrGaZb3wRNxRE/gbbf3
uiWzNCp+A1JT/Hbkd23KyLEzixES+8Y9zIgjCO1Cd+Z3ze4SP0nUqydP4jrezeNc
WXC218U7+S1fotgAL6tF
-----END PRIVATE KEY-----
"""
    auto pss_public = """
-----BEGIN PUBLIC KEY-----
MIIBIDALBgkqhkiG9w0BAQoDggEPADCCAQoCggEBAPC+/A5tQXDUh99QSZa7Bgcu
wZJmAmMJNaX9Cbi4ke04yYw/XKL+WjkyHMzEBIx1OgmiEuwsgERFvJLU6JAcJyVS
OMCH3dUwf5t+1skjOKoTpUKpcRRwigS/lLKLY7pedtIeEc56uxPunfVLISUW+U4S
NI3C3ZvwQARMg8ZX7fza4X9yMySznjGSMbSawIdnEiKUElbtKqqpQ48TMM9wE7kr
VdgWKsFotXXsIfU7gJXkfkUQ5th72Dyqlbxr2nLMZKqhHUqjBkg6UZuuWXfIeoyV
0j9wgibZLxfl9tUK05kQ/lUHY5d2vV99NYN86j2G2VQ7MbD78nlThfiFaQOFpJ0C
AwEAAQ==
-----END PUBLIC KEY-----
"""
    auto pss_restricted_private = """
-----BEGIN PRIVATE KEY-----
MIIE8gIBADBBBgkqhkiG9w0BAQowNKAPMA0GCWCGSAFlAwQCAQUAoRwwGgYJKoZI
hvcNAQEIMA0GCWCGSAFlAwQCAQUAogMCASAEggSoMIIEpAIBAAKCAQEAyHP5vnhK
7mQqMXUSNYjcjaZkw9gVB+U9ng2JHRxfz9VFu6dH28lOCY7hC2sKb729WWH6S8jr
JPtbQJReW5/+bl/QipOb4gFowlA0vhvMZKG8d8QfYbitUrgKTCi13m5rm2fgQRMD
njwjnvc+wqzo+KGml5lNnYUS7mtXtMYktQBzoL328Om9ZgHxu96WMpqDpN/PMQl9
wSrjudgfPBZuA98HohU4XSPeG+VdAUI8WyntMz8N+ZvXsg7BfOa9XN1u95HYOzRb
IKnOpNc8tjDcbkQWb3ENHfk899HlFywGJgmzL/EXbpCds9vMR5gcJ18tvKrXm/13
kAmCD4wYEQiJ4wIDAQABAoIBAA9nnkdNflCwwO9ZFGOp121JXtAQMJmRO/mEB8gF
cLWLqdbAhcy1vToN0WyrtrscpUsTtQUS9UM7z+j4HgJDmu2HjoK/0kh9nDvmpw7/
GueW6GF54IGFjEhhnWT58fQcn0drpbfKisojzAcA8EFMwYCuB5v565WR7wEEndJm
Mdb5BWmuUanoRCEoZiHe7LqQAQFuUr1l1KHLSDwa/WG3JYz+MY6vwRGHnORWUAgs
pwuONbVvDO1Of96wwrP+8NwEAEwinf1bjsAPwp8rt18OHibmKO9ckep+by4Xb8w/
C87Ohc8YhEZaPKfk7u0ce5j/fJ9yTUaEClIlJdiGYHnzBQ0CgYEA+vAI8ZG2ra2L
E1uCt/Mi+sD9AlpYJKPmAkBm4Im99qHTEyjDJOluMyLAQi/yQL13DtZmSxFD+wqF
0cY1CrlgoLJWzk7uEodL8iXrpvN/PhIgv/ArS3XDUrHY5/MlF95Jcn6ofRal/bUb
81rJ0/vA4G6BFlcbe8hLIffXJBD0BeUCgYEAzH82ngZuJe/KSiU97MKehF6lhGVq
uEKBcSXrQq0iWGvLHr6Ipz+GRhsQF/jMEhJjlmGwDfA5th3Op1h5sRCs2T4X7124
7Dyg8jqJwzaVme/bRVdfGyNgczsJ2CDBrHBYtVBCj5MHrF2D9F7R2tAYt6STfkkp
aA/YmoL+1Tty1CcCgYBprNInYRTMOSw7X6tB6htN/B3QctlxvK/tPM/WOUuNZIKu
K9laXw9wa28JsIVVbDSKRBHaBRBa1Qib2duWg5DEu7XAtg6eMOnztLJzoVFxg1LV
Yb/pzFMQLSNKMuPpFExb8ijkXLhUioOVzSL3qaqFGSQ1k0NYsnWpfWTVpGoPXQKB
gQCCAvb0bN4mnFcd4KOIHQuu2IWesOOylgW5rAPwdiZ51L5ko3o56rcxZw9KTlFf
w/UwrJbDpdDrKfQyjJuBqTpbO5D4C7v/5WPl3LWtOMzP+6CKa0v58vnlCMnvcJPx
T9P2IjrPEQk12paEDKya0uaECswC4/zBPiHVyia95nfQzQKBgQDDgCG5qiFukxAt
PZmcuWQJAAv8mRpztJHwSwOdJQUzuOwg93jR4mUGl0O5cQkqUUZ5RimqmYMexMV6
l3D4p2jG61+QttqwSxs4rmTEtBg//j1CweYjhMQgkAY/KXSi7EM9DO5qw5om9qvJ
GDAIrB7JvxXdZrExDz0K6+OUyfxWSQ==
-----END PRIVATE KEY-----
"""
    auto pss_restricted_public = """
-----BEGIN PUBLIC KEY-----
MIIBVjBBBgkqhkiG9w0BAQowNKAPMA0GCWCGSAFlAwQCAQUAoRwwGgYJKoZIhvcN
AQEIMA0GCWCGSAFlAwQCAQUAogMCASADggEPADCCAQoCggEBAMhz+b54Su5kKjF1
EjWI3I2mZMPYFQflPZ4NiR0cX8/VRbunR9vJTgmO4QtrCm+9vVlh+kvI6yT7W0CU
Xluf/m5f0IqTm+IBaMJQNL4bzGShvHfEH2G4rVK4Ckwotd5ua5tn4EETA548I573
PsKs6PihppeZTZ2FEu5rV7TGJLUAc6C99vDpvWYB8bveljKag6TfzzEJfcEq47nY
HzwWbgPfB6IVOF0j3hvlXQFCPFsp7TM/Dfmb17IOwXzmvVzdbveR2Ds0WyCpzqTX
PLYw3G5EFm9xDR35PPfR5RcsBiYJsy/xF26QnbPbzEeYHCdfLbyq15v9d5AJgg+M
GBEIieMCAwEAAQ==
-----END PUBLIC KEY-----
"""
    auto dsa_private = """
-----BEGIN PRIVATE KEY-----
MIICXQIBADCCAjYGByqGSM44BAEwggIpAoIBAQC29cZAWy9D4qujdFsxV4CTZygs
ONuN0enMx/FUJgiqfx3YEvxIzIZ9yiJLgbagsEftFM4dNLK9ZAYbmKOaWJCzGGeG
QJYOV++N7MI+eKJf4dQKyyfnjYf4i+oVQBrBZuxhG8ABgrT4JSfl6P1yNYjYYidC
AWfbuSviROVJU0Wop9ecKcyxWXu6auUvt+QLkeFpqo/KEFuJbU8coCNDc0fIaCfw
QU7tNM13Is1Xf8GF2NPWE4l7il44f/X02+O7e24WlJ24WwoM0j6b/NvUzVB0Ddji
5tUjYbKlIndnEcTTiR2ImoLSEI0yusWHMlaH7gVq5kNlq/jCJaTHrhq6dXK3Ah0A
yyiMqouTgQSBFQ/cR3/d4IcgCJn0s7yc/YU/zwKCAQEAlgtsi7CROvrU9RPJRGwR
G7Y+1EYkOx+lIsTnANelOMqOiT7yO/pTqjHVF97RilDmx3xjij+pAe1xtpkyVoP8
RbEiIBE0mtatqMQ2LjB0C0cITrKCS9XGQ0WepsTEld43JYEFx3crWBenT6AN2iQu
xHMcUWV24w/uEoM+7Byg7Pp1klMqHvp4XZQHX2ulgMCW9QSgUoPCxfsxX93TVlOk
SPgwJD2hwf2n4C28/hPSI6s6xfHyMF/LmOulKnygJIZyapxD6oQxDZIfFhJKCPh6
1PvNaXiYUR2AKMqHrDai5nb3cZJVH3Aa5w4+mDYy9aWqPEWVAO37AUyiuLQ9NOkp
ZQQeAhw4xV8o3+4cT0bYtOGKt+WTp+jhh3VjRzjFwo++
-----END PRIVATE KEY-----
"""
    auto dsa_public = """
-----BEGIN PUBLIC KEY-----
MIIDRDCCAjYGByqGSM44BAEwggIpAoIBAQC29cZAWy9D4qujdFsxV4CTZygsONuN
0enMx/FUJgiqfx3YEvxIzIZ9yiJLgbagsEftFM4dNLK9ZAYbmKOaWJCzGGeGQJYO
V++N7MI+eKJf4dQKyyfnjYf4i+oVQBrBZuxhG8ABgrT4JSfl6P1yNYjYYidCAWfb
uSviROVJU0Wop9ecKcyxWXu6auUvt+QLkeFpqo/KEFuJbU8coCNDc0fIaCfwQU7t
NM13Is1Xf8GF2NPWE4l7il44f/X02+O7e24WlJ24WwoM0j6b/NvUzVB0Ddji5tUj
YbKlIndnEcTTiR2ImoLSEI0yusWHMlaH7gVq5kNlq/jCJaTHrhq6dXK3Ah0AyyiM
qouTgQSBFQ/cR3/d4IcgCJn0s7yc/YU/zwKCAQEAlgtsi7CROvrU9RPJRGwRG7Y+
1EYkOx+lIsTnANelOMqOiT7yO/pTqjHVF97RilDmx3xjij+pAe1xtpkyVoP8RbEi
IBE0mtatqMQ2LjB0C0cITrKCS9XGQ0WepsTEld43JYEFx3crWBenT6AN2iQuxHMc
UWV24w/uEoM+7Byg7Pp1klMqHvp4XZQHX2ulgMCW9QSgUoPCxfsxX93TVlOkSPgw
JD2hwf2n4C28/hPSI6s6xfHyMF/LmOulKnygJIZyapxD6oQxDZIfFhJKCPh61PvN
aXiYUR2AKMqHrDai5nb3cZJVH3Aa5w4+mDYy9aWqPEWVAO37AUyiuLQ9NOkpZQOC
AQYAAoIBAQCeaflxSis06gowuwcI9yd82eJLV622GqfCWYBjTXcQkZXp/oofs7D1
C10CsTiBHFysu8MmUd9Lds2wTCDCdXSGVe4sdOEsdivLDWnC5pK1usE3mTdB7Xmh
HQyJVU+OEOWiMZa2B2tQlnduYy9xTay37MrW53gWPXvjJqmnk5wJ2ZzfuwV9ci3O
l/wHNTMRA19So5r/zfaddtWwR0ziZhVKU5FWn1kfEwb4Q1nm5oIUDYVTe02OmASv
fe9IP887/OhfdhcLzdUgF0x30UXC4m40166VawzuY3tmccOvgcAPQlfjkJzuhIu0
EnLU/MsAJVuSdi0Tfruwv3A4O0fKcZ3N
-----END PUBLIC KEY-----
"""
    valid(
        rsa2048,
        rsa_public,
        b"RSA binding regression",
        256,
        "3d13197e2aae8ba45fab9d6d51579120b2c57b792d67cfc0e6b7e51178f60afd",
        "6ed2d9f09d865a856a4e1935fe3c6a5742a78f4e0c14ef0b5102d3ad3f681268",
    )
    valid(
        rsa2048,
        rsa_public,
        b"",
        256,
        "dd43600d76a8b9b0c53c75248604060f93cf859c9022dc909bd92d277e5b3465",
        "cf462d1e3cb0f55ffc130ac6012de982ec2a43664dd44aa0d865a2874b7c15c5",
    )
    valid(
        rsa2048,
        rsa_public,
        bytes([0, 1, 127, 128, 255]),
        256,
        "54768ab95a5836cee49bd5a4a4d0bddb4b1842624344701fa97ea43180809e5a",
        "deee635750b99204eac2bab003157c656b5076d779807edea97a9840bcbc2c53",
    )
    valid(
        rsa_pkcs1,
        rsa_public,
        b"RSA binding regression",
        256,
        "3d13197e2aae8ba45fab9d6d51579120b2c57b792d67cfc0e6b7e51178f60afd",
        "6ed2d9f09d865a856a4e1935fe3c6a5742a78f4e0c14ef0b5102d3ad3f681268",
    )
    valid(
        rsa_pkcs1,
        rsa_public,
        b"",
        256,
        "dd43600d76a8b9b0c53c75248604060f93cf859c9022dc909bd92d277e5b3465",
        "cf462d1e3cb0f55ffc130ac6012de982ec2a43664dd44aa0d865a2874b7c15c5",
    )
    valid(
        rsa_pkcs1,
        rsa_public,
        bytes([0, 1, 127, 128, 255]),
        256,
        "54768ab95a5836cee49bd5a4a4d0bddb4b1842624344701fa97ea43180809e5a",
        "deee635750b99204eac2bab003157c656b5076d779807edea97a9840bcbc2c53",
    )
    valid(
        rsa3072,
        rsa3072_public,
        b"RSA binding regression",
        384,
        "db97a6bf261d34ddaeff547f6f4365af8f8ad1f45a99a6e4804a0b4cd45dc478",
        "b46cfc2650c7d227c5f4ba4919fa8171809bab27ef28182823fa06dfba36af2d",
    )
    valid(
        rsa3072,
        rsa3072_public,
        b"",
        384,
        "f98763e71accfedcf61947faea470c5a49da7f0708f4ea90454cb4db6c466c8c",
        "a47817a1fab17b3b69106aafd9c3e8ef6de804ad94823926db0d7a0c67af4c0b",
    )
    valid(
        rsa3072,
        rsa3072_public,
        bytes([0, 1, 127, 128, 255]),
        384,
        "47f2e0e4af0482f84ae7777f4eb90c194dce77dc3436e0ead9e3df8815031540",
        "25dbe62e69e5216a8197104379beea9eb8533fb5a48779b5073415c51aa7f135",
    )
    reject_foreign(
        ec256_private,
        ec256_public,
        hex.decode(
            "3046022100957bfcf33400a3d957d572b7d447098cc905fe97074d819e466400969e5c2a9902210093ee133f5a88dbbf6e9e1067fc517c23dd8bae0f14cf662418ead67501bbd1a7",
        ),
    )
    reject_foreign(
        ec384_private,
        ec384_public,
        hex.decode(
            "306502310094595ef00cba0f6bb283df56db744aec738bf706ecd7c240721c3e12f1dd83eb72653032828876e4e8672f75889e0570023025ac2f9598d7f185a02b87355077f0af63709d7e87652176a20dbdbcf1a153d0ee4df4018be86112cf2c3edddad8b896",
        ),
    )
    reject_foreign(
        ed25519_private,
        ed25519_public,
        hex.decode(
            "f2645a1d90896354160e9c0bdaf7107df964c60b80fc86e630ecd42ed54d5f9f0ecabba6f5aaefab7ad47543cdc399c8c074cca0817903cd191e2dd69eb87505",
        ),
    )
    reject_foreign(
        ed448_private,
        ed448_public,
        hex.decode(
            "10fb63d2001ea1f0dd6014da5b1612068099e0ae40eb0a5d40321939ef4aecf826d9b0e4acb5c9ac70fab0f1a3b05b1d5b5443a3c52a70ea802b3ce38e3737ecddadb02631b87a652d92ec715bd26abc5b984acf285b3504e459241dfaa0aeefcb80e0c7c065dfbcdf234881f08edfbf1a00",
        ),
    )
    reject_foreign(
        x25519_private,
        x25519_public,
        hex.decode("6e6f742061207369676e696e67206b6579"),
    )
    reject_foreign(
        pss_private,
        pss_public,
        hex.decode(
            "a98a85fd92f182fa522b2433d4014fe986858a4a042781d50d398d44f24415a20f662265db63659fbb7e926c7cb549e207dcc000f8ef695712d0479650a705ca46260d4b811079ec6d8bcadac2253e1b1e75e39f651d5f2d0447a20f8512bddcfe9af46c54f11658fd2105c379a690970d5d52c51b15ab3232609c3d550e60980b188b40fbf55b4d1bc15dc6a08ff1a7daac63845c634569de105acc5e58499518eb47874fc4c4101a8a89bfcb02060d223f48e6b59fdb5012d6381e83e8faace35b0f86dbcb7bfd76f3578631344c08d525db841db2aabdb96ceef7f46315bcf94c7966a905d4e6ed1401c4383decd401edc3634743c0313a2144c5f342faf6",
        ),
    )
    reject_foreign(
        pss_restricted_private,
        pss_restricted_public,
        hex.decode(
            "0960567b569be19850264a844273dedb6f75bcb97a9072d224dc748feb53f947062a58cc013bf64d277b036b13b804bafd3eb9b024df8a491e1911604e455fad835273ccab985f21c2ea25cc63deb5bc61d020845303b2dea139cf37019b442908c7da835034318ed9b6df3331681d180e280659cfedf56f94a6337e60f15cfe3ed54305fde0c1eef544c5e094b967c71f26017cea0074ad6067c250b89bd5965f5f05ee93ca3c51bfbc87da6273b96d33d3ba4e5225c9bdcfdabbbc245e3134d8868efba2ccb6c4ab3f677307f3558eb98e609e56a055f0a195dee6a4274503c24638935b9cb453c6c6e09b92b8822517de7dd462c52507cbf96f807e178879",
        ),
    )
    reject_foreign(
        dsa_private,
        dsa_public,
        hex.decode(
            "303c021c2a14d88bb65746a864ece089b58be57e705ae2a96206cedf9fabe680021c4c42d9bffeb67d3e3266037977f4b5993ab9b183f53a8464675a27f8",
        ),
    )
    assert not crypto.rsa_sha256_verify(rsa_public, "RSA binding regression", hex.decode("a3549b5b79415ce9fd6058debc36d8d569ff2e3abd8a8268f14cb897ad659218f1a5f2e3a945eb19ddef2c722659062321eac297231f37c2425f7142f7f1c6a278e51e671947063e0656b6e3ae7544b7ed4f03e848ff62d597caeb69694bfd67f95a929d9924451314a8d6ea25ce02c9e5984d3cf169f64cbe15655ac653a694202385c0273e1864bd740ad7368cba3dae52e6caab85f8e20c3f712ec03bc733c6fbc1bd03ae0d9be99d67cb990db856973bff7ae6fb877ec48bda4350eca17e41b2f4a2a05e2f1bf4c799a0629b9c24b9635a27d0dacd7a10963bd9180efcc5aff8a759646f7cb1a8656343dd5c2fe7d25e483bc6dfcfb7111ef528b2820f61"))
    reject_damaged(
        rsa_public,
        hex.decode(
            "5c42befa3cd508cdb2de15299928a28855616e83ffe406763eaeaea797e8e7d2ad40f3fed799c00bbbce1244b45d1960a2625ecbfcd0b442f3fdd0d7c012b6445d569dc36d6686a96428b2c517ac9de761e0da02cf0c475e23aa47fb7e8ec6a00427d75a7d53f478dec612d29fedbaf2209a1bc55455628d533cc4f38764aff346d13611a72ae1267dd6cb54107fdb417e9cc80cc64b0e8257744f18e16654f464df8f410b9f89b7ec88ad463befbd0f12947ad31864a583de49b6f470232b3f0c27c9c85500a1a9a851d995342d949b699d5f83031408881ee38d4bf4e95d0ec136e9fb7ac6f60f0e926e4ffe11b15931e16f1ab037370f07fb6cc223395fcb",
        ),
    )
    reject_damaged(
        rsa3072_public,
        hex.decode(
            "1c2b82ff9c0b85887d6542ac800083acde4c5dc64348dc743386e07c953b2da3d5ad86e99cdcfa80e7cb16766e52c66ba9c3e9000ca7dff75b56f13dca2df7f486594362576a3a61a33a689b73573fcd6b6a18b40d2530f817d6a857050efe1b95c99808ffa2da028562834cf7f6952fa7ada18f4c6f411e1d93b4b1be78ca676e607bc39562109e78a8af9449730383ca9266ed82b4428e43852652662f0390c515ff9e7cc4519827f31bd37901a84bbcb6fa8ff3e8625f3c6f2c05b70d0b46a70f9b366a9b15ecd6ce55242cc67a8bc064de6beb470ec2573a454ead22c46c7d08a89c58af556cef9f10102265fc6cf629090357d17020aaea001b701a347eda07adaa7f4e76b03b74d003c2a2baefc2991ae17ee4ccec71d48c6bacf594ff9814fef51f47588f7e0ee24f4daae2123b5f980963ac929a0c32e8ef9f35247e11727f7920800d3a586a122ae6d2e6728b02ea698c40cbb9850341f1651bbb67a1e67d2d43ad8b9500602dae02bb61de20819010558f18c06f75a36f9e2ef8d3",
        ),
    )
    auto valid_signature = crypto.rsa_sha256_sign(rsa2048, "RSA binding regression")
    assert not crypto.rsa_sha256_verify(other_public, "RSA binding regression", valid_signature)
    assert not crypto.rsa_sha256_verify(rsa2048, "RSA binding regression", valid_signature)
    reject_private(rsa_public)
    for malformed in ["", "not PEM", "-----BEGIN PRIVATE KEY-----\n-----END PRIVATE KEY-----", "-----BEGIN PRIVATE KEY-----\n@@@\n-----END PRIVATE KEY-----", rsa2048[:80], rsa_public[:60], rsa2048.replace("END PRIVATE KEY", "END PUBLIC KEY"), str(chr(0))]:
        reject_private(malformed)
        assert not crypto.rsa_sha256_verify(malformed, "RSA binding regression", valid_signature)
    # PEM APIs do not accept bare DER inputs.

    auto der_private = hex.decode(
        "308204a20201000282010100ca6edc813e4e04ce119597cd0d94694301148d19d5ff1a272e23d14d6dbf24bae8b766adf3c7ccd2d26b99513a13ee4236b1c5281494a8557d0a5ad4fe30c577275db3f1d42618a6b0058ec0eca9793ec2e77036cc3c9a3f50b5b7f5d0db1840bad38c7a019961ba07c0a322446da6e594999ccbf82c11996fdea4f6964a42bfd7001dbc776b7c9fca48f5d9299c5b59dc927db81b122ccfdf17056e83e6836da8fa45ac5b3d5755c5b2085dc34a6d423d69865cf55eba291ba4e4c9b97bbafe480b2c029605dc282befe4f316a3abe158c2f55396cad86529ef652cedfa339f26dc04a3b371f7a44e8e5568da8689bde1fa95a2091dfa3152b26bb23e6860650203010001028201002ce69f2959b10376d3c6ab1793fa05c9136dc408658382810b466d8fef9654a6d2a48f75691bb9ae4f770758517a07408f9229c9849b1ea56ba946d1749a4bca6ca4679bedb30f87b5865e0e23d04970e6d95ff5ffa2379270ac05e453d1ad14fa7b9eee96f9370884dcb132921c02476d090f92dc76a5e6b33e1eb80548ee79eabf597245816496f4ae093a9d8c41b64be352d2a5151a950dab6175fb9eac1f8b1d70b534207ed9371a8ff05288ba602f510231abba05acd4463a41ba267533f36908fb823d40ef2a5ce163118e9349cbedb5b75653a78048d72c7f39839ff4cbc882218143d4f9f610d5d6759377650412e8cf55432f5247299a5fcbcbbc5702818100ec38538ab66dd220b0114df1027517c029981af123c81aaf0811f3580a73fef119567439ca2d0c6422f73bd782ad6b869ea8e59f51cb1fe56d1d16206d559155690dacf76136c64f70606fe107cd4bcbaa72b12bef699497a7999dbc1e85f0f5c869db840d424f76364fb85d771df149beef712b2d309bc99000b1ba4d3b87b702818100db6244ca43bd5bbc1340a155d893782432a5501e7c169be9a9250cb5c7d91aeea3d27d70f78be2d56c8705b577b0b9440c4557cfa5bf84576ca5b7ff38eeb0f05e7bf1a9c538d5dbea3cb3168837b451d170312febde6013fb50408b1ca9a83082a3c1189d8f49d326cdb407be904835cdca6ef672c8084576b49f6e467000c30281805364fb5170604b1bd12cfac84866d050be1e7dfc7df66a3d3aeb236f0499b8faa3485b55ffa371c97472b644ba54c12e509b2e1dd601912c2d1e64ae640b1e78f9d2a32adebe38bad8e0a2fede03436c0eabd912babe1a8083e1fec6e83bbf3af65b30ed68c4ed4abd97dd6dd9dccf4f8ac1fbc1137e22b1c14e0d70c94d67f7028180558eea93137e0b03bb626e494390a7bf3e3d3894db021a4633602ea7ca4c0273e55216f7f2a30c90fda55ef599bb4355a58f79a292dab12b43451210284cf69697d2b0b9ec1fb14ff5f73c44e2550f2f058642b894bd822ce8cb3090d059de2d2515e61dca7cf266b5f06d70c3c21357606dbd7ec1deff52cfebbb6dcf6ca8ef0281806cd11e27b2189365a41d90ea3e2c2ab3b798afd93eea73f4ade7de23c294fde618d0e903ad0c7c49648857b678ee3bc7640e4f0f38c460f40f94dc32aba2e5d9f1e7b0550ac2acf6b5294e1af80056d51ff069b556fdea84ed5e22582531f7667f2291fa0877ee821b956bb969ebc4587d2785c1ff88eb5df6d2952067bcf4df",
    )
    auto der_public = hex.decode(
        "30820122300d06092a864886f70d01010105000382010f003082010a0282010100ca6edc813e4e04ce119597cd0d94694301148d19d5ff1a272e23d14d6dbf24bae8b766adf3c7ccd2d26b99513a13ee4236b1c5281494a8557d0a5ad4fe30c577275db3f1d42618a6b0058ec0eca9793ec2e77036cc3c9a3f50b5b7f5d0db1840bad38c7a019961ba07c0a322446da6e594999ccbf82c11996fdea4f6964a42bfd7001dbc776b7c9fca48f5d9299c5b59dc927db81b122ccfdf17056e83e6836da8fa45ac5b3d5755c5b2085dc34a6d423d69865cf55eba291ba4e4c9b97bbafe480b2c029605dc282befe4f316a3abe158c2f55396cad86529ef652cedfa339f26dc04a3b371f7a44e8e5568da8689bde1fa95a2091dfa3152b26bb23e6860650203010001",
    )
    auto der_rejected = False
    try:
        crypto.rsa_sha256_sign(der_private, "RSA binding regression")
    except ValueError:
        der_rejected = True
    assert der_rejected
    assert not crypto.rsa_sha256_verify(der_public, "RSA binding regression", valid_signature)
    # Fault paths remain usable across repetitions; success still works afterward.
    for i in range(32):
        reject_private(ec256_private)
        assert not crypto.rsa_sha256_verify(ec256_public, "RSA binding regression", valid_signature)
    assert crypto.rsa_sha256_verify(
        rsa_public,
        "RSA binding regression",
        valid_signature,
    )
    print("RSA key-family and PKCS#1 v1.5 binding: PASS")
