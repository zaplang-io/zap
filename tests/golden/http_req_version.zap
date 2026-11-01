# Golden fixture: http.read_request exposes the request-line HTTP version (#486).
#
# The client (http.get) dials the ephemeral server with a known request line
# "GET /ping HTTP/1.1"; the server reads it with http.read_request and reports
# back the method, path and the version token parsed off that line. Printing
# method/path alongside the version confirms the pre-existing accessors are
# unchanged by retaining the version field. One request, served then joined —
# deterministic under both collectors and arches (no wall-clock, no heap bytes).

def serve_one(channel[int64] portch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto req = http.read_request(c)
    auto out = http.method(req) + " " + http.path(req) + " " + http.req_version(req)
    http.respond(c, 200, out)
    net.close_conn(c)
    net.close_listener(l)

def main():
    auto portch = channel[int64]()
    spawn serve_one(portch)
    auto port = <-portch
    auto url = http.url("127.0.0.1", port, "/ping")
    auto resp = http.get(url)
    print(http.status(resp))
    print(http.body(resp))
