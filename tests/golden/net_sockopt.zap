# Golden fixture: socket options over the `net` module (#994, epic #985).
# Named setters/getters for TCP_NODELAY, SO_KEEPALIVE, SO_RCVBUF/SO_SNDBUF and
# SO_REUSEADDR/SO_REUSEPORT. A connected TCP loopback pair exercises the
# per-connection options (set then read back), and an unbound UDP socket
# exercises the address-reuse options before bind. Deterministic: the boolean
# options read back exactly what was set; a buffer size is only asserted
# positive, since the OS may round or double the requested value.

def opt_server(channel[int64] portch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto msg = net.recv(c, 16)
    net.close_conn(c)
    net.close_listener(l)


def main():
    auto portch = channel[int64]()
    spawn opt_server(portch)
    auto port = <-portch
    auto c = net.connect("127.0.0.1", port)

    # TCP_NODELAY + SO_KEEPALIVE read back what was set.
    net.set_nodelay(c, True)
    print(net.get_nodelay(c))  # True
    net.set_keepalive(c, True)
    print(net.get_keepalive(c))  # True

    # Buffer sizes: the OS value is >= 0 (often rounded up or doubled).
    net.set_recv_buffer(c, 65536)
    print(net.get_recv_buffer(c) > 0)  # True
    net.set_send_buffer(c, 65536)
    print(net.get_send_buffer(c) > 0)  # True

    net.send(c, "x")
    net.close_conn(c)

    # Address-reuse options on an unbound socket, applied before bind.
    auto u = net.udp_socket()
    net.set_reuseaddr(u, True)
    net.set_reuseport(u, True)
    net.udp_bind(u, "127.0.0.1", 0)
    print("reuse-ok")
    net.close_conn(u)
