# Golden fixture: HTTP/1.1 request parsing edge cases (ticket #104, D-0056).
#
# Exercises the server-side parser against a POST carrying a body: the client
# (http.post) sends a request line with a path, a Content-Length + Content-Type
# header, and a body; the server reads it with http.read_request and reports back
# the method, path, a case-insensitively looked-up header value, and the body it
# parsed. A non-200 status (201 Created) checks the response status line both
# ways. One request, served then joined — deterministic.

def serve_one(channel[int64] portch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto req = http.read_request(c)
    auto out = http.method(req) + " " + http.path(req) + " ct=" + http.req_header(req, "content-type") + " body=" + http.req_body(req)
    http.respond(c, 201, out)
    net.close_conn(c)
    net.close_listener(l)

def main():
    auto portch = channel[int64]()
    spawn serve_one(portch)
    auto port = <-portch
    auto url = http.url("127.0.0.1", port, "/submit")
    auto resp = http.post(url, "name=zap")
    print(http.status(resp))
    print(http.body(resp))
