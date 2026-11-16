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
# block into the composed line, and writes it back as the 200 response body. Main
# is the raw client (`net.connect` + a hand-written request record); it reads the
# whole response to EOF (the server writes `Connection: close`, so the read loop
# terminates deterministically regardless of TCP segmentation) and recovers the
# body by splitting the response at the blank line. Carrying the result in the
# response body (not a side channel) keeps the single request/response exchange
# free of any cross-coroutine wait cycle.
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

def serve(channel[int64] portch):
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
    http.respond(c, 200, out)
    net.close_conn(c)
    net.close_listener(l)

def read_all(bytes conn) -> str:
    # Read to EOF: the server sends `Connection: close`, so recv returns "" once
    # the whole response has arrived, independent of how TCP split the bytes.
    auto acc = ""
    auto chunk = net.recv(conn, 4096)
    while len(chunk) > 0:
        acc = acc + chunk
        chunk = net.recv(conn, 4096)
    return acc

def body_of(str resp) -> str:
    # The response body is everything after the blank line that ends the headers.
    auto parts = resp.split("\r\n\r\n")
    return parts[1]

def main():
    # Rich request: a custom header, a duplicated custom header, and the standard
    # subset, in a fixed wire order. `Content-Length: 0` terminates the read.
    auto pc1 = channel[int64]()
    spawn serve(pc1)
    auto p1 = <-pc1
    auto c1 = net.connect("127.0.0.1", p1)
    auto req1 = "GET /x HTTP/1.1\r\nHost: Example.com\r\nX-Custom-Thing: alpha\r\nX-Custom-Thing: beta\r\nAccept: text/plain\r\nX-Url: http://x:1/p\r\nX-Tight:tight\r\nContent-Length: 0\r\n\r\n"
    net.send(c1, req1)
    auto resp1 = read_all(c1)
    net.close_conn(c1)
    print(body_of(resp1))

    # Empty header block: a bare request line and the terminating blank line. The
    # enumeration is the empty list and a by-name lookup returns "".
    auto pc2 = channel[int64]()
    spawn serve(pc2)
    auto p2 = <-pc2
    auto c2 = net.connect("127.0.0.1", p2)
    net.send(c2, "GET / HTTP/1.1\r\n\r\n")
    auto resp2 = read_all(c2)
    net.close_conn(c2)
    print(body_of(resp2))
