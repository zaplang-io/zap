# Golden fixture: net.port on a Unix-domain socket (#1159). A Unix socket has no
# port, so net.port raises a catchable ValueError instead of reading an unrelated
# byte of the sockaddr_un; on a TCP socket it still returns the bound port.

import fs


def main():
    # TCP: net.port returns the bound ephemeral port.
    auto l = net.listen("127.0.0.1", 0)
    auto p = net.port(l)
    print(p > 0)  # True
    net.close_listener(l)

    # AF_UNIX: net.port raises ValueError (no port).
    auto dir = fs.mkdtemp()
    auto path = dir + "/p.sock"
    auto u = net.listen_unix(path)
    try:
        auto q = net.port(u)
        print("no-raise")
    except ValueError:
        print("no-port")
    net.close_listener(u)
