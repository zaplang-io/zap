# Golden fixture: the whole-operation read deadline (ticket #567) bounds a
# slow-dribble (slowloris) request over TLS, through `http.read_request` (the
# #556 transport seam). The TLS twin of `net_read_deadline_slowloris`: it proves
# the whole-op deadline is threaded through the SAME seam as the per-park
# deadline, so it fires over TLS too, not just plaintext.
#
# `main` is the server/reader: it binds a TLS listener, starts a DETACHED dribbler
# coroutine (a daemon, so it never blocks program exit and is torn down when main
# returns), accepts the conn, sets a GENEROUS per-park timeout (so the 50 ms
# dribbles never trip it — proving the whole-op deadline, not the per-park one, is
# what fires) and a SHORT whole-op deadline, then calls `http.read_request`. The
# dribbler sends an incomplete request (bytes that never terminate the headers)
# one at a time, forever. Only the whole-op deadline can cut the read off: it
# fires, `http.read_request` raises a catchable `TimeoutError`, main prints the
# marker and returns (the daemon dribbler is terminated at exit, so it never sends
# into a closed conn — no broken pipe). If the whole-op deadline were broken the
# read would never return (the dribbles keep the per-park clock alive) and the
# program would HANG rather than silently passing. The marker — not any wall-clock
# number — is what the golden asserts, so it does not race under either collector
# or arch.


def tls_dribbler(int64 port):
    auto c = tls.connect_insecure("127.0.0.1", port)
    # Dribble an incomplete request forever, one byte at a time, each well under
    # the server's per-park timeout. Detached: terminated at program exit.
    while True:
        try:
            tls.send(c, "x")
        except Exception:
            break
        time.sleep_ns(time.millis(50))


def tls_slow_server():
    auto cert = """-----BEGIN CERTIFICATE-----
MIIDJzCCAg+gAwIBAgIUY0g00QCmIsdA4L4Ta31cDYVdzHwwDQYJKoZIhvcNAQEL
BQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MCAXDTI2MTEwMTAzNDgxMloYDzIxMjYx
MDA4MDM0ODEyWjAUMRIwEAYDVQQDDAlsb2NhbGhvc3QwggEiMA0GCSqGSIb3DQEB
AQUAA4IBDwAwggEKAoIBAQCt3EjRT+LIPxRsxREWREWrx+lQFnqMzsxmqdOwJZRz
IeOY7A4x5E69NddJJe3aHIAfR5S+c9A5CBF2aVklnV30BgFTNUaFOKWOl31IY9FQ
v62FQF9ZAqQ4nopkMdjc4G4DIOSoz8xu+It1wTMfn8/HAbPozHQgy1gJuMzjz/O1
TKa6XGq9L82mgtM36Bv8eVXQhEcZ74+4hvkp07pMEKD7r4kNhGVZEOcCSuOf4IEk
AJ9SUWmB788JoQb5uy0rYTeiG+Ld0sE3kjNZDyThZ5ZN+GKVZ2DtK5T1Z2ZQxnmt
7jJ7weLIb8xbqFEeyV8FHdcB43UV+NfngITjgBB6tqkrAgMBAAGjbzBtMB0GA1Ud
DgQWBBTNlSfS/6wvgwhSKeyIEKooxBvYbTAfBgNVHSMEGDAWgBTNlSfS/6wvgwhS
KeyIEKooxBvYbTAPBgNVHRMBAf8EBTADAQH/MBoGA1UdEQQTMBGCCWxvY2FsaG9z
dIcEfwAAATANBgkqhkiG9w0BAQsFAAOCAQEARMCaTNhSotNS7AgrzxFWj4T5/Oza
QGJhRV8yHuv5qsjHlmOpZyFMGZot6r/PXXGA6q3w9DTemfoSjKGiPQIV0Evjp4Gu
2i56XFuc1TwzgDPrgh05I8wn8+XLRsQlfKcqwlln/gzzkBqQt3ZSg2IbFEKw6WUC
pIX15SF4onvok+2FBInEeWk+arxYpLvLR77HamKhi/hnNM/0QtXK4Fo0sadYTGjM
IOJ03TtcB+eqqayLksNBGo2DLMrMnocAbwStMr0RmOSOh9Q7zxUTeRGBEqLxN6XQ
yjHPFYgfB1TqGcHbD7LJKPOS3rjYmFFICkFgoPmYCQ4kVdudpEFDbU1yxA==
-----END CERTIFICATE-----
"""
    auto key = """-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCt3EjRT+LIPxRs
xREWREWrx+lQFnqMzsxmqdOwJZRzIeOY7A4x5E69NddJJe3aHIAfR5S+c9A5CBF2
aVklnV30BgFTNUaFOKWOl31IY9FQv62FQF9ZAqQ4nopkMdjc4G4DIOSoz8xu+It1
wTMfn8/HAbPozHQgy1gJuMzjz/O1TKa6XGq9L82mgtM36Bv8eVXQhEcZ74+4hvkp
07pMEKD7r4kNhGVZEOcCSuOf4IEkAJ9SUWmB788JoQb5uy0rYTeiG+Ld0sE3kjNZ
DyThZ5ZN+GKVZ2DtK5T1Z2ZQxnmt7jJ7weLIb8xbqFEeyV8FHdcB43UV+NfngITj
gBB6tqkrAgMBAAECggEAGMuCPQuyEZHhdVOziKPq9cE4dnoGNz+k6f1NWeE6JyNN
pLkBmz7utdBQaDe3ELSg1xUr0u+MYh3S87q5goJLbGpQ1Q+Tg5tdDRVwwhquNuBn
PEo+buDQD90YaoqrXXDKb7rIsYSVKLxck0oe4uM1TDHjeQ9zflvRre1DlzfIj3Hn
OW8tat8ljlwjDXPHNKJccLcn9ScBkvqjCYSmkGh0viuBX2S37o2QGEo49o9E1IOk
KCNZQAH/g9P8ThecCnqfeTn3A9514cRNqCYkNWVYY4CMzGbN/cCI05njx8Hdcfig
C35HuJiFj0YCcuXIQxwCVaJLOnpkvqa75QSBGO0ZaQKBgQDjtkJ+4HqlHOxZyf+t
Ng7QaU4exN0fnvnIsgiiwGgtjfQcpodD1ztNC3LJ3h3aVUMKSz6yY4SmZFwh8beN
sYEoiTOBiN/ogkQ0EMu0SntwAjfYRgwwtjJrltlf2ShIu6qHXud0cwK0ZPuHIcQ9
AS/sMTUUm2Dpyh98qJZI1B9WCQKBgQDDdW4Gs/MdyWaq6CPg1EKHnqHnoPv1T6eQ
vUNDVyalV+VCqa5HserknplJ343jTd8oUrJRyy/+8vzwCjiscVo7qjMGXFg1tFrK
Vr6aP6SZa4MtuDq79OT1mUJZofx2OE/pSNwmk1IcOnMPA3bL9YrO7aR6q36vKGPv
KVTmFTaykwKBgC6lWk/FSjpih2QJRg/EYBp/R9pzt1xP45kiJzS3t1+FkbkAv/q8
r1AkBTqm/nSIPLhyG4DNdBHlwx3M9y3Wa7/TAzMIx91IOhJ0UlY3X7vdKHVsO/kK
iZ+SgirpQCBOGSur3Dv1h1tOyFuxJhLdGGUI23fCMIMTvYl6u3ZlxZmJAoGBAJ2F
uSt7wRYjZzObrLaYOhn0dYUoT4bPMDtM3LWbuhCM+r4z47nyIF0pmF5Z5Y7oBk4H
/6A9wauVhtWpVy1amR72tvKMX6bAeF8Tc776RZxDck/ZZrRt1Ew+vKNiEgXwsi5P
QuVv2z/Wtz3FS+tqMvk7ulAGgC2HvqpTRBG9zPwzAoGBAJdB0F4awaT+0H+n80TR
x29GIzLvicHaw/U/LYviOHA/sPgO4DF0/uujyBS/nKaQTig/+fQaphz5ckL5i1mu
5TfkFJj1FcN9H4Ia/wa7eZtMeahhLvtweMe1J94BSTrql260aACmKe4IG7Hqn18I
52bNmc+xpiHHtv4dxzIFEc6C
-----END PRIVATE KEY-----
"""
    auto dir = fs.mkdtemp()
    auto cert_path = os.path_join(dir, "cert.pem")
    auto key_path = os.path_join(dir, "key.pem")
    fs.write_file(cert_path, cert)
    fs.write_file(key_path, key)
    auto l = tls.listen("127.0.0.1", 0, cert_path, key_path)
    auto port = tls.port(l)
    spawn detached tls_dribbler(port)
    auto c = tls.accept(l)
    net.set_read_timeout(c, 10000)  # generous per-park: a 50 ms dribble never trips it
    net.set_read_deadline(c, 300)  # whole-op budget: bounds the entire request
    try:
        auto req = http.read_request(c)
        print("unexpected: read returned")
    except TimeoutError:
        print("tls-slowloris-fired")
    print("done")


def main():
    tls_slow_server()
