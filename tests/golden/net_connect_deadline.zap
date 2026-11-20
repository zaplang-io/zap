# Golden fixture: hostname-aware bounded TCP connect (#1138). Unlike
# net.connect_ip_deadline (numeric IP only), net.connect_deadline resolves a DNS
# name and bounds the whole operation — resolution + every address attempt — by
# one absolute CLOCK_MONOTONIC-ns deadline (time.monotonic_ns()-based). A dial to
# a listening "localhost" with a generous budget succeeds; a deadline already in
# the past raises a catchable TimeoutError before any work. Network-free:
# "localhost" resolves from the hosts database to the loopback the server binds.

import time


def srv(channel[int64] portch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    net.close_conn(c)
    net.close_listener(l)


def main():
    auto portch = channel[int64]()
    spawn srv(portch)
    auto port = <-portch

    # A hostname dial with a generous (+5s) budget connects.
    auto deadline = time.monotonic_ns() + 5000000000
    auto c = net.connect_deadline("localhost", port, deadline)
    print("connected")
    net.close_conn(c)

    # A deadline already in the past raises TimeoutError before any work.
    try:
        auto c2 = net.connect_deadline("localhost", port, time.monotonic_ns() - 1)
        print("no-timeout")
    except TimeoutError:
        print("timed-out")
