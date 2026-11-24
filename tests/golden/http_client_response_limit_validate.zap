# Golden fixture: validation and composition of max_response_bytes (#1448).
#   1. A negative cap is a ValueError, raised before any dial — the bogus
#      unroutable URL is never contacted (mirrors the negative-timeout check).
#   2. Omitting the cap (the default, 0 = unlimited) keeps today's behavior: a
#      body larger than any of the capped goldens' limits is buffered and
#      returned in full, proving the control is opt-in and composes with a plain
#      client.

def big_server(channel[int64] portch, channel[int64] done):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto req = http.read_request(c)
    auto n = net.send(
        c,
        "HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello world",
    )
    auto ack = <-done
    net.close_conn(c)
    net.close_listener(l)

def main():
    try:
        auto bad_client = http.Client(max_response_bytes=-1)
        auto bad = bad_client.get("http://127.0.0.1:9/x")
        print("unexpected: negative cap accepted")
    except ValueError:
        print("negative-cap-rejected")
    auto portch = channel[int64]()
    auto done = channel[int64]()
    spawn big_server(portch, done)
    auto port = <-portch
    auto url = http.url("127.0.0.1", port, "/big")
    auto client = http.Client()
    auto r = client.get(url)
    print(http.status(r))
    print(http.body(r))
    client.shutdown()
    done <- 1
    print("done")
