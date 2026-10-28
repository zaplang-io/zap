# Golden fixture: HTTP/1.1 client request headers (ticket #280) + response
# accessors. The client sets a `dict[str,str]` of request headers on each of the
# three verb forms — get(url, headers), post(url, body, headers) and
# request(method, url, body, headers) — and the server echoes the specific
# header values it parsed back into the body, so the exchange proves the custom
# headers (incl. an Authorization header and a Content-Type override) travelled
# on the wire. Main then reads the response status, a response header, and the
# body back through http.status / http.header / http.body. Deterministic: three
# sequential one-shot servers on ephemeral ports, each parked on the netpoller.

def serve_echo(channel[int64] portch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto req = http.read_request(c)
    auto out = http.method(req) + " trace=" + http.req_header(req, "x-trace-id") + " auth=" + http.req_header(req, "authorization") + " ct=" + http.req_header(req, "content-type") + " body=" + http.req_body(req)
    http.respond(c, 200, out)
    net.close_conn(c)
    net.close_listener(l)

def main():
    auto h = {
        "X-Trace-Id": "t-1",
        "Authorization": "Bearer tok",
        "Content-Type": "application/json",
    }

    # get(url, headers): no body, but the caller-supplied headers still travel.
    auto pc1 = channel[int64]()
    spawn serve_echo(pc1)
    auto p1 = <-pc1
    auto r1 = http.get(http.url("127.0.0.1", p1, "/a"), h)
    print(http.status(r1))
    print(http.body(r1))

    # post(url, body, headers): Content-Type overrides the text/plain default.
    auto pc2 = channel[int64]()
    spawn serve_echo(pc2)
    auto p2 = <-pc2
    auto r2 = http.post(http.url("127.0.0.1", p2, "/b"), "name=zap", h)
    print(http.header(r2, "content-type"))
    print(http.body(r2))

    # request(method, url, body, headers): the general form.
    auto pc3 = channel[int64]()
    spawn serve_echo(pc3)
    auto p3 = <-pc3
    auto r3 = http.request("PUT", http.url("127.0.0.1", p3, "/c"), "x=1", h)
    print(http.body(r3))
