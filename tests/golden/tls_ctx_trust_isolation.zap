# Golden fixture (ticket #560, D-0143): per-connection TLS trust isolation —
# the HEADLINE acceptance criterion. Two servers present leaf certificates signed
# by two DIFFERENT private CAs, and two client contexts each trust ONLY one of
# those CAs (bare `tls.SSLContext()` + `load_verify_locations`). Both servers are
# live concurrently. Each context accepts exactly its own CA's server and REJECTS
# the other's — proving the two contexts' trust stores never bleed into one
# another (each owns its own SSL_CTX / X509_STORE; no process-global
# SSL_CERT_FILE/SSL_CERT_DIR mutation).
#
# Both servers run concurrently throughout; `tls.accept` skips a probe whose
# handshake the client aborts (a cross-CA reject), so each server still echoes the
# probe from its own CA. `main` shuts both listeners at the end.


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
    auto ca_b = write_pem(
        """-----BEGIN CERTIFICATE-----
MIIDIzCCAgugAwIBAgIUUICIMsj/L214YpxtTnjgy1G9wywwDQYJKoZIhvcNAQEL
BQAwGDEWMBQGA1UEAwwNWmFwIFRlc3QgQ0EgYjAgFw0yNjExMTAxMTI5MDdaGA8y
MTI2MTAxNzExMjkwN1owGDEWMBQGA1UEAwwNWmFwIFRlc3QgQ0EgYjCCASIwDQYJ
KoZIhvcNAQEBBQADggEPADCCAQoCggEBAJFyQ46lriiieZOfLuKWbimvyTM3OKFT
Hce0jgssxNpiMBCRUO1gKswoNX9JWLRjnsVLarD3e1y9Cs9Lkr7i8e+JrK6adlBX
eiYuY29w+mp/lOW/Q1KpjUQD0ZBbfEkS5H4MfwTdOeJH3fyLKmklaM2mnOyEWUZu
1nbG5oI7AFgMzTJsIANw2DMnsh1tSRbrgk9+i9QDIkGQNJ2U++wi/lwkcin9LCmW
8hZyBvpWl4CfOwkAAm4OAMXlI8MWEj3Tyxn9IzOO48M+F2oyeWFpVYCDbQBjoC4o
HnFNbmizp2638Jdi14eHO8T2Hpqd2gZ47RVKee5hciNwjYrzdmDdp3UCAwEAAaNj
MGEwHQYDVR0OBBYEFKjS1UpsW5T2zhWg/ogMDn+RxJNQMB8GA1UdIwQYMBaAFKjS
1UpsW5T2zhWg/ogMDn+RxJNQMA8GA1UdEwEB/wQFMAMBAf8wDgYDVR0PAQH/BAQD
AgEGMA0GCSqGSIb3DQEBCwUAA4IBAQBeED4BBiCKTIyACFCa8blPeg3150lujGaI
egFBUbnZ++ROFx2VoPy81dNjHDYK8xEl7RWscf4Ky0ORDZdQC7VUuqXeXwli/i9J
IkrNeS226l9zSzo4fsGu8QgRez2MD8Zi6eomF0Tjsv7rPiY1Bkp3Tzzh6CcYa9L2
Za4291Ud3wdHkARhQYcR+sf39gSShJnVWECbazFMslg4GfInmdfo3evDyCW7DeEF
MGOGmysVjiNYAcBm0GeEw+dQaD3H3kFkkx72S/uKA6NJPkwppIXmAWwfiZt4mG8E
z2BU5Zt8h8eftvxdA4oCQre4inxJnD3M5KW50qHzKsq4DuVCs/WL
-----END CERTIFICATE-----
""",
    )

    # ctx_a trusts ONLY CA A; ctx_b trusts ONLY CA B (empty-start bare contexts).
    auto ctx_a = tls.SSLContext()
    tls.load_verify_locations(ctx_a, ca_a, "")
    auto ctx_b = tls.SSLContext()
    tls.load_verify_locations(ctx_b, ca_b, "")

    auto srv_a = listen_server(
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
    auto srv_b = listen_server(
        """-----BEGIN CERTIFICATE-----
MIIDUzCCAjugAwIBAgIUe0msv6u7i8RYK3r0m558+wQOv0owDQYJKoZIhvcNAQEL
BQAwGDEWMBQGA1UEAwwNWmFwIFRlc3QgQ0EgYjAgFw0yNjExMTAxMTI5MDdaGA8y
MTI2MTAxNzExMjkwN1owFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG
9w0BAQEFAAOCAQ8AMIIBCgKCAQEAlvOJvxIo4kOwOzefe47iKSl03vapggh9l1yP
sqXPT1bnq/dOULK5+IJ6rhQltftF6XVQQR5k7RrLqWVXnDmQj/PVpeDsgC4+aHSK
HGUOfjrc0Yonmw3gHbYuNE3rrvUxFvftzv+yS4ShLCaQUgwMouhDBiEKrrRnDuFa
cC+MrAydVA5qnOAQYg+XfrI7PSAoq3viTQAW/EYtnQJYjG409nCEx3+chc2gZ8FX
9QWrHsCYp2lJhctcdmsoVHZP1vszUBtEnA3Zp2mZ5JxbeUpino2T3AAsApVXTovf
jMbJCR8zfyVfonONrzxrGq5FPvmhnU51e26trzbg2dpWJU5fcQIDAQABo4GWMIGT
MBoGA1UdEQQTMBGCCWxvY2FsaG9zdIcEfwAAATAJBgNVHRMEAjAAMAsGA1UdDwQE
AwIFoDAdBgNVHSUEFjAUBggrBgEFBQcDAQYIKwYBBQUHAwIwHQYDVR0OBBYEFGiM
VxK1dUTS7YfUFgge01Ua2aSbMB8GA1UdIwQYMBaAFKjS1UpsW5T2zhWg/ogMDn+R
xJNQMA0GCSqGSIb3DQEBCwUAA4IBAQAHAhniyN9EHL+9TsCpx7KVkAuKkr5S7cB4
Xp6EHYPWa6lofwTi5Wn1Fdg7CSZRxlNG/4nXW9FCvTgM2bCBUEEos7kY5PP4zjHJ
T3ST3yK7NyX/CKi8ktpnmsHIpMvdGi5CG45soczeY98DzjMb+8N2Oh1p5JpQjN08
P95iGFO38nOmJhTyBCRjnvBIG9HxQ8cj87CxtEcDfVMRg16j1hTzzWab3OQe35Pq
JK/EOr5LhWqfq1vgz31IdU/wsQeER1+zhC9KLPCFokMAG0exy89oeWWcgbY8nB+x
IAXjes9fUe/anDuMyr639AH4Usb75KXiQq++5iqapHxGn/+g8w2W
-----END CERTIFICATE-----
""",
        """-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCW84m/EijiQ7A7
N597juIpKXTe9qmCCH2XXI+ypc9PVuer905Qsrn4gnquFCW1+0XpdVBBHmTtGsup
ZVecOZCP89Wl4OyALj5odIocZQ5+OtzRiiebDeAdti40Teuu9TEW9+3O/7JLhKEs
JpBSDAyi6EMGIQqutGcO4VpwL4ysDJ1UDmqc4BBiD5d+sjs9ICire+JNABb8Ri2d
AliMbjT2cITHf5yFzaBnwVf1BasewJinaUmFy1x2ayhUdk/W+zNQG0ScDdmnaZnk
nFt5SmKejZPcACwClVdOi9+MxskJHzN/JV+ic42vPGsarkU++aGdTnV7bq2vNuDZ
2lYlTl9xAgMBAAECggEAOmyhTFRFgamsLIdPUV3QmBjlFKUIn30UywA21/DIJ8+d
lFgANkiupQovXHAM1n4eJYlT2hhe/MxJtlM0kRFQq/62muDO+J+tZPVyPSI59U5o
CCv+P0ukCivofzMHfOyGpvqnzt31jrgTP4jfg/oI7AaUZmtkfJDzcHV1cQVeX3pc
ACpBiaU4pC311mQ6U/C9VzJqKOSaOnYPKVQMFSaYaCxBI2LChdLEy25sScubAsTC
JQpU7DUsp+kUPqVeonmD4VcZpffa6ogx1q80ECgSyxTaOjj+NYNGcPvt9ryMww34
LmSEa6Ddxde8kaOy8JOC1nthQUsh0Nv+ZNrQXtsKvwKBgQDQNs+RLJgF3tJ891Im
nTBUD8aJ7gOo/5fvUk5uE0acRDODs9/UbJia8dH82Ny6U05rBTZX2tRmJVZ2u9S8
0ByU440ughpD8BhscmmCvPt3X1sLAI2qIFZTvqNPx07yUFV1DnzVTA/l/hjmjvUy
/fjPeZ/H/QzhRjv5mJv05k7/UwKBgQC5mF7rzs9JfZNna2xyV26YB2mT3Ogpq6z9
mbBx7AkECWzieuA/gaVYE1B+VeJ/TSV5/XM6bc77eR1+Jxmcbz/ON+H63HVLTPEL
D/VTWu4OomzFG756nLMBw1iNkIIDD9u7N4h5OB00qUGmT7NQEUBUFbO8lxl9cmSq
IvL3gXuBqwKBgQCGXEhXo1I6v45O6/5MQrU0GbYBveEIxl8SJafM6o1vO6eOfp2a
47Uaj5/1LTBK4IVDFGr8CI91lvSTzCO88ALxpWGuuHAglSf9m1sw7ajMYHFMyT+h
oBp1Ehkq2y+7uFFdJ0sxDVNZFD6XUXszO6a6o/6SvF36ZStAptVFFPW3VwKBgCX6
kzoH+Q0d4GMibW+8ioobwjQa2XtlgSHoKF43oeSslRaHdIoqWo56bVdc9dt9CvU6
szC6tcdqEmn1UF1dup3Ve63dHTeKiCzuWBJ1og9ml4GCWuk2Vb8/nUhi0OgVf66A
/7FmlTtog2GoH5Z74UETq9MkzHeKSCU0v6l8Wqy/AoGBAIlYAq8uYe9kiSWaQlEJ
lg7SrRfIN1yy/IoxFga1+ze+AofXcXzl4qg6NspakhuHmTMbcsj4oc8z58t22C1y
DN+SlLdNAz82FFa7eN9KEZmRIPPn6GSrNBm0IMwvasjgBiUTo4SD5pIlRc9BJkeq
ot8JGYRdyOhe79AfMs/SK4WN
-----END PRIVATE KEY-----
""",
    )
    spawn serve(srv_a)
    spawn serve(srv_b)
    auto port_a = tls.port(srv_a)
    auto port_b = tls.port(srv_b)

    # Cross-CA probes are rejected; same-CA probes are echoed.
    print("ctxB->srvA: " + probe(ctx_b, port_a))
    print("ctxA->srvB: " + probe(ctx_a, port_b))
    print("ctxA->srvA: " + probe(ctx_a, port_a))
    print("ctxB->srvB: " + probe(ctx_b, port_b))

    tls.close_listener(srv_a)
    tls.close_listener(srv_b)
