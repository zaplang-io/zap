# A TCP peer consumes a ClientHello byte but never completes TLS. The numeric
# address avoids DNS; hostname remains the verified certificate/SNI identity.

def peer(bytes listener, channel[int64] release):
    auto socket = net.accept(listener)
    net.close_listener(listener)
    try:
        net.recv(socket, 1)
        auto done = <-release
    finally:
        net.close_conn(socket)

def main():
    auto listener = net.listen("127.0.0.1", 0)
    auto port = net.port(listener)
    auto release = channel[int64]()
    spawn peer(listener, release)
    auto timed_out = False
    try:
        auto socket = tls.connect_resolved(
            "localhost",
            "127.0.0.1",
            port,
            time.monotonic_ns() + 100000000,
        )
        tls.close_conn(socket)
    except TimeoutError:
        timed_out = True
    finally:
        release <- 1
    if not timed_out:
        raise ValueError("TLS handshake ignored dial budget")
    print("verified resolved TLS dial bounds a stalled handshake")
