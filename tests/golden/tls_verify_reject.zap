# Golden fixture: TLS certificate verification is ON by default and REJECTS a
# cert with no trust anchor (ticket #147, D-0062 — the non-negotiable security
# property: the client is never default-insecure).
#
# The same self-signed loopback echo server as tls_echo.zap, but the client uses
# the DEFAULT `tls.connect` — certificate-chain + hostname verification against
# the system trust store. The server's self-signed cert chains to no trusted CA,
# so the handshake MUST fail; the raised exception is caught and we print
# "rejected". Reaching the connected branch would be the bug this guards against.

def main():
    auto port = tls._test_server()
    try:
        auto conn = tls.connect("127.0.0.1", port)
        print("BUG: connected to an unverified server")
        tls.close_conn(conn)
    except Exception as e:
        print("rejected")
