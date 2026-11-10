# Golden fixture (ticket #560, D-0143): the context loaders fail CLEANLY (a
# catchable exception, not a crash) on every bad input, and resources are
# released on the failed setup. Covered: a nonexistent CA file; a nonexistent
# client-cert file; a nonexistent key file; a cert/key PAIR THAT DOES NOT MATCH
# (client A certificate with client B's key); and `load_verify_locations` with
# neither cafile nor capath. Each case must be caught and report "reject".

def write_pem(str data) -> str:
    auto dir = fs.mkdtemp()
    auto path = os.path_join(dir, "pem")
    fs.write_file(path, data)
    return path


def main():
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

    # A nonexistent CA bundle path.
    auto c1 = tls.SSLContext()
    try:
        tls.load_verify_locations(c1, "/zap/no/such/ca.pem", "")
        print("bad-ca-path: NOFAIL")
    except Exception as e:
        print("bad-ca-path: reject")

    # A nonexistent client-certificate path.
    auto c2 = tls.SSLContext()
    try:
        tls.load_cert_chain(c2, "/zap/no/such/cert.pem", "/zap/no/such/key.pem")
        print("bad-cert-path: NOFAIL")
    except Exception as e:
        print("bad-cert-path: reject")

    # A real certificate but a nonexistent key path.
    auto c3 = tls.SSLContext()
    try:
        tls.load_cert_chain(c3, cli_a, "/zap/no/such/key.pem")
        print("bad-key-path: NOFAIL")
    except Exception as e:
        print("bad-key-path: reject")

    # A cert/key pair that does not match (client A cert, client B key).
    auto c4 = tls.SSLContext()
    try:
        tls.load_cert_chain(c4, cli_a, cli_b_key)
        print("mismatched-pair: NOFAIL")
    except Exception as e:
        print("mismatched-pair: reject")

    # load_verify_locations with neither cafile nor capath.
    auto c5 = tls.SSLContext()
    try:
        tls.load_verify_locations(c5, "", "")
        print("no-ca-args: NOFAIL")
    except Exception as e:
        print("no-ca-args: reject")
