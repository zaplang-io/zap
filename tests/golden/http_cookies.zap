# Golden fixture: client response header enumeration + Set-Cookie parsing (#989).
#
# A raw-net server replies with a body and TWO `Set-Cookie` headers (which the
# single-value `http.header` could not both surface). The client reads the
# response, enumerates every header with `http.headers(resp)` — duplicates
# preserved, in order — and parses each `Set-Cookie` value with
# `http.parse_set_cookie`, printing the cookie name/value plus the Secure /
# HttpOnly flags. The exchange is one request with `Connection: close`, held open
# until the client signals it is done, so it is deterministic.

def ck_server(channel[int64] portch, channel[int64] done):
    auto l = net.listen("127.0.0.1", 0)
    portch <- net.port(l)
    auto c = net.accept(l)
    auto resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nSet-Cookie: a=1; Path=/; Secure\r\nSet-Cookie: b=2; HttpOnly\r\nConnection: close\r\n\r\nok"
    auto req = http.read_request(c)
    auto n = net.send(c, resp)
    auto ack = <-done
    net.close_conn(c)
    net.close_listener(l)

def main():
    auto portch = channel[int64]()
    auto done = channel[int64]()
    spawn ck_server(portch, done)
    auto port = <-portch
    auto url = http.url("127.0.0.1", port, "/x")
    auto r = http.get(url)
    print(http.status(r))
    print(http.body(r))
    auto hs = http.headers(r)
    auto nck = 0
    for name, value in hs:
        if name == "Set-Cookie":
            nck = nck + 1
            auto ck = http.parse_set_cookie(value)
            auto line = ck.name + "=" + ck.value
            if ck.secure:
                line = line + " secure"
            if ck.http_only:
                line = line + " httponly"
            print(line)
    print(str(nck))
    done <- 1
    print("done")
