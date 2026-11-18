# Golden fixture: per-request HTTP client timeouts (ticket #986, D-0229).
#
# `http.get(url, timeout=ms)` sets a total deadline on the whole call (connect +
# reads + redirects). Three facets, each asserted by a deterministic marker (not
# any wall-clock number), so the golden does not race under either collector or
# arch:
#   1. In-time: a server that replies promptly + a generous timeout -> success.
#   2. Expiry: a server that reads the request but never replies + a small
#      timeout -> the read deadline fires and raises a catchable TimeoutError.
#   3. Validation: a negative timeout is a ValueError, raised before any dial.
#
# The stall server holds its connection open until the client signals (over
# `done`) that its call has already timed out, so the exchange is deterministic.

def echo_server(channel[int64] portch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto req = http.read_request(c)
    http.respond(c, 200, "ok")
    net.close_conn(c)
    net.close_listener(l)

def stall_server(channel[int64] portch, channel[int64] done):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto req = http.read_request(c)
    auto ack = <-done
    net.close_conn(c)
    net.close_listener(l)

def main():
    auto p1 = channel[int64]()
    spawn echo_server(p1)
    auto port1 = <-p1
    auto url1 = http.url("127.0.0.1", port1, "/ok")
    auto resp = http.get(url1, timeout=5000)
    print(http.status(resp))
    print(http.body(resp))
    try:
        auto bad = http.get("http://127.0.0.1:9/x", timeout=-1)
        print("unexpected: negative timeout accepted")
    except ValueError:
        print("negative-timeout-rejected")
    auto p2 = channel[int64]()
    auto done = channel[int64]()
    spawn stall_server(p2, done)
    auto port2 = <-p2
    auto url2 = http.url("127.0.0.1", port2, "/slow")
    try:
        auto slow = http.get(url2, timeout=50)
        print("unexpected: slow response returned")
    except TimeoutError:
        print("client-timeout-fired")
    done <- 1
    print("done")
