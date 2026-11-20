# Golden fixture: TCP keepalive interval tuning (#1141, follow-up to #994).
# #994's set_keepalive only toggles SO_KEEPALIVE; these set the idle time before
# the first probe, the interval between probes, and the probe count, and read
# back exactly what was set (the POSIX TCP_KEEPIDLE/TCP_KEEPINTVL/TCP_KEEPCNT
# knobs are stored verbatim, unlike the OS-rounded buffer sizes). A non-positive
# value is a catchable ValueError.

def ka_server(channel[int64] portch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    net.recv(c, 16)
    net.close_conn(c)
    net.close_listener(l)


def main():
    auto portch = channel[int64]()
    spawn ka_server(portch)
    auto port = <-portch
    auto c = net.connect("127.0.0.1", port)

    net.set_keepalive(c, True)
    net.set_keepalive_idle(c, 120)
    net.set_keepalive_interval(c, 30)
    net.set_keepalive_count(c, 4)
    print(net.get_keepalive_idle(c))  # 120
    print(net.get_keepalive_interval(c))  # 30
    print(net.get_keepalive_count(c))  # 4

    # A non-positive value is a ValueError (checked before setsockopt).
    try:
        net.set_keepalive_count(c, 0)
        print("no-raise")
    except ValueError:
        print("rejected")

    net.send(c, "x")
    net.close_conn(c)
