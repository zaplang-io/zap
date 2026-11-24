# Golden fixture: http.Client(max_response_bytes=N) over Transfer-Encoding:
# chunked framing (#1448). The cap is enforced on the DECODED size as chunks are
# accumulated — there is no declared length to reject up front. A raw-net server
# answers two sequential connections:
#   1. two chunks decoding to EXACTLY the cap ("ab" + "cd" = 4, cap 4) -> success.
#   2. two chunks decoding to one byte OVER the cap ("abc" + "de" = 5, cap 4) ->
#      rejected mid-stream. The catch is spelled `except OSError` to pin the
#      hierarchy: ResponseTooLargeError is a subtype of OSError.
# Both responses carry `Connection: close`, so neither socket is pooled and the
# two accepts line up deterministically.

def chunk_server(channel[int64] portch, channel[int64] done):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c1 = net.accept(l)
    auto req1 = http.read_request(c1)
    auto n1 = net.send(
        c1,
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n2\r\nab\r\n2\r\ncd\r\n0\r\n\r\n",
    )
    auto c2 = net.accept(l)
    auto req2 = http.read_request(c2)
    auto n2 = net.send(
        c2,
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n",
    )
    auto ack = <-done
    net.close_conn(c1)
    net.close_conn(c2)
    net.close_listener(l)

def main():
    auto portch = channel[int64]()
    auto done = channel[int64]()
    spawn chunk_server(portch, done)
    auto port = <-portch
    auto url = http.url("127.0.0.1", port, "/x")
    auto client = http.Client(max_response_bytes=4)
    auto r1 = client.get(url)
    print(http.status(r1))
    print(http.body(r1))
    try:
        auto r2 = client.get(url)
        print("unexpected: over-limit accepted")
    except OSError:
        print("chunked-rejected-as-oserror")
    client.shutdown()
    done <- 1
    print("done")
