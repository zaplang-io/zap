# Golden fixture: HTTP client redirect following (#987, D-0222).
#
# A server coroutine serves exactly two requests on one ephemeral port. The
# first (path /start) replies 302 with a RELATIVE "Location: /final"; the second
# (path /final) replies 200 with a body. Main issues a single http.get on /start
# and lets the client follow the redirect: it must resolve the relative Location
# against the request URL (url.join / RFC-3986) to reach the right host:port, so
# arriving at /final with body "arrived" proves the resolution end to end.
#
# Output is deterministic: the ephemeral port never appears. We print the final
# status (200), the final body (arrived), and the number of redirects followed
# (1) via http.redirects.

def serve_redirect(channel[int64] portch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c1 = net.accept(l)
    http.read_request(c1)
    http.respond(c1, 302, "", {"Location": "/final"})
    net.close_conn(c1)
    auto c2 = net.accept(l)
    http.read_request(c2)
    http.respond(c2, 200, "arrived")
    net.close_conn(c2)
    net.close_listener(l)

def main():
    auto portch = channel[int64]()
    spawn serve_redirect(portch)
    auto port = <-portch
    auto url = http.url("127.0.0.1", port, "/start")
    auto resp = http.get(url)
    print(http.status(resp))
    print(http.body(resp))
    print(len(http.redirects(resp)))
