# Golden fixture: enumerating the raw server request header block
# (ticket #960, D-0197) via `http.req_headers(req) -> list[(str, str)]`.
#
# The accessor returns every received header as a (name, value) pair in received
# order, preserving duplicate names as separate pairs; names come back verbatim
# (original case), while by-name discovery through the existing `http.req_header`
# stays case-insensitive. Because the raw request bytes (not a `dict`, whose
# iteration order is unspecified) are written on the wire by hand, the received
# order — and therefore the enumeration — is fully deterministic.
#
# Each exchange: a one-shot server coroutine binds an ephemeral port, accepts the
# connection, parses the request with `http.read_request`, enumerates the header
# block, and publishes the composed result over a `channel[str]` BEFORE it writes
# the 200 response — so the main coroutine's `net.recv` unblocks only after the
# result is queued, making the printed order independent of the scheduler. Main
# is the raw client (`net.connect` + `net.send` of a hand-written request record).
#
# Coverage:
#   * a custom header (`X-Custom-Thing`) is discoverable via enumeration AND
#     readable by name — case-insensitively (sent `X-Custom-Thing`, looked up
#     `x-custom-thing`);
#   * multiple distinct headers enumerate in received order;
#   * a duplicate header name is preserved as two separate pairs, and the
#     case-insensitive by-name lookup returns the FIRST of the duplicates;
#   * the standard subset (`Host`, `Accept`, `Content-Length`) still parses and
#     resolves by name (regression on `http.req_header`), with `HOST` proving the
#     case-insensitive lookup against a `Host:` line;
#   * a value that itself contains colons (`X-Url: http://x:1/p`) splits on the
#     FIRST colon only, so the colons inside the value survive;
#   * a header with no space after the colon (`X-Tight:tight`) keeps its full
#     value — the leading-whitespace trim is optional, not required;
#   * an empty header block (a bare request line) enumerates cleanly to the empty
#     list, and a by-name lookup on it returns `""`.

def serve(channel[int64] portch, channel[str] resultch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto req = http.read_request(c)
    # Enumerate the full header block in received order.
    auto hs = http.req_headers(req)
    auto out = str(len(hs))
    for name, value in hs:
        out = out + " [" + name + "=" + value + "]"
    # By-name discovery of a custom header, case-insensitively (first duplicate
    # wins); then a standard header looked up in a different case.
    out = out + " byname=" + http.req_header(req, "x-custom-thing")
    out = out + " host=" + http.req_header(req, "HOST")
    resultch <- out
    http.respond(c, 200, "ok")
    net.close_conn(c)
    net.close_listener(l)

def main():
    # Rich request: a custom header, a duplicated custom header, and the standard
    # subset, in a fixed wire order. `Content-Length: 0` terminates the read.
    auto pc1 = channel[int64]()
    auto rc1 = channel[str]()
    spawn serve(pc1, rc1)
    auto p1 = <-pc1
    auto c1 = net.connect("127.0.0.1", p1)
    auto req1 = "GET /x HTTP/1.1\r\nHost: Example.com\r\nX-Custom-Thing: alpha\r\nX-Custom-Thing: beta\r\nAccept: text/plain\r\nX-Url: http://x:1/p\r\nX-Tight:tight\r\nContent-Length: 0\r\n\r\n"
    net.send(c1, req1)
    auto reply1 = net.recv(c1, 1024)
    net.close_conn(c1)
    print(<-rc1)

    # Empty header block: a bare request line and the terminating blank line. The
    # enumeration is the empty list and a by-name lookup returns "".
    auto pc2 = channel[int64]()
    auto rc2 = channel[str]()
    spawn serve(pc2, rc2)
    auto p2 = <-pc2
    auto c2 = net.connect("127.0.0.1", p2)
    net.send(c2, "GET / HTTP/1.1\r\n\r\n")
    auto reply2 = net.recv(c2, 1024)
    net.close_conn(c2)
    print(<-rc2)
