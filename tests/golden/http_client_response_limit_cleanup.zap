# Golden fixture: a partial over-limit response must NOT leave a reusable
# connection in the keep-alive pool (#1448). Unlike the framing goldens, the
# over-limit response here is `Connection: keep-alive`, so WITHOUT eviction the
# rejected socket — still holding the unread body — would be returned to the pool
# and reused for the next request, corrupting it.
#
# The server accepts TWO connections on one origin:
#   A. an over-limit keep-alive response (Content-Length 5, cap 4). The client
#      rejects it with ResponseTooLargeError and must CLOSE the socket.
#   B. a fresh connection serving a normal body. It is reached only if the
#      follow-up GET dialed anew — i.e. conn A was evicted, not pooled.
# The follow-up returning its own body ("wxyz") is the proof the client stayed
# clean and the poisoned socket never re-entered the pool.

def evict_server(channel[int64] portch, channel[int64] done):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto cA = net.accept(l)
    auto reqA = http.read_request(cA)
    auto nA = net.send(
        cA,
        "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: keep-alive\r\n\r\nabcde",
    )
    auto cB = net.accept(l)
    auto reqB = http.read_request(cB)
    auto nB = net.send(
        cB,
        "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: keep-alive\r\n\r\nwxyz",
    )
    auto ack = <-done
    net.close_conn(cA)
    net.close_conn(cB)
    net.close_listener(l)

def main():
    auto portch = channel[int64]()
    auto done = channel[int64]()
    spawn evict_server(portch, done)
    auto port = <-portch
    auto url = http.url("127.0.0.1", port, "/x")
    auto client = http.Client(max_response_bytes=4)
    try:
        auto bad = client.get(url)
        print("unexpected: over-limit accepted")
    except ResponseTooLargeError:
        print("rejected-and-evicted")
    auto good = client.get(url)
    print(http.status(good))
    print(http.body(good))
    client.shutdown()
    done <- 1
    print("done")
