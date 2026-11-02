# Golden fixture: the whole-operation read deadline (ticket #567) bounds a
# slow-dribble (slowloris) request over TLS, through `http.read_request` (the
# #556 transport seam). The TLS twin of `net_read_deadline_slowloris`: it proves
# the whole-op deadline is threaded through the SAME seam as the per-park
# deadline, so it fires over TLS too, not just plaintext.
#
# A server coroutine binds a TLS listener, accepts one conn, sets a VERY LARGE
# per-park timeout and a SHORT whole-op deadline, then calls `http.read_request`.
# The client dribbles an incomplete request (bytes that never terminate the
# headers) one byte at a time, each far under the per-park timeout — so only the
# whole-op deadline can cut the read off. It fires, `http.read_request` raises a
# catchable `TimeoutError`, the server prints a deterministic marker.
#
# Two robustness choices make this pass under both collectors and arches without
# racing the wall clock:
#   * The per-park timeout is huge (120 s), so if the whole-op deadline were
#     BROKEN the read would not merely fall back to a per-park timeout (which
#     would wrongly still print the marker) — it would block past the harness
#     timeout and the test would HANG, i.e. fail, rather than false-pass.
#   * The server does NOT close the conn until the client has finished dribbling
#     (a `done` handshake): unlike plaintext `net.send` (MSG_NOSIGNAL), a
#     `tls.send` into a just-closed conn can raise SIGPIPE, so the client must
#     never send after the close. The client dribbles a bounded run that is
#     comfortably longer than the whole-op deadline, then rendezvouses on `done`.
# The marker — not any wall-clock number — is what the golden asserts.


def tls_slow_server(channel[int64] portch, channel[int64] done):
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
    portch <- tls.port(l)
    auto c = tls.accept(l)
    net.set_read_timeout(c, 120000)  # huge per-park: dribbles never trip it; a broken
    # whole-op would hang past the harness, not false-pass
    net.set_read_deadline(c, 300)  # whole-op budget: bounds the entire request
    try:
        auto req = http.read_request(c)
        print("unexpected: read returned")
    except TimeoutError:
        print("tls-slowloris-fired")
    # Signal AFTER firing but BEFORE closing: the client is still dribbling into
    # the (open) conn, and only stops once it receives this. So we never close
    # under an in-flight tls.send (which could SIGPIPE).
    done <- 1
    tls.close_conn(c)
    tls.close_listener(l)


def main():
    auto portch = channel[int64]()
    auto done = channel[int64]()
    spawn tls_slow_server(portch, done)
    auto port = <-portch
    auto c = tls.connect_insecure("127.0.0.1", port)
    # Dribble an incomplete request, one byte at a time well under the per-park
    # timeout. Bounded, but comfortably longer than the whole-op deadline, and the
    # server holds the conn open until we finish — so we never send into a closed
    # conn (no broken pipe).
    for i in range(0, 30):
        try:
            tls.send(c, "x")
        except Exception:
            break
        time.sleep_ns(time.millis(50))
    auto ack = <-done
    tls.close_conn(c)
    print("done")
