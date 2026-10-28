# Golden fixture: HTTP/1.1 server response headers (ticket #279). The server
# handler sets a dict[str,str] of custom response headers on http.respond —
# modeling the AWS Lambda Runtime API, which drives its client through
# Lambda-Runtime-* response headers — plus a Content-Type override and two
# reserved names (Content-Length / Connection) that the server must ignore and
# recompute. The client reads them back with http.status / http.header /
# http.body, proving the custom headers travelled and the reserved ones stayed
# server-owned (exactly one of each on the wire). Deterministic: a one-shot
# server on an ephemeral port, parked on the netpoller.

def serve_invocation(channel[int64] portch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto req = http.read_request(c)
    auto h = {
        "Lambda-Runtime-Aws-Request-Id": "req-8badf00d",
        "Lambda-Runtime-Invoked-Function-Arn": "arn:aws:lambda:us-east-1:0:function:zap",
        "Content-Type": "application/json",
        "Content-Length": "999",
        "Connection": "keep-alive",
    }
    http.respond(c, 200, "ok", h)
    net.close_conn(c)
    net.close_listener(l)

def main():
    auto pc = channel[int64]()
    spawn serve_invocation(pc)
    auto p = <-pc
    auto r = http.get(http.url("127.0.0.1", p, "/2018-06-01/runtime/invocation/next"))

    # Custom Lambda-Runtime-* headers round-trip to the client verbatim.
    print(http.status(r))
    print(http.header(r, "lambda-runtime-aws-request-id"))
    print(http.header(r, "lambda-runtime-invoked-function-arn"))
    # Content-Type override replaces the default text/plain.
    print(http.header(r, "content-type"))
    # Content-Length / Connection are server-managed: the caller's bogus values
    # were dropped and the server recomputed the real length ("ok" = 2).
    print(http.header(r, "content-length"))
    print(http.header(r, "connection"))
    print(http.body(r))
