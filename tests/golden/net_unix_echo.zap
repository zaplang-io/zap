# Golden fixture: AF_UNIX stream round-trip over the `net` sockets (#993, epic
# #985). A server binds a Unix-domain stream socket at a path inside a fresh
# temp directory, publishes the path to main over a channel, accepts ONE
# connection and echoes the bytes it receives. Main connects to that path,
# sends a line, and prints the echo. Both accept and connect/recv park on the
# netpoller, exactly as the TCP `net_echo` fixture — `listen_unix`/`connect_unix`
# reuse the same `net.accept` / `recv` / `send` / `close_*` surface.
#
# The server handles exactly one connection then returns, so the end-of-main
# coroutine join completes (nothing left parked on accept).

import fs


def echo_server(channel[str] pathch, str path):
    auto l = net.listen_unix(path)
    pathch <- path
    auto c = net.accept(l)
    auto msg = net.recv(c, 1024)
    net.send(c, msg)
    net.close_conn(c)
    net.close_listener(l)


def main():
    auto dir = fs.mkdtemp()
    auto path = dir + "/echo.sock"
    auto pathch = channel[str]()
    spawn echo_server(pathch, path)
    auto connect_path = <-pathch
    auto c = net.connect_unix(connect_path)
    net.send(c, "ping-unix")
    auto reply = net.recv(c, 1024)
    print(reply)  # ping-unix
    net.close_conn(c)
