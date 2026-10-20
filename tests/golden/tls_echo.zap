# Golden fixture: TLS client round-trip over the netpoller (ticket #147, D-0062).
#
# `tls._test_server()` starts a background self-signed TLS echo server on an
# ephemeral loopback port (the public TLS server is deferred; this is the test
# scaffold). Main is the real TLS *client*: it connects with verification
# DISABLED (the self-signed cert has no trust anchor), sends one encrypted line
# and prints the decrypted echo. The handshake and the send/recv all ride the
# netpoller — each WANT_READ / WANT_WRITE parks the goroutine and frees its M —
# so the exchange is deterministic no matter how the scheduler interleaves the
# client and the server goroutine, under both collectors.
#
# `connect_insecure` is the explicit, loudly-named opt-out from verification;
# the default `tls.connect` (verification ON) rejecting this same self-signed
# server is covered by tls_verify_reject.zap.

def main():
    auto port = tls._test_server()
    auto conn = tls.connect_insecure("127.0.0.1", port)
    tls.send(conn, "secure-ping")
    print(tls.recv(conn, 1024))
    tls.close_conn(conn)
