# Golden fixture: http.Client decodes a Transfer-Encoding: chunked response body
# (#990). A raw-net server replies with a two-chunk body ("foo" + "bar")
# terminated by a zero-length chunk; the client reads it through http.Client and
# the decoded body is the concatenation "foobar". Connection: close here, so the
# socket is not pooled — this isolates the chunked decoder from connection reuse.

def chunk_server(channel[int64] portch, channel[int64] done):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto resp = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nfoo\r\n3\r\nbar\r\n0\r\n\r\n"
    auto req = http.read_request(c)
    auto n = net.send(c, resp)
    auto ack = <-done
    net.close_conn(c)
    net.close_listener(l)

def main():
    auto portch = channel[int64]()
    auto done = channel[int64]()
    spawn chunk_server(portch, done)
    auto port = <-portch
    auto url = http.url("127.0.0.1", port, "/x")
    auto client = http.Client()
    auto r = client.get(url)
    print(http.status(r))
    print(http.body(r))
    client.shutdown()
    done <- 1
    print("done")
