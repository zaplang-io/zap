# Golden fixture: a set read timeout does not disturb a normal read (ticket #485,
# D-0096). The companion to `net_read_timeout`: with a generous timeout set, a
# peer that sends promptly is still echoed back unchanged — the deadline only
# fires on an idle wait, never on data that arrives in time. This pins that the
# timed path and the default fast path agree when data is available.

def echo_server(channel[int64] portch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    net.set_read_timeout(c, 5000)
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
