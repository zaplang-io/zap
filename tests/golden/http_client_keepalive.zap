# Golden fixture: http.Client reuses a pooled keep-alive connection (#990).
#
# A raw-net server accepts EXACTLY ONE connection and serves two requests on it,
# each answered with a keep-alive response (Content-Length + Connection:
# keep-alive). The client issues two GETs to the same origin through one
# http.Client; both succeed, which proves the second GET reused the pooled socket
# — had it opened a new connection the single-accept server would never answer it
# and the program would hang. The server holds the socket open until the client
# signals (over `done`) that both calls returned, so the exchange is
# deterministic.

def ka_server(channel[int64] portch, channel[int64] done):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto resp = "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: keep-alive\r\n\r\nfoo"
    auto req1 = http.read_request(c)
    auto n1 = net.send(c, resp)
    auto req2 = http.read_request(c)
    auto n2 = net.send(c, resp)
    auto ack = <-done
    net.close_conn(c)
    net.close_listener(l)

def main():
    auto portch = channel[int64]()
    auto done = channel[int64]()
    spawn ka_server(portch, done)
    auto port = <-portch
    auto url = http.url("127.0.0.1", port, "/x")
    auto client = http.Client()
    auto r1 = client.get(url)
    auto r2 = client.get(url)
    print(http.status(r1))
    print(http.body(r1))
    print(http.status(r2))
    print(http.body(r2))
    client.shutdown()
    done <- 1
    print("done")
