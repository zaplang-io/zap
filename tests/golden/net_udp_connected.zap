# Golden fixture: connected UDP over the `net` sockets (#991, epic #985).
# net.udp_connect records a default peer on a datagram socket so the ordinary
# net.send / net.recv (the same calls the TCP streams use) work on it — Python's
# `socket.connect` for a SOCK_DGRAM. The server side stays connectionless
# (recvfrom to learn the sender, sendto to reply), proving the two styles
# interoperate on the one wire.

def main():
    auto srv = net.udp_socket()
    net.udp_bind(srv, "127.0.0.1", 0)
    auto port = net.port(srv)

    # Connected client: send/recv with no per-call address.
    auto cli = net.udp_socket()
    net.udp_connect(cli, "127.0.0.1", port)
    auto nsent = net.send(cli, "hello")
    print(nsent)  # 5

    auto data, host, src_port = net.recvfrom(srv, 1024)
    print(data)  # hello

    # Reply to the connected client; it reads with a plain recv.
    net.sendto(srv, "world", host, src_port)
    auto reply = net.recv(cli, 1024)
    print(reply)  # world

    net.close_conn(srv)
    net.close_conn(cli)
