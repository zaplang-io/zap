# Golden fixture: AF_UNIX datagram round-trip over the `net` sockets (#993, epic
# #985). net.unixgram_socket + unixgram_bind + sendto_unix + recvfrom_unix are
# the connectionless Unix-domain counterparts of the #991 UDP calls, addressing
# peers by path. Both ends bind a known path inside a fresh temp directory, so
# the sender path recvfrom_unix reports is deterministic. A datagram sent to a
# bound loopback socket is delivered synchronously, so one coroutine suffices.

import fs


def main():
    auto dir = fs.mkdtemp()
    auto spath = dir + "/srv.sock"
    auto cpath = dir + "/cli.sock"

    auto srv = net.unixgram_socket()
    net.unixgram_bind(srv, spath)

    auto cli = net.unixgram_socket()
    net.unixgram_bind(cli, cpath)

    auto nsent = net.sendto_unix(cli, "ping", spath)
    print(nsent)  # 4

    auto data, from_path = net.recvfrom_unix(srv, 1024)
    print(data)  # ping
    print(from_path == cpath)  # True (the sender's bound path)

    # Echo the datagram back to whoever sent it.
    net.sendto_unix(srv, data, from_path)
    auto reply, rpath = net.recvfrom_unix(cli, 1024)
    print(reply)  # ping

    net.close_conn(srv)
    net.close_conn(cli)
