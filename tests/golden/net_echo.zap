# Golden fixture: raw TCP echo over the `net` sockets (ticket #104, D-0056).
#
# A server goroutine binds an ephemeral port (net.listen host 0 -> OS-assigned),
# publishes it to main over a channel, accepts ONE connection and echoes the
# bytes it receives. Main dials that port, sends a line, and prints what comes
# back. Both the accept and the connect/recv park on the netpoller (epoll),
# freeing their worker M; the single request/response exchange is deterministic
# regardless of how the scheduler interleaves the two goroutines.
#
# The server handles exactly one connection then returns, so the program's
# end-of-main goroutine join completes (nothing left parked on accept).

def echo_server(channel[int64] portch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto msg = net.recv(c, 1024)
    net.send(c, msg)
    net.close_conn(c)
    net.close_listener(l)

def main():
    auto portch = channel[int64]()
    spawn echo_server(portch)
    auto port = <-portch
    auto c = net.connect("127.0.0.1", port)
    net.send(c, "ping-pong")
    auto reply = net.recv(c, 1024)
    print(reply)
    net.close_conn(c)
