# Same public test certificate as tls_read_timeout. SSL_CERT_FILE is scoped to
# this isolated fixture process; the library never changes process trust roots.

def peer(
    channel[int64] ports,
    channel[str] certificates,
    channel[int64] entered,
    channel[int64] release,
):
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
    auto listener = tls.listen("127.0.0.1", 0, cert_path, key_path)
    ports <- tls.port(listener)
    certificates <- cert_path
    try:
        for mode in range(0, 2):
            auto socket = tls.accept(listener)
            try:
                tls.recv(socket, 1)
                if mode == 1:
                    entered <- 1
                auto stopped = <-release
            finally:
                tls.close_conn(socket)
    finally:
        tls.close_listener(listener)

def writer(bytes socket, bytes payload, Atomic[int64] cancelled, Atomic[int64] cleaned):
    try:
        tls.send(socket, payload)
    except Cancelled:
        cancelled.add(1)
    finally:
        tls.close_conn(socket)
        cleaned.add(1)

def trigger(channel[int64] entered):
    auto begun = <-entered
    raise ValueError("cancel TLS write fixture")

def main():
    auto ports = channel[int64]()
    auto certificates = channel[str]()
    auto entered = channel[int64]()
    auto release = channel[int64]()
    spawn peer(ports, certificates, entered, release)
    auto port = <-ports
    auto certificate = <-certificates
    os.setenv("SSL_CERT_FILE", certificate)
    auto payload = b"x"
    for i in range(0, 25):
        payload = payload + payload
    auto socket = tls.connect_resolved(
        "localhost",
        "127.0.0.1",
        port,
        time.monotonic_ns() + 5000000000,
    )
    net.set_deadline_ns(socket, time.monotonic_ns() + 100000000)
    auto timed_out = False
    try:
        tls.send(socket, payload)
    except TimeoutError:
        timed_out = True
    finally:
        tls.close_conn(socket)
        release <- 1
    if not timed_out:
        raise ValueError("backpressured TLS write ignored deadline")
    socket = tls.connect_resolved(
        "localhost",
        "127.0.0.1",
        port,
        time.monotonic_ns() + 5000000000,
    )
    Atomic[int64] cancelled = Atomic(0)
    Atomic[int64] cleaned = Atomic(0)
    auto failed = False
    try:
        scope:
            spawn writer(socket, payload, cancelled, cleaned)
            spawn trigger(entered)
    except Exception:
        failed = True
    if not failed or cancelled.load() != 1 or cleaned.load() != 1:
        raise ValueError("TLS write cancellation did not clean up")
    release <- 1
    print("verified resolved TLS connection")
    print("backpressured TLS write deadline and cancellation")
