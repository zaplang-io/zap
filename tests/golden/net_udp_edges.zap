# Golden fixture: UDP edge cases over the `net` sockets (#991, epic #985).
#   1. A zero-length datagram is a VALID empty payload, not EOF — net.recvfrom
#      returns it with len 0 (unlike a TCP net.recv of "" which signals EOF).
#   2. net.set_read_timeout bounds a net.recvfrom park exactly as it bounds a
#      net.recv: with no datagram waiting, the recvfrom raises a catchable
#      TimeoutError once the deadline elapses (distinct from an empty datagram).

def main():
    auto srv = net.udp_socket()
    net.udp_bind(srv, "127.0.0.1", 0)
    auto port = net.port(srv)

    auto cli = net.udp_socket()

    # 1. Empty datagram round-trips as a 0-length payload.
    net.sendto(cli, "", "127.0.0.1", port)
    auto data, host, src_port = net.recvfrom(srv, 64)
    print(len(data))  # 0

    # 2. A bounded recvfrom with nothing to read raises TimeoutError.
    net.set_read_timeout(srv, 50)
    try:
        auto d2, h2, p2 = net.recvfrom(srv, 64)
        print("no-timeout")
    except TimeoutError:
        print("timeout")

    net.close_conn(srv)
    net.close_conn(cli)
