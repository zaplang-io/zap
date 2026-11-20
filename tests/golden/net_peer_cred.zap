# Golden fixture: AF_UNIX peer credentials (#1143, follow-up to #993). A server
# reads net.peer_cred on an accepted Unix-domain connection and recovers the
# connecting peer's (uid, gid, pid). This is a single-process loopback, so the
# peer IS this process: its pid equals os.getpid() — a deterministic check. A
# `done` channel orders the server's output before main's, and peer_cred on a
# non-Unix (UDP) socket is a catchable ValueError (network-free). POSIX/Linux CI.

import os
import fs


def cred_server(channel[str] pathch, channel[int64] donech, str path):
    auto l = net.listen_unix(path)
    pathch <- path
    auto c = net.accept(l)
    auto uid, gid, pid = net.peer_cred(c)
    # Loopback: the connecting peer is this same process.
    if pid == os.getpid():
        print("cred-ok")
    else:
        print("cred-bad")
    net.close_conn(c)
    net.close_listener(l)
    donech <- 1


def main():
    auto dir = fs.mkdtemp()
    auto path = dir + "/cred.sock"
    auto pathch = channel[str]()
    auto donech = channel[int64]()
    spawn cred_server(pathch, donech, path)
    auto connect_path = <-pathch
    auto c = net.connect_unix(connect_path)
    # Keep the connection open until the server has read the credentials.
    auto done = <-donech
    net.close_conn(c)

    # peer_cred on a non-Unix socket is a ValueError.
    auto u = net.udp_socket()
    try:
        auto a, b, p = net.peer_cred(u)
        print("no-raise")
    except ValueError:
        print("not-unix")
    net.close_conn(u)
