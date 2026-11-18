# Golden fixture: the HTTP client carries an arbitrary BINARY request body — one
# with embedded NUL bytes — with a correct Content-Length, intact end to end.
#
# This is the core-`http` primitive the `requests` multipart builder
# (zaplang-io/requests#2) relies on: `multipart/form-data` file parts are binary,
# so the stdlib client must frame a body containing NULs and high bytes without
# truncating at a NUL or miscounting Content-Length. A one-shot server reads the
# request and echoes its body straight back; the client POSTs a 5-byte body with
# two embedded NULs (`b"a\0b\0c"`) and asserts all 5 bytes survived the
# round-trip. Every byte is < 0x80, so the byte length and the codepoint count
# agree and the assertion is independent of collector and arch.

def echo_body(channel[int64] portch):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto req = http.read_request(c)
    http.respond(c, 200, http.req_body(req))
    net.close_conn(c)
    net.close_listener(l)

def main():
    auto portch = channel[int64]()
    spawn echo_body(portch)
    auto port = <-portch
    auto url = http.url("127.0.0.1", port, "/upload")
    auto resp = http.post(url, b"a\0b\0c")
    auto got = http.body(resp)
    print(http.status(resp))
    if len(got) == 5:
        print("bytes-roundtrip-ok")
    else:
        print("bytes-roundtrip-bad")
