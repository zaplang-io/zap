# Golden fixture: UDP datagram round-trip over the `net` sockets (#991, epic
# #985). A server binds an ephemeral IPv4 port (udp_bind host 0 -> OS-assigned,
# read back with net.port), the client sends one datagram to it with
# net.sendto, the server reads it with net.recvfrom (recovering the client's
# source host + port), echoes it straight back to that address, and the client
# reads the echo.
#
# UDP is connectionless, so no accept/connect handshake is needed and a single
# coroutine suffices: a sendto to a bound loopback socket is delivered to the
# destination's receive buffer synchronously, so the matching recvfrom finds
# the datagram already waiting. The sender address the server reports is the
# loopback 127.0.0.1 the client auto-bound to on its first send.

def main():
    auto srv = net.udp_socket()
    net.udp_bind(srv, "127.0.0.1", 0)
    auto port = net.port(srv)

    auto cli = net.udp_socket()
    auto nsent = net.sendto(cli, "ping", "127.0.0.1", port)
    print(nsent)  # 4 bytes queued in the one datagram

    auto data, host, src_port = net.recvfrom(srv, 1024)
    print(data)  # ping
    print(host)  # 127.0.0.1 (the client's auto-bound loopback source)

    # Echo the datagram back to whoever sent it.
    net.sendto(srv, data, host, src_port)
    auto reply, rhost, rport = net.recvfrom(cli, 1024)
    print(reply)  # ping

    net.close_conn(srv)
    net.close_conn(cli)
