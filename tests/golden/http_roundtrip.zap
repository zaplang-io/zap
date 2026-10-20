# Golden fixture: HTTP/1.1 client<->server round-trip (ticket #104, D-0056).
#
# A server goroutine serves exactly one request on an ephemeral port: it parses
# the request and replies 200 with a body echoing the method and path it saw.
# Main is the HTTP client: it builds the URL for the ephemeral port, GETs it,
# and prints the response status and body. Server and client each park on the
# netpoller across accept / connect / recv, so the exchange is deterministic.
#
# Covers the client (http.get -> status/body parsing) and the server
# (http.read_request -> method/path, http.respond) end to end.

def serve_one(channel[int64] portch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto req = http.read_request(c)
    auto reply = "method=" + http.method(req) + " path=" + http.path(req)
    http.respond(c, 200, reply)
    net.close_conn(c)
    net.close_listener(l)

def main():
    auto portch = channel[int64]()
    spawn serve_one(portch)
    auto port = <-portch
    auto url = http.url("127.0.0.1", port, "/hello")
    auto resp = http.get(url)
    print(http.status(resp))
    print(http.body(resp))
