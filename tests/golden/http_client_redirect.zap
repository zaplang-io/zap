# Golden fixture: http.Client redirect policy (#1049). Two one-shot servers:
#   1. following disabled -> a 302 is returned as-is (status 302, empty redirect
#      chain), not followed.
#   2. max_redirects = 0 with following on -> the first 302 exceeds the cap and
#      raises a catchable ValueError.
# Each server answers exactly one request (Connection: close) and holds until the
# client signals it has the result, so the exchange is deterministic.

def redir_server(channel[int64] portch, channel[int64] done):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto resp = "HTTP/1.1 302 Found\r\nLocation: http://example.invalid/next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    auto req = http.read_request(c)
    auto n = net.send(c, resp)
    auto ack = <-done
    net.close_conn(c)
    net.close_listener(l)

def main():
    auto p1 = channel[int64]()
    auto d1 = channel[int64]()
    spawn redir_server(p1, d1)
    auto port1 = <-p1
    auto c1 = http.Client(follow=False)
    auto r1 = c1.get(http.url("127.0.0.1", port1, "/x"))
    print(http.status(r1))
    if len(http.redirects(r1)) == 0:
        print("not-followed")
    c1.shutdown()
    d1 <- 1
    auto p2 = channel[int64]()
    auto d2 = channel[int64]()
    spawn redir_server(p2, d2)
    auto port2 = <-p2
    auto c2 = http.Client(max_redirects=0)
    try:
        auto r2 = c2.get(http.url("127.0.0.1", port2, "/x"))
        print("unexpected: followed")
    except ValueError:
        print("too-many-redirects")
    c2.shutdown()
    d2 <- 1
    print("done")
