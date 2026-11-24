# Golden fixture: http.Client(max_response_bytes=N) over Content-Length framing
# (#1448). A raw-net server answers two sequential connections:
#   1. a body of EXACTLY the cap (4 bytes, cap 4) -> success.
#   2. a declared Content-Length OVER the cap (5 bytes, cap 4) -> rejected off the
#      header alone (before any body byte is read) with a ResponseTooLargeError.
# Both responses carry `Connection: close`, so each request dials a fresh socket
# and the two server accepts line up deterministically.

def cl_server(channel[int64] portch, channel[int64] done):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c1 = net.accept(l)
    auto req1 = http.read_request(c1)
    auto n1 = net.send(
        c1,
        "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabcd",
    )
    auto c2 = net.accept(l)
    auto req2 = http.read_request(c2)
    auto n2 = net.send(
        c2,
        "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nabcde",
    )
    auto ack = <-done
    net.close_conn(c1)
    net.close_conn(c2)
    net.close_listener(l)

def main():
    auto portch = channel[int64]()
    auto done = channel[int64]()
    spawn cl_server(portch, done)
    auto port = <-portch
    auto url = http.url("127.0.0.1", port, "/x")
    auto client = http.Client(max_response_bytes=4)
    auto r1 = client.get(url)
    print(http.status(r1))
    print(http.body(r1))
    try:
        auto r2 = client.get(url)
        print("unexpected: over-limit accepted")
    except ResponseTooLargeError:
        print("content-length-rejected")
    client.shutdown()
    done <- 1
    print("done")
