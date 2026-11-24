# Golden fixture: http.Client(max_response_bytes=N) over EOF-framed bodies
# (#1448) — no Content-Length and not chunked, so the body runs until the peer
# closes. The cap is enforced on the buffered size as bytes arrive. A raw-net
# server answers two sequential connections, closing each socket right after the
# send so the client's EOF-framed read terminates:
#   1. EXACTLY the cap (4 bytes, cap 4) -> success.
#   2. one byte OVER the cap (5 bytes, cap 4) -> rejected mid-stream. The catch is
#      spelled `except Exception` to pin the top of the hierarchy.

def eof_server(channel[int64] portch, channel[int64] done):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c1 = net.accept(l)
    auto req1 = http.read_request(c1)
    auto n1 = net.send(c1, "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nabcd")
    net.close_conn(c1)
    auto c2 = net.accept(l)
    auto req2 = http.read_request(c2)
    auto n2 = net.send(c2, "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nabcde")
    net.close_conn(c2)
    auto ack = <-done
    net.close_listener(l)

def main():
    auto portch = channel[int64]()
    auto done = channel[int64]()
    spawn eof_server(portch, done)
    auto port = <-portch
    auto url = http.url("127.0.0.1", port, "/x")
    auto client = http.Client(max_response_bytes=4)
    auto r1 = client.get(url)
    print(http.status(r1))
    print(http.body(r1))
    try:
        auto r2 = client.get(url)
        print("unexpected: over-limit accepted")
    except Exception:
        print("eof-rejected-as-exception")
    client.shutdown()
    done <- 1
    print("done")
