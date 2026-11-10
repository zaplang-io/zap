# Golden fixture (ticket #560, D-0143): MUTUAL TLS via the client context's
# `load_cert_chain`. An internal client-auth-requiring echo server
# (`tls._test_mtls_server`, a test scaffold — NOT public API) requires a client
# certificate signed by CA A. The client trusts CA A for the server's identity
# (`load_verify_locations`) and presents a client credential:
#   * a valid cert+key signed by CA A  -> ACCEPTED (echoed);
#   * NO client certificate            -> REJECTED;
#   * a cert+key signed by CA B        -> REJECTED (wrong issuer).
# Reject probes run first; the scaffold skips their failed handshakes and serves
# the one valid connection.


def write_pem(str data) -> str:
    auto dir = fs.mkdtemp()
    auto path = os.path_join(dir, "pem")
    fs.write_file(path, data)
    return path


# Write a cert+key to disk and open a TLS listener on an ephemeral port; the
# listener handle is returned to `main`, which owns its lifetime (so it can shut
# the server down with `tls.close_listener` once the probes are done — the #488
# graceful-shutdown contract). Created in `main`, not the serve coroutine, so no
# coroutine is ever left parked in `tls.accept` after the program's work is done.
def listen_server(str cert, str key) -> bytes:
    auto cert_path = write_pem(cert)
    auto key_path = write_pem(key)
    return tls.listen("127.0.0.1", 0, cert_path, key_path)


# Echo-serve a listener until it is closed. `tls.accept` skips peers whose
# handshake fails (a rejected probe), so the loop keeps serving; closing the
# listener from `main` wakes the parked accept with a raise, which ends the loop
# and lets the coroutine (and the program) exit cleanly.
def serve(bytes l):
    try:
        while True:
            auto c = tls.accept(l)
            auto msg = tls.recv(c, 1024)
            tls.send(c, msg)
            tls.close_conn(c)
    except Exception as e:
        pass


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
    auto server_cert = write_pem(
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
    )
    auto server_key = write_pem(
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

    auto port = tls._test_mtls_server(server_cert, server_key, ca_a)

    # absent: trusts the server (CA A) but presents NO client cert.
    auto ctx_absent = tls.SSLContext()
    tls.load_verify_locations(ctx_absent, ca_a, "")

    # wrong: presents a client cert signed by CA B (server's client-CA is CA A).
    auto ctx_wrong = tls.SSLContext()
    tls.load_verify_locations(ctx_wrong, ca_a, "")
    auto cli_b = write_pem(
        """-----BEGIN CERTIFICATE-----
MIIDUzCCAjugAwIBAgIUe0msv6u7i8RYK3r0m558+wQOv0swDQYJKoZIhvcNAQEL
BQAwGDEWMBQGA1UEAwwNWmFwIFRlc3QgQ0EgYjAgFw0yNjExMTAxMTI5MDdaGA8y
MTI2MTAxNzExMjkwN1owFzEVMBMGA1UEAwwMemFwLWNsaWVudC1iMIIBIjANBgkq
hkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAu+GjetipglhORWDrlTuPhoLWS46BjsOK
Ryoio3JfBV8+Pp/uvHKH41WpabmHdvPHjHatPwz9VKYvKDA42ELexoREsQcpZEA+
zeIwCC5sAj1M7rHXEgnSLU9kxD26JdT0B7g6PtFVMy0n5JvkWp4LxdlFDf3tPo3u
JGFtYdQX1g3tHWwTkTzMfKc3ZKncysjU28nojInp6N1waX+4ukrOoazdkyyTx178
cLF+U5s5RMzCN0fOt6pTI5uM6tjNeIG8Rh9S1BNtlgKt4sPuklzjSq5FawbE0ckS
28SkFvi3KPtxEEzfdc15ESUR6d6DQ+revYybdL25oZFHsKbFbE9mdQIDAQABo4GT
MIGQMBcGA1UdEQQQMA6CDHphcC1jbGllbnQtYjAJBgNVHRMEAjAAMAsGA1UdDwQE
AwIFoDAdBgNVHSUEFjAUBggrBgEFBQcDAQYIKwYBBQUHAwIwHQYDVR0OBBYEFJ8l
8yFyMmcIrcyjbMvepuuZIN9hMB8GA1UdIwQYMBaAFKjS1UpsW5T2zhWg/ogMDn+R
xJNQMA0GCSqGSIb3DQEBCwUAA4IBAQBQki0GHtgkBGOUq5lJiASCzqImJByoMn+q
PDdTFvhcy3MNbjtCnLk/nYJmwrlKSSYrd7mH7CaS1u9B5rKuust9+eF1USeNGDAz
YyntM1A3BlueII6ZKqPyx62l2U7vOsQyqR4h2b+PsGbKRCxEcMQVsRTMEOnlWe6H
O+q8rQd+3tP/Ti0G2mauVzoKbQb/v1rdO6RdXxRdp5/93I12ua6l3nrGA7hrtMx7
dR/td4mB3B3ETcvvIP1Lj382Ipl7ItoijL1Z3enOrI0G0nRxeFkAABHabl9dev5+
tIO8jWcFUiOz8LihNqXfyPOfaf+y3rbTwJIjQnbqR8pOHxA5awUz
-----END CERTIFICATE-----
""",
    )
    auto cli_b_key = write_pem(
        """-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC74aN62KmCWE5F
YOuVO4+GgtZLjoGOw4pHKiKjcl8FXz4+n+68cofjValpuYd288eMdq0/DP1Upi8o
MDjYQt7GhESxBylkQD7N4jAILmwCPUzusdcSCdItT2TEPbol1PQHuDo+0VUzLSfk
m+RangvF2UUN/e0+je4kYW1h1BfWDe0dbBORPMx8pzdkqdzKyNTbyeiMieno3XBp
f7i6Ss6hrN2TLJPHXvxwsX5TmzlEzMI3R863qlMjm4zq2M14gbxGH1LUE22WAq3i
w+6SXONKrkVrBsTRyRLbxKQW+Lco+3EQTN91zXkRJRHp3oND6t69jJt0vbmhkUew
psVsT2Z1AgMBAAECggEACKQo3vhSmYxiszWxOCioYgUoAosgIdcUB9BkDfTvcmGO
TRaRhijVX6sbsuFl3d55/deigb0g9LD9B8HqB2ZR9Tr1pte/u1zqFpGR6HpsHNFB
ddQHooyBlnRpm84D/wPzolxCj4uUudkYld7mqpVM0NF8p9dXP1KLUrCJdZ+p5f4j
LUW9b9/AyGm8NtpPuujQXYF7irkoDnFh7P1DUs29755s2mUG8zcyI6Zraef8g6Tk
bYFGIzkPtNlEYGyXwuDmlEXvNh9phgWpSmJTGK2G2voQ1pdiKK4JGL4UxHhGqsmq
IbhzXdSbiUozx01ea4aUy7gFMxFogJBIWKNGDetVGQKBgQD1oHmVdeZ73KSVZqTJ
hvI21qieIGW06MJZx92kq8n4VogfGfApPRY9JjaDzBgnFyucAKpjr6lmF9RkwtNM
+dSREnC7zXRw7YN0kcg43P0WdrD4L1nSJZUrRiXoTX2sIgG+GSMx9B8FLkKefLfV
NkcddxUgi1fcoX79INfEmZqmyQKBgQDD0N10ZNPvJ2MUxH+0/slduOA6+Avvj8/6
ovnZHBC1kJtUbsHgIFJbTI5hNANdSaSgE0RV8psJjDG5ikRKtyQd39VyWj7KmPtH
qmeX+5nN7o7YRXHmgEcIaFDqaMNT+gRCKsWfeCEI0dCLzQZCoDqFT3wjEMD3RGp0
x1ez7INcTQKBgCxcOxkfHe0owCCrVP+Ae6YjxngoabXug4sraYAf9m3/Ur4rNRB9
k9KfYCEVbxdAPAQEqsEuLG3fx+Aav4abaCR7LBj+1hS/EIqw+XQrlRnFfdiU1Kf7
WaFjHd2CgTvhOGdYqt6OtDt/S1DMPXcoQlez9gDWqZuTHxggIlfDVgbhAoGASuHG
rkoYme4v/7MdOzPsAYE/jo1o0xXl7O0CKrqG0UgZfnJUhccWd3+ROy7O0o/k+qoj
idKa403PfiqX7xKWJy5/y5kjRgq2nTNXJlG7AXtm+OGlqwYHI/nwZ5Pkjmqf0kXV
CSTBTUQfQjcHW9c2SeCZkjdH88cgBsRsj4u0uEkCgYEAw78Ez4WQv6TOV5qpJfAz
axKSMRmczEBsHmDBqXoTdPuB2jlZeHbsm6Pqr2mwKPEKG1Of/5t0+0frxygJKF20
d7TV4aZr16r0gXRdZm6ATvDVhkhecXAURclDqAsK9Ry4IvX4FluQfLfeG3uVIlVN
xtTEyGNi4snecjEqlb9POps=
-----END PRIVATE KEY-----
""",
    )
    tls.load_cert_chain(ctx_wrong, cli_b, cli_b_key)

    # valid: presents a client cert+key signed by CA A.
    auto ctx_valid = tls.SSLContext()
    tls.load_verify_locations(ctx_valid, ca_a, "")
    auto cli_a = write_pem(
        """-----BEGIN CERTIFICATE-----
MIIDUzCCAjugAwIBAgIUa0q+zPpioN0ciijbbbjGeRb0JJAwDQYJKoZIhvcNAQEL
BQAwGDEWMBQGA1UEAwwNWmFwIFRlc3QgQ0EgYTAgFw0yNjExMTAxMTI5MDdaGA8y
MTI2MTAxNzExMjkwN1owFzEVMBMGA1UEAwwMemFwLWNsaWVudC1hMIIBIjANBgkq
hkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAkEh6R98GiqOi/2SUPm/GUiGCNF/Jo+Wn
9U5dQier+rGRyMEvthIJnU/txAMGbvOlowNdwWrN8D0tQVorpLjrahIfaY/JM6Kf
+uw+djNDpTKSKRWaO/SIxzfrLEMYu5fSSB3L3sy/YFr85IJN+6mgD1A0+mh+aPZ/
6QtjKfygCt2jiGhukhghuTRbPouMvfmG8hBLg2RqbLZehgv+9LhBsZ4nji7GgH3n
b892qcJHafRbtOmUwts8u+Z2e7fsTNYtSXGUANNqZs7F8UcNGr/CGaU18ju14ub4
MGOOVKTFYR0VtXWk1n/ax+q8x8sLpZi3Hem9ekvQBlfgrwdCYmm+KQIDAQABo4GT
MIGQMBcGA1UdEQQQMA6CDHphcC1jbGllbnQtYTAJBgNVHRMEAjAAMAsGA1UdDwQE
AwIFoDAdBgNVHSUEFjAUBggrBgEFBQcDAQYIKwYBBQUHAwIwHQYDVR0OBBYEFP87
0a+S8hcETDMvsjWHXx0cQ6XaMB8GA1UdIwQYMBaAFG162bSashGy1C92qQQbZanD
NhSwMA0GCSqGSIb3DQEBCwUAA4IBAQAQkRV1x1MnNKttZEMDro95gMzBdm056i6Z
l2BDMvcBOSNKiKzDltNJjN3Qkxx90qW0bpOBvBvROvBQmJZ+7EPtcyDtEecHXLzK
lYbukgq4Lp/HvoFvjQazMZ8V7S4QnOI0DTQR82zuTDB2C4fMplh6mRe/NmYEJXix
eSXxgvH4bR2DvhcEbfj9Y0CLNrsWqYUQ12nXJaSV3wa9r0TIL0ZHyCKucohKpqYv
brOjiXNv44OXKi8GTxizzx0JIDvrDAYwU2gfAORSzi8zJYnL4TXU42+El8gqRduv
dn4AbJahDZ9QTGF8ltPKCGnJI6gjB1UA09acxJBQcuLBvGf0KSeq
-----END CERTIFICATE-----
""",
    )
    auto cli_a_key = write_pem(
        """-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCQSHpH3waKo6L/
ZJQ+b8ZSIYI0X8mj5af1Tl1CJ6v6sZHIwS+2EgmdT+3EAwZu86WjA13Bas3wPS1B
WiukuOtqEh9pj8kzop/67D52M0OlMpIpFZo79IjHN+ssQxi7l9JIHcvezL9gWvzk
gk37qaAPUDT6aH5o9n/pC2Mp/KAK3aOIaG6SGCG5NFs+i4y9+YbyEEuDZGpstl6G
C/70uEGxnieOLsaAfedvz3apwkdp9Fu06ZTC2zy75nZ7t+xM1i1JcZQA02pmzsXx
Rw0av8IZpTXyO7Xi5vgwY45UpMVhHRW1daTWf9rH6rzHywulmLcd6b16S9AGV+Cv
B0Jiab4pAgMBAAECggEAC3ONLP4hBUjmi3GeqHPzoBojvOdrWBRGp9wP38vJRQzp
3q6ijEdTNwacfuWkkZLJ7y25t7uz26DnYhv3gcvhd1o33wd39x6tptswO+5mrIpe
el9KDf8jCfjAtBMhN+ZdXLbssuK6P7oI/1PuP46fcwexJLy9Z7REw6jRNohj5WjC
XZLUXbU/3+jMRkr1UWQsxg41+bxxJzbvlhsrf1fx3Gvyl1NMVau+8F8QQpdkFt9H
JsNqb7ki1ZPFfjJaoQFsFeWJ7xegcCoffT5QEAyriybJc+P20RbbsdWPHRK+TI9/
XOQzeZzu/Kh3MJ/g/sjMoclLCYBWuHwTgdjgRLMo+QKBgQDMCGksce8X0CUCdZpY
e/s2zkOcJzz2sSTbiNPF8LP58lWnovhRdtHuGVfl6TpLevZlfHfLFcKD79BrcUc6
7qKP4jnytO6eekLRXMO/YqYJ7jWTJQxHJaaUBysrpGfHE92LT7eT6MFwX56YB5PS
ILDP54Zuml2jNGO54nJ+RSyk5QKBgQC1CDHB6iOx1FsbTxhZuBB3QXQLX3r0FPD6
HaQGyh3Drlyz1C9QGi4SlIEhr/ARejt07TBkzMnZd22rdrlk8QloO81a71bH2WxN
M33/bB+MB/vfmY+qVoDZ2lzh1TtG7jJH0Bin4pN+bx3DsPrUfn5DkCmijZKSdgbz
42xg0OhD9QKBgCAxr47Ax45FXKGftuQBZClIsKhiFJ+EYiF13JyeF3f4AgX6lB4e
lo9wVktD2tvwIhLvaeOUomW5IHTjoe6tmt8DPkuKuyAsgICSl5yKT1K/0JpQPTi4
uykeXHR9c5aZDtsdkAT5tBQsMlUg1PVSDTQw/yhSmTXYXWgGUDSWa631AoGAU6tv
4+IlkUFGwn5yELWD1rqHueHtX/Z10gP1FbZAv3pNGkhNfcY72RaOAFXirRFDfa1K
mqwT9tC9/WuoEEFfSa+qIAdSopeY+K195JnCkuP1w8ijn+TPNOE54J6/AO+CibJi
UuXjqaINra6Un7W/AdcVhy3LuqmWWQ6Zvr2PPCkCgYA5974O7842PttVIbeKotw5
RkZ1YP1GMh3YnYFPNYExWqB03y0J/6LljnxT3raUnCGAM+3AOKHz4FNrYq5xzpy2
UcUsJPxADra3lpsIVzVRCy6cDuI2Vmc15/mdLLP4qRonGmKvsrfWN/9mjswxeKPx
BDwzZwrab5v7GNZVnWBgqA==
-----END PRIVATE KEY-----
""",
    )
    tls.load_cert_chain(ctx_valid, cli_a, cli_a_key)

    print("absent: " + probe(ctx_absent, port))
    print("wrong:  " + probe(ctx_wrong, port))
    print("valid:  " + probe(ctx_valid, port))
