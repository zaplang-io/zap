# Golden fixture (ticket #560, D-0143): chain and endpoint verification stay ON
# through a client context. All three servers are signed by the trusted CA A, but
# only the first is a valid localhost identity:
#   * server A (SAN localhost / 127.0.0.1)   -> ACCEPTED;
#   * server with SAN other.example only     -> REJECTED (hostname mismatch);
#   * an EXPIRED server certificate          -> REJECTED (chain verify fails).
# The context trusts CA A, so a rejection here is specifically the endpoint /
# validity check, not a missing trust anchor.


def write_pem(str data) -> str:
    auto dir = fs.mkdtemp()
    auto path = os.path_join(dir, "pem")
    fs.write_file(path, data)
    return path


# One-shot TLS server: writes cert+key to disk, listens on an ephemeral port
# (published over `portch`), accepts one SUCCESSFUL connection, echoes the one
# record it reads and closes. A client whose handshake fails is skipped by
# `tls.accept` (so a rejected probe leaves the server waiting), and a server that
# never gets a good client simply stays parked until the program exits.
def one_shot_server(channel[int64] portch, str cert, str key):
    auto cert_path = write_pem(cert)
    auto key_path = write_pem(key)
    auto l = tls.listen("127.0.0.1", 0, cert_path, key_path)
    portch <- tls.port(l)
    auto c = tls.accept(l)
    auto msg = tls.recv(c, 1024)
    tls.send(c, msg)
    tls.close_conn(c)
    tls.close_listener(l)


# Probe a server with a client context: "ok:<echo>" on success, "reject" if the
# handshake / verification fails (the catchable contract).
def probe(bytes ctx, int64 port) -> str:
    try:
        auto c = tls.connect("127.0.0.1", port, ctx)
        tls.send(c, "ping")
        auto r = tls.recv(c, 1024)
        tls.close_conn(c)
        return "ok:" + r
    except Exception as e:
        return "reject"


def main():
    auto ca_a = write_pem(
        """-----BEGIN CERTIFICATE-----
MIIDIzCCAgugAwIBAgIUFi53Mg8T3tIG4uiZGo9oEhhqlJIwDQYJKoZIhvcNAQEL
BQAwGDEWMBQGA1UEAwwNWmFwIFRlc3QgQ0EgYTAgFw0yNjExMTAxMTI5MDdaGA8y
MTI2MTAxNzExMjkwN1owGDEWMBQGA1UEAwwNWmFwIFRlc3QgQ0EgYTCCASIwDQYJ
KoZIhvcNAQEBBQADggEPADCCAQoCggEBAI/36u1q9SlD1GSD2yViMWGkm4IOnvuQ
yWCSRkc1/BdoUUl3aKnxNs0i8piSxINr/F/+kt4ezJzK4sCt9eJBaT0SCHOW0m/P
7H4wJvvC/6iOj73kuGTH37tuKtk6spzv1ftcO+3B4IICn0af+4Y1LhM0w8XV0Jll
1FrzKZvBb8qUCzEv9PRQGSYHw9kSQ+30QXiGOZ/h7q0ovguAznjZGBtLH0iaF9Uk
fqDm4cweIYf9JlrEm2s88aJPAurZi2X8wFls83BFZFjZo7bs14Fd2A6kyS/LFSIM
yZaeBRFjM5wlkkVxu7iKk3Q8rCZx36W00KG1EGFbKP/jfdbG0bHmY1MCAwEAAaNj
MGEwHQYDVR0OBBYEFG162bSashGy1C92qQQbZanDNhSwMB8GA1UdIwQYMBaAFG16
2bSashGy1C92qQQbZanDNhSwMA8GA1UdEwEB/wQFMAMBAf8wDgYDVR0PAQH/BAQD
AgEGMA0GCSqGSIb3DQEBCwUAA4IBAQAFI7ERqMvJirWlWcGh5wlu2hNENoKPSicO
209FnUNfKlQGaN1/D/87OAxriI0e3TrxUtm2VFN1JK3n2GaUf3Mn4UZRqVgn3b4e
ca1DwcBxMv/HnZgAJBlGkbcsm08reYxf16BRhv6Fjuiyiz+T7ROa9XI8n5ool++D
+IF/RTfQjsUQelVSMCjTht5WEd1/M0WYVzD1b2LVf0FDnkOXqCP+B3au3Ts2PpVd
08eXEPCNnSJw0ZDaAVRTMZH8ZxFrIoUfSIYon/4F34cRwTqMBS0sp1CtZo1kkxKg
VyeCXxjGNuU1dLn4GflYhts/FYlJ6D0u7doSWzJtNzsHqnxOaaWJ
-----END CERTIFICATE-----
""",
    )
    auto ctx = tls.SSLContext()
    tls.load_verify_locations(ctx, ca_a, "")

    auto pg = channel[int64]()
    auto pn = channel[int64]()
    auto pe = channel[int64]()
    spawn one_shot_server(
        pg,
        """-----BEGIN CERTIFICATE-----
MIIDUzCCAjugAwIBAgIUa0q+zPpioN0ciijbbbjGeRb0JI8wDQYJKoZIhvcNAQEL
BQAwGDEWMBQGA1UEAwwNWmFwIFRlc3QgQ0EgYTAgFw0yNjExMTAxMTI5MDdaGA8y
MTI2MTAxNzExMjkwN1owFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG
9w0BAQEFAAOCAQ8AMIIBCgKCAQEAnoXjYR/iCPdAt/yuEemPW6OkbWlDVIqsKkYn
6NsrQ7pMWoW6PAy6NkjTdr/nldwDJxaGEir2hQD33I8AFRq84fdwcXaa7R7B2GXK
oPKyYL7/E8/btSiF0CQ1I4IoeO9bdrdd/Gk1s5CIlr/tAVI0dRw0qYlX9cflPF6F
pVnL94AvlwzuhXtloglsPClLsTtZR/9BPM2dvb8CcubWi5TLnuoSezhcf2iyWz5I
ohcF8dL8f4pstwmhjba/pGKaKLpLFiLG+otjJ0tKJEM/LoJYjTHexiqAJsSKD1fi
RJTDZp3Ft98A3PiYKO+Zzs//em5v2w/YWT03+mbyp4joT8HNRQIDAQABo4GWMIGT
MBoGA1UdEQQTMBGCCWxvY2FsaG9zdIcEfwAAATAJBgNVHRMEAjAAMAsGA1UdDwQE
AwIFoDAdBgNVHSUEFjAUBggrBgEFBQcDAQYIKwYBBQUHAwIwHQYDVR0OBBYEFMGL
3lJkoIr4X2oX5d3RpbVeSVhAMB8GA1UdIwQYMBaAFG162bSashGy1C92qQQbZanD
NhSwMA0GCSqGSIb3DQEBCwUAA4IBAQA4px70gZrWX+e+R69lfn5x+qGkYOesJW+d
OfsGPOa+x6Ol6wo7ozT95JYKF3OP4nl/zmyTarMQwGZueRilCfIUYj/7R+o5FxOf
aTWNTVuyE6aZFmfkxHSxMQ+TicSZnPOnP6jLA304HoXuQ0Ha84PhGUy2LYugNis/
cIAVqxLGSB5Lu3Hpj4bh8hG7jA98dmzbHmvAxJ3VG4L1peNI5r9eK4L19dgj7rD3
U26fLCJmH34gVMQve/ydJ6DBj0kkThzKVSmK/dlK0ICNR/h8iNUSOLR+CP+FHoRq
23Wa8CopaWZqqCuR47JIQDxMYfDyoCSdoDuDJuT7pukcT/n9fqXK
-----END CERTIFICATE-----
""",
        """-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCeheNhH+II90C3
/K4R6Y9bo6RtaUNUiqwqRifo2ytDukxahbo8DLo2SNN2v+eV3AMnFoYSKvaFAPfc
jwAVGrzh93BxdprtHsHYZcqg8rJgvv8Tz9u1KIXQJDUjgih471t2t138aTWzkIiW
v+0BUjR1HDSpiVf1x+U8XoWlWcv3gC+XDO6Fe2WiCWw8KUuxO1lH/0E8zZ29vwJy
5taLlMue6hJ7OFx/aLJbPkiiFwXx0vx/imy3CaGNtr+kYpoouksWIsb6i2MnS0ok
Qz8ugliNMd7GKoAmxIoPV+JElMNmncW33wDc+Jgo75nOz/96bm/bD9hZPTf6ZvKn
iOhPwc1FAgMBAAECggEAHKbT7GHUG60+1hJWTvgBOH25EipwqDbhO8nO3FRCu1zQ
gJsDiGd/f6DFkcTJQYmQy0+Tn3SC9pKwef2htO7OfWQY/3UBtwJ0ZF2vlEwRaBGV
bnoG+rUOjIqCzT5w/Ly8chZd5ymH8oLY4TBi0LaQMyuMLCleC9cbrH/kFXkHt927
RMgKBgxVIe6uLw4rX76soajgww+iokHR3/6U4eQqFocNqeA/8DW6vUS73qKHKg56
10E3Z4mgz7goN4VEx+gh52HOG+iLtbNQU7E7D6SDn33mqxuxW352yzTxeRIro5bE
0hwb4SRcezgjjAGJKI/6SMWZMA2iZJAQQKk7fk1DMQKBgQDOP0n1mGBtxSAwjN3a
j2ORDB8sr42ZBrotNJElU7Ay7NMpVwspQEuXfz1I9ct7HH57d7TpXDfrDaZiUoVt
tXThwbSSEOWxpdufU8ynQdEHK5YXXocVPVPSV85Q9xuCxjMXpyACv7SZ7ovwX7E1
zyqym2eoVB4hfq4GuPgXLR3M1QKBgQDEw2jw2ipeSDVEZm1vDGncUKezmfn0EK7N
rChBb6Efxw42kARhCbTrdtgL3BIaE3pRBnMaykx9HkEA+R1LofXd9XfUY4I/QO8/
d5lFGAHHeCY7pR3mMZcuTGrm0NV7kV8PXY0mbRhjXCDlgcqRR/RQG5205JgAwOdH
LE5jlUl2sQKBgD7KJcoPScwhZ1tgh2oynS1bc2I3T/R2h9ebWVTz1YCreN3tEgjM
+jZ1FkNQEVK5qggMuZcR7DQzQ6RLWzO7OopAXqjZXBRpWiN3b6a78tcr/dW/gYCF
2y7+bGFZ2ZZGoOdiiexMUL9MUG4ts9Wd7HHTvgsRENtBpdmGFoF3ysrBAoGADccV
GRBduR5bRH/bl9QJsZigNY5Jl1J3uwUcCID1ilEY6K8q71SLq5U1yZbOWa8VeIj4
FF8n/fbZXGUVLQ+ZNMKUcnXJT/6PAiWwwMrOjRYXr0N/lXfRezd0oNbsq+tg+aV2
VcN72rE/yHJPKd8c92MHpvv+vMbFKxGqJ4TzkPECgYEAsJLFsbIUGb0EMBf24COG
Mc4LvtqWilvlggx3Nc3QF3esv6xf9gpJDWz2D2DfPo7AkAVxHSpsCoERyB91diAx
DZq6tXnu87bDiWQBxd1JjD7yEQu1ddnSS8LR79PLQiQbbBESebMT90DKT9dupLRV
99b+ciJ2L40lIm5OeyyG5LI=
-----END PRIVATE KEY-----
""",
    )
    spawn one_shot_server(
        pn,
        """-----BEGIN CERTIFICATE-----
MIIDVTCCAj2gAwIBAgIUa0q+zPpioN0ciijbbbjGeRb0JJEwDQYJKoZIhvcNAQEL
BQAwGDEWMBQGA1UEAwwNWmFwIFRlc3QgQ0EgYTAgFw0yNjExMTAxMTI5MDdaGA8y
MTI2MTAxNzExMjkwN1owGDEWMBQGA1UEAwwNb3RoZXIuZXhhbXBsZTCCASIwDQYJ
KoZIhvcNAQEBBQADggEPADCCAQoCggEBAKbUtDS9PRP1mc+IQ3Uwqyyw3sNmNtA+
P+70AGn1UgB5heyl6Deh0PnW76KmJARryUKK4A9suyzJ9ecnWGuBC0EIIBdw0ecz
2hrkyMhPYVwtt3FyjyWTmawSWIoKJ/4E8stgnr/pUlwzL2WbGw+EMyd1ZiqM17yV
u+RUk6QAhPwA7IM0iuhzqy2JB/8K+Yd/c//zMvQoecXTCOXspNgLCbkKPC/lDbhE
aiR0ZUH1dsphiHSm4Q6cEpjG/nAIbxnnm3ZCCtP/0T1nCLM+nxHktjSrtunoxxXT
5wRGwQHdBqSFKQ067OXp0Bwb3GHkjKmfLZT7rQyplQZ/NoINz7stPnkCAwEAAaOB
lDCBkTAYBgNVHREEETAPgg1vdGhlci5leGFtcGxlMAkGA1UdEwQCMAAwCwYDVR0P
BAQDAgWgMB0GA1UdJQQWMBQGCCsGAQUFBwMBBggrBgEFBQcDAjAdBgNVHQ4EFgQU
G2N6yHOT7PY/Q1X+p4JDHbVoO0EwHwYDVR0jBBgwFoAUbXrZtJqyEbLUL3apBBtl
qcM2FLAwDQYJKoZIhvcNAQELBQADggEBAGnWcR7KweKH08Fb7yx3Bd8QX2gE9nRI
6HnoEgYMilrdW9Hfl9i12iXaHqJGbP0xY5/X895JFmFXAly9LbeIQWTCj9C5HwU2
ZfWHEtZ7lgF41f5NYI+m3oKImns3DNKNNVZCyZptwMIq4YcMgCjRF9vpSBZLITGF
VXoEd8OYAeVz6suCm7ah3vHYdJFD9AGCFhvdDnTOxnuveCBenAKzSnqIXFDvxBoB
n9Wi8U4IpkBOKvds0fvM2opgXd/w5kTwX3jU9gujejGq3JWnuM4iiW4pTTSfDyca
7qJ8LE1Ke0En1aipZmure3TwhXjJ8NHoSD4aLqJI4iGjRT1OvRh6eiU=
-----END CERTIFICATE-----
""",
        """-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCm1LQ0vT0T9ZnP
iEN1MKsssN7DZjbQPj/u9ABp9VIAeYXspeg3odD51u+ipiQEa8lCiuAPbLssyfXn
J1hrgQtBCCAXcNHnM9oa5MjIT2FcLbdxco8lk5msEliKCif+BPLLYJ6/6VJcMy9l
mxsPhDMndWYqjNe8lbvkVJOkAIT8AOyDNIroc6stiQf/CvmHf3P/8zL0KHnF0wjl
7KTYCwm5Cjwv5Q24RGokdGVB9XbKYYh0puEOnBKYxv5wCG8Z55t2QgrT/9E9Zwiz
Pp8R5LY0q7bp6McV0+cERsEB3QakhSkNOuzl6dAcG9xh5Iypny2U+60MqZUGfzaC
Dc+7LT55AgMBAAECggEAGUoMCSSYkiJ8TuPSify3Lf6fvhIMBBQzn9bYCzfXVuAV
B5L8uRYr9ft5dvflZNAsAYzIuz3Zv8fboU8rDZzRpmmDZ9o/z2LiUIpWz1qNISqC
8M0TWvhdZqKfTkmM12pLnb3HQsMtK3HM4QOangcFtBkNRCz2dQ1jnpCdktJLVmHU
qPp1XnfNVzON+AxeQNETvcYbJT+Q8YmiQAgiFBPY7BGZcs1OUSSc4LVBU3jsvyIB
OuAEeFSrdb5H3vagWT7yZv7dIXsSXGp5Y76OX86x7MLH0hRc/HmzvqfOkKwSQyaf
cQoYFcIkMdOXqQw9C1WBTCxYBFmE5uUGwwHktbWxywKBgQDdvtAhnTRXRlDR3LKC
a3/gZLWCWugx7OGrevrAhzGpHrw4dXO8yCNperFibOnxy+JzvNIHorurLEpRzIsb
68Q8IpDyc06zMSvVV9aTre7ZReT7/gZBTzCnBwqnNQ6JxsbuqHtM2xNihx1jKWly
W00HcpRCHCMxPZL4D+HHJGSZcwKBgQDAmjs/5fVw0ND6MX0qRmRybtUYrrlTF8Pg
JsOBVR2jjV5GM4/wdgMwBJSWdKnMcxFjAbrdj6pzFh3hhdRqVjbCcrKZdSJZeI35
lzeYm1/YE5GeXD6yIM+Kc6dldS0jYNuM89rE9b1ySR+S6JMQuyDZkhunEOcKwYQi
TjvCghS9YwKBgFT2otruWDrEh30R2hgcxn+dNur+mA9hG0rQMToJ0x5g12+umS0R
Zngo+cSjxLPQEjGUfMlOMigxLTzQadDF8oe0GuO7CMGiKpBxOpeVQnpfzJA9Ufo+
NH8bNJrCupJR5rFVPwFAw9mxubHgETYvA4HCBkqaa2WXmTORKkpaiG0/AoGBAKCT
NZilcUbYYZJmFjNbsr0RGErWyR20r5K+3rqCKw7DevjOXlAdI7Ux7Guv4izZNoKz
24aDE5mVs93lRN5O+KKPG8YDOVscvkl7wFdUGsk8L2i3AXJjFoUYvVRCKBrZ0tGm
MRuKwaJ5woAptQr66H1caf+UgPufsTkkvoABEad/AoGAPX9l4cP9WnA5D4mxAw7A
zjoosdm/wvz1aCAT74OPyA6pumYFXfkKZ+QdBlJkk5Y7oe1Osbvs4NuvbuOa8Tpi
T5aDl6N3kv2chqdfI1vp3EHcE/824I7xPSi7gf6s1v8RYBx0EFf+0YUxcJ6yRpfN
3iC1h6jCb9UA5uOF5TX2atA=
-----END PRIVATE KEY-----
""",
    )
    spawn one_shot_server(
        pe,
        """-----BEGIN CERTIFICATE-----
MIIDUTCCAjmgAwIBAgIUa0q+zPpioN0ciijbbbjGeRb0JJIwDQYJKoZIhvcNAQEL
BQAwGDEWMBQGA1UEAwwNWmFwIFRlc3QgQ0EgYTAeFw0yMDAxMDEwMDAwMDBaFw0y
MDAxMDIwMDAwMDBaMBQxEjAQBgNVBAMMCWxvY2FsaG9zdDCCASIwDQYJKoZIhvcN
AQEBBQADggEPADCCAQoCggEBAMgPJRjaMe3WM6E2KGicC/CKjN5kfeOk3hpLYY8N
QXDx6Idl25giSS+RF8RZ1sNtRPPSm2mUF0PcYh63797zsckXZY5lFDQwKWa9zOBX
99KzI7e0GtcTebJVPDxwMzcUGbZOfUOpa5YRYnl1iUNyoD3J2+wm99wvsdMqCQxN
n5piJV7KaCuQV1OCAeTMYLuFWEXls6DOWGA8kPii6RBSu98k9GG7hib2ERlyLv3I
c0jYor72QyUvot/dvBiZqm5Oik58PbUyio40q91LSSYAgV/zXeWPpuEtpePBtmWk
61GXNn/0NSgEMmaA/Scnf9KgP8LnaTg3UoGfZKYwcRoSP3kCAwEAAaOBljCBkzAa
BgNVHREEEzARgglsb2NhbGhvc3SHBH8AAAEwCQYDVR0TBAIwADALBgNVHQ8EBAMC
BaAwHQYDVR0lBBYwFAYIKwYBBQUHAwEGCCsGAQUFBwMCMB0GA1UdDgQWBBTd+fnT
FAnXN5oebE4ibAjc/dFnoTAfBgNVHSMEGDAWgBRtetm0mrIRstQvdqkEG2WpwzYU
sDANBgkqhkiG9w0BAQsFAAOCAQEARHcvz0XDUBbX2GFKlykGEHIpf4+KnCYK1I7b
/CALr/oous26ga2r8zsZSr/wkpxr1GduxlKIf+90JxlGuy9XwlgE4wbjm/n9vOcw
b6XnIfd01cLWtiWoESrYUHzTdw2/T/6cMNAgaYjdf9XkdhmzCIolx7k9vvXewB2/
46c2ZTBhU7ijqenq336aGd2ytduf92icHkxtZDRBUKmaFFdada9EzFwW1p0nRRFt
OXRBB0pbGcW/fN0On7kHuLi0VI4G74DjMJxkTpWVcaS62ZVZ74jy/B8fcJysAnoS
EIV81cxNSL2hfENIZB6oqyjA0eEBikXJF6ScYoo95T66TNlAFQ==
-----END CERTIFICATE-----
""",
        """-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDIDyUY2jHt1jOh
NihonAvwiozeZH3jpN4aS2GPDUFw8eiHZduYIkkvkRfEWdbDbUTz0ptplBdD3GIe
t+/e87HJF2WOZRQ0MClmvczgV/fSsyO3tBrXE3myVTw8cDM3FBm2Tn1DqWuWEWJ5
dYlDcqA9ydvsJvfcL7HTKgkMTZ+aYiVeymgrkFdTggHkzGC7hVhF5bOgzlhgPJD4
oukQUrvfJPRhu4Ym9hEZci79yHNI2KK+9kMlL6Lf3bwYmapuTopOfD21MoqONKvd
S0kmAIFf813lj6bhLaXjwbZlpOtRlzZ/9DUoBDJmgP0nJ3/SoD/C52k4N1KBn2Sm
MHEaEj95AgMBAAECggEAD4l7klNJyuSWlGfaGgu6cqcz+waXb1FLzYN6s2suYKjs
6+0JcaaCBA9y5tUqVHmdQAlkd/3gE61fMm8LEkffUBSA1t77Q3VVFc1aRrcWQFcY
yEjUx8zTp/muKKx3Eck+GPY+DL3w5NH7UZ8C9snFb58IdS3ulHtgxltOohW6NjlT
Sm8GgGsA3QKNEjkn5ZlAXMc4nney7+Wcly6t84UxkktUXDhC99JQEMTyQz3RJ358
UTaJ4eMrGUR6lr3KhIDU+FkzzOPlm6J9S+YvN1V6Zk9cYAv2xik4GNevYx7E1rpo
1NA1Dm6jVMn21lIol6X8ZSQckuj4jr8PLST3/qL+IQKBgQD4pfSFr6LuAnaPuv/7
PFHDXhNs6vA0xk2G3/QsvMbZcuifJUJ4Mvd9dkWTVxsiQ196OAkt5AzWjbKimhqg
UAI/eXu2wCcLFQa1ztJ6j5i+rFhv1c0t468nrgWsyoigZ1bWYUEfHGMcCkrTEn/H
BEwe/KIX9xiS+dwKAx/WIuH5cQKBgQDN+Wnpmp74rOd6uaML0RpcDkG9ajE1qm3D
hvu1cCu5Z/PsI0fKo4R1+6volW8EIGNGpSMFnaE+85Fsf58UD/iMgtU9Ld7sE2+g
qFT25lfHim+6WSBMZT60Blckdjjmte4PqIyw1ywOyBWKUpI5Zn+u5TQEVhq2pFat
p589uM7iiQKBgQD14ea0bMMJ6pWj/KA2qaeRMu6yvoFbLt5rifvEYcmiLuequQXK
J1xmTRbwZSsH7ufzzIYHQ4UOhoEoGoxKC9StQEz+o3X4l4pTL3skh+vcYkHyFdMY
lhcXfhdOj9ocBRTyWqozQbcge20S3TBOdglwexMv78H1VfAqVnydHVGpgQKBgHUQ
zKF4v+GWY/nciE6vCcxIThTZxh46PyhiD62Pk0pNStVrN+J0DVUBNTN7ux0dS09b
jAwlCwA8buq7ni5I5Xr24TtkVs7WPRuSkLfDMyC2Dcpb3X+OC5VGBUEz8CMwQ52i
rlPT97ZvAc8cHv+AwhElM+tDBOvJe3o3SspJv0wBAoGBALZaccGkzb5BAbBzea5j
7PnAR1v6LK6PGXZgSijs6BunT00w+13cFIKY6pw0U0GM1FEF5zsIeKJTMa6nbZOI
oBb+Szvb3ZgoNcDkykdBl/Hw5kdRbI3uEH/8TA2MBr6tPGbI9U06bnav8gD89dxX
WFzi8TqnCLGfVj94AdmROHqP
-----END PRIVATE KEY-----
""",
    )

    print("good:    " + probe(ctx, <-pg))
    print("badname: " + probe(ctx, <-pn))
    print("expired: " + probe(ctx, <-pe))
